//! Desktop/storage adapter: manages the one open note and its autosave.
//! Blocking I/O runs on `gio::spawn_blocking` workers; the final save on
//! close/switch is synchronous so it completes before the note is replaced.
//!
//! Save generations: every save job carries the session revision it
//! snapshotted. `last_saved` only advances monotonically, so a stale worker
//! can never mark newer content as saved, and switching notes resets the
//! baseline so the next note starts clean.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gtk4::{gio, glib, prelude::*};

use omascratch_store as store;
use store::NoteDoc;

use crate::canvas::CanvasView;

const AUTOSAVE_DEBOUNCE_MS: u64 = 2_000;
const AUTOSAVE_CAP_MS: u64 = 20_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Storage {
    doc: RefCell<NoteDoc>,
    path: RefCell<PathBuf>,
    last_saved: Cell<u64>,
    saving: Cell<bool>,
    debounce: RefCell<Option<glib::SourceId>>,
    cap: RefCell<Option<glib::SourceId>>,
}

impl Storage {
    /// Open an existing note file as the initial open note.
    pub fn open(path: PathBuf) -> Rc<Storage> {
        let doc = crate::perf::time("startup note read+decode", || store::read_note(&path)).unwrap_or_else(|_| NoteDoc {
            id: omascratch_core::NoteId::new(),
            title: "Untitled".into(),
            folder: None,
            order_key: "a0".into(),
            created_ms: now_ms(),
            modified_ms: now_ms(),
            background: Default::default(),
            content: omascratch_core::NoteContent::default(),
            opaque_elements: vec![],
        });
        Rc::new(Storage {
            doc: RefCell::new(doc),
            path: RefCell::new(path),
            last_saved: Cell::new(0),
            saving: Cell::new(false),
            debounce: RefCell::new(None),
            cap: RefCell::new(None),
        })
    }

    pub fn current_path(&self) -> PathBuf {
        self.path.borrow().clone()
    }

    pub fn current_title(&self) -> String {
        self.doc.borrow().title.clone()
    }

    pub fn load_into_canvas(&self, canvas: &CanvasView) {
        canvas.set_asset_dir(store::assets_dir(&self.path.borrow()));
        canvas.set_content(self.doc.borrow().content.clone());
        canvas.set_background(self.doc.borrow().background);
        // A freshly loaded note starts at session revision 0.
        self.last_saved.set(0);
    }

    /// Update the open note's page background and write it synchronously.
    pub fn set_background(self: &Rc<Self>, bg: omascratch_core::PageBackground, canvas: &CanvasView) {
        self.doc.borrow_mut().background = bg;
        let (revision, content) = canvas.content_snapshot();
        let mut doc = self.doc.borrow().clone();
        doc.content = content;
        if let Err(e) = store::write_note(&self.path.borrow(), &doc, now_ms()) {
            tracing::error!("background save failed: {e}");
            return;
        }
        if revision > self.last_saved.get() {
            self.last_saved.set(revision);
        }
    }

    /// Persist pasted image bytes beside the open note; returns the asset
    /// name. Synchronous: it runs once per paste and must exist before the
    /// image element referencing it is saved.
    pub fn write_asset(&self, bytes: &[u8], ext: &str) -> Option<String> {
        match store::write_asset(&self.path.borrow(), bytes, ext) {
            Ok(name) => Some(name),
            Err(e) => {
                tracing::error!("saving pasted image failed: {e}");
                None
            }
        }
    }

    /// The open note's file moved (notebook renamed): keep content, follow path.
    pub fn relocate(&self, new_path: PathBuf) {
        *self.path.borrow_mut() = new_path;
    }

    /// Open a note WITHOUT saving the current one (its notebook was trashed).
    pub fn abandon_and_open(self: &Rc<Self>, path: &Path, canvas: &CanvasView) {
        if let Ok(doc) = store::read_note(path) {
            *self.doc.borrow_mut() = doc;
            *self.path.borrow_mut() = path.to_path_buf();
            self.load_into_canvas(canvas);
        }
    }

    /// Switch the open note: save the current one synchronously, then load the
    /// new file. Returns the new note's title for the breadcrumb.
    pub fn switch_to(self: &Rc<Self>, path: &Path, canvas: &CanvasView) -> String {
        canvas.commit_text_edit();
        if *self.path.borrow() != path {
            let _ = self.save_final(canvas);
        }
        let doc = match crate::perf::time("note read+decode", || store::read_note(path)) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("cannot open note {}: {e}", path.display());
                return self.current_title();
            }
        };
        let title = doc.title.clone();
        *self.doc.borrow_mut() = doc;
        *self.path.borrow_mut() = path.to_path_buf();
        self.load_into_canvas(canvas);
        title
    }

    /// Update the open note's title and write it **synchronously** (tiny file),
    /// so the sidebar can re-read the new title immediately without racing the
    /// async autosave worker.
    pub fn set_title(self: &Rc<Self>, title: &str, canvas: &CanvasView) {
        self.doc.borrow_mut().title = title.to_string();
        let (revision, content) = canvas.content_snapshot();
        let mut doc = self.doc.borrow().clone();
        doc.content = content;
        if let Err(e) = store::write_note(&self.path.borrow(), &doc, now_ms()) {
            tracing::error!("title save failed: {e}");
            return;
        }
        if revision > self.last_saved.get() {
            self.last_saved.set(revision);
        }
    }

    pub fn attach_autosave(self: &Rc<Self>, canvas: &CanvasView) {
        let storage = self.clone();
        let canvas_weak = canvas.downgrade();
        canvas.set_on_change(move || {
            let Some(canvas) = canvas_weak.upgrade() else { return };
            if let Some(id) = storage.debounce.borrow_mut().take() {
                id.remove();
            }
            let s2 = storage.clone();
            let c2 = canvas.clone();
            let id = glib::timeout_add_local_once(
                std::time::Duration::from_millis(AUTOSAVE_DEBOUNCE_MS),
                move || {
                    *s2.debounce.borrow_mut() = None;
                    s2.save_now(&c2, false);
                },
            );
            *storage.debounce.borrow_mut() = Some(id);

            if storage.cap.borrow().is_none() {
                let s2 = storage.clone();
                let c2 = canvas.clone();
                let id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(AUTOSAVE_CAP_MS),
                    move || {
                        *s2.cap.borrow_mut() = None;
                        s2.save_now(&c2, false);
                    },
                );
                *storage.cap.borrow_mut() = Some(id);
            }
        });
    }

    /// Snapshot + write on a worker. `force` saves even when the ink revision
    /// is unchanged (used for metadata like title).
    fn save_now(self: &Rc<Self>, canvas: &CanvasView, force: bool) {
        let (revision, content) = canvas.content_snapshot();
        if (!force && revision <= self.last_saved.get()) || self.saving.get() {
            return;
        }
        self.saving.set(true);

        let mut doc = self.doc.borrow().clone();
        doc.content = content;
        let path = self.path.borrow().clone();
        let modified = now_ms();

        let storage = self.clone();
        let canvas_weak = canvas.downgrade();
        glib::spawn_future_local(async move {
            let result =
                gio::spawn_blocking(move || store::write_note(&path, &doc, modified)).await;
            storage.saving.set(false);
            match result {
                Ok(Ok(())) => {
                    if revision > storage.last_saved.get() {
                        storage.last_saved.set(revision);
                    }
                    if let Some(canvas) = canvas_weak.upgrade() {
                        if canvas.revision() > storage.last_saved.get() {
                            storage.save_now(&canvas, false);
                        }
                    }
                }
                Ok(Err(e)) => tracing::error!("autosave failed: {e}"),
                Err(_) => tracing::error!("autosave worker panicked"),
            }
        });
    }

    /// Final synchronous save (close or note switch). Cancels pending timers.
    pub fn save_final(&self, canvas: &CanvasView) -> Result<(), store::StoreError> {
        canvas.commit_text_edit();
        if let Some(id) = self.debounce.borrow_mut().take() {
            id.remove();
        }
        if let Some(id) = self.cap.borrow_mut().take() {
            id.remove();
        }
        let (revision, content) = canvas.content_snapshot();
        let mut doc = self.doc.borrow().clone();
        // Always write on switch/close if there is any unsaved ink OR the
        // caller bumped the title (which already forced a save, but be safe).
        if revision <= self.last_saved.get() {
            return Ok(());
        }
        doc.content = content;
        store::write_note(&self.path.borrow(), &doc, now_ms())?;
        self.last_saved.set(revision);
        Ok(())
    }
}
