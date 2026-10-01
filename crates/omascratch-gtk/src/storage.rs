//! Desktop/storage adapter: opens (or creates) the startup notebook+note and
//! runs autosave. Blocking I/O happens on `gio::spawn_blocking` workers; the
//! only exception is the final save on close, which must complete before the
//! process exits.
//!
//! Save generations: every save job carries the session revision it
//! snapshotted. A completion only advances `last_saved` monotonically, so a
//! stale worker can never mark newer content as saved.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

use gtk4::{gio, glib, prelude::*};

use omascratch_store as store;
use store::{NoteDoc, Settings};

use crate::canvas::CanvasView;

const AUTOSAVE_DEBOUNCE_MS: u32 = 2_000;
const AUTOSAVE_CAP_MS: u32 = 20_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Storage {
    pub doc: RefCell<NoteDoc>,
    pub note_path: PathBuf,
    last_saved: Cell<u64>,
    saving: Cell<bool>,
    debounce: RefCell<Option<glib::SourceId>>,
    cap: RefCell<Option<glib::SourceId>>,
}

impl Storage {
    /// Open the most recently modified note anywhere under the notebooks
    /// root, or create "My Notebook" with a first note. Runs once at startup
    /// on the main thread (small files; async loading comes with the M3
    /// navigation UI).
    pub fn open_startup_note() -> Rc<Storage> {
        let settings = Settings::load_or_default(&store::config_dir());
        let root = settings.notebooks_root.clone();
        std::fs::create_dir_all(&root).ok();
        for d in [store::config_dir(), store::state_dir(), store::cache_dir()] {
            std::fs::create_dir_all(d).ok();
        }
        // Persist default settings on first run so the user can find and
        // edit the notebooks_root; never rewrite an existing file.
        if !store::config_dir().join("settings.toml").exists() {
            settings.save(&store::config_dir()).ok();
        }

        let (notebooks, skipped) = store::scan_root(&root).unwrap_or_default();
        for s in &skipped {
            tracing::warn!("skipped unreadable notebook dir: {}", s.display());
        }

        let (doc, path) = Self::pick_or_create_note(&root, notebooks);
        Rc::new(Storage {
            doc: RefCell::new(doc),
            note_path: path,
            last_saved: Cell::new(0),
            saving: Cell::new(false),
            debounce: RefCell::new(None),
            cap: RefCell::new(None),
        })
    }

    fn pick_or_create_note(
        root: &std::path::Path,
        notebooks: Vec<store::NotebookTree>,
    ) -> (NoteDoc, PathBuf) {
        // Newest-modified loadable note wins.
        let mut best: Option<(u64, NoteDoc, PathBuf)> = None;
        for nb in &notebooks {
            for entry in &nb.notes {
                if entry.conflict {
                    continue;
                }
                match store::read_note(&entry.path) {
                    Ok(doc) => {
                        if best.as_ref().is_none_or(|(m, _, _)| doc.modified_ms > *m) {
                            best = Some((doc.modified_ms, doc, entry.path.clone()));
                        }
                    }
                    Err(e) => {
                        // Corrupt or newer-schema: leave the file untouched.
                        tracing::warn!("cannot open note: {e}");
                    }
                }
            }
        }
        if let Some((_, doc, path)) = best {
            return (doc, path);
        }

        // First run (or nothing loadable): create a notebook + empty note.
        let nb_dir = root.join("My Notebook");
        let nb = match store::scan_notebook(&nb_dir) {
            Ok(Some(tree)) => tree,
            _ => store::create_notebook(root, "My Notebook", now_ms())
                .expect("cannot create the first notebook; check the notebooks folder permissions"),
        };
        let doc = NoteDoc {
            id: omascratch_core::NoteId::new(),
            title: "First note".into(),
            folder: None,
            order_key: "a0".into(),
            created_ms: now_ms(),
            modified_ms: now_ms(),
            content: omascratch_core::NoteContent::default(),
            opaque_elements: vec![],
        };
        let path = store::note_path(&nb.dir, doc.id);
        store::write_note(&path, &doc, doc.modified_ms)
            .expect("cannot write the first note; check the notebooks folder permissions");
        (doc, path)
    }

    /// Install autosave: debounce + hard cap, snapshots taken from the canvas.
    pub fn attach_autosave(self: &Rc<Self>, canvas: &CanvasView) {
        let storage = self.clone();
        let canvas_weak = canvas.downgrade();
        canvas.set_on_change(move |/* change */| {
            let Some(canvas) = canvas_weak.upgrade() else { return };

            // Reset the debounce timer.
            if let Some(id) = storage.debounce.borrow_mut().take() {
                id.remove();
            }
            let s2 = storage.clone();
            let c2 = canvas.clone();
            let id = glib::timeout_add_local_once(
                std::time::Duration::from_millis(AUTOSAVE_DEBOUNCE_MS as u64),
                move || {
                    *s2.debounce.borrow_mut() = None;
                    s2.save_async(&c2);
                },
            );
            *storage.debounce.borrow_mut() = Some(id);

            // Hard cap: at most CAP ms between a change and a save, even
            // under continuous drawing.
            if storage.cap.borrow().is_none() {
                let s2 = storage.clone();
                let c2 = canvas.clone();
                let id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(AUTOSAVE_CAP_MS as u64),
                    move || {
                        *s2.cap.borrow_mut() = None;
                        s2.save_async(&c2);
                    },
                );
                *storage.cap.borrow_mut() = Some(id);
            }
        });
    }

    /// Snapshot + write on a worker. Generation-checked, re-entrancy-guarded.
    pub fn save_async(self: &Rc<Self>, canvas: &CanvasView) {
        let (revision, content) = canvas.content_snapshot();
        if revision <= self.last_saved.get() || self.saving.get() {
            return;
        }
        self.saving.set(true);

        let mut doc = self.doc.borrow().clone();
        doc.content = content;
        let path = self.note_path.clone();
        let modified = now_ms();

        let storage = self.clone();
        let canvas_weak = canvas.downgrade();
        glib::spawn_future_local(async move {
            let result =
                gio::spawn_blocking(move || store::write_note(&path, &doc, modified)).await;
            storage.saving.set(false);
            match result {
                Ok(Ok(())) => {
                    // Monotonic: a stale completion can't regress this.
                    if revision > storage.last_saved.get() {
                        storage.last_saved.set(revision);
                    }
                    // Changes may have landed while the worker ran.
                    if let Some(canvas) = canvas_weak.upgrade() {
                        if canvas.revision() > storage.last_saved.get() {
                            storage.save_async(&canvas);
                        }
                    }
                }
                Ok(Err(e)) => tracing::error!("autosave failed: {e}"),
                Err(_) => tracing::error!("autosave worker panicked"),
            }
        });
    }

    /// Final save on close. Deliberately synchronous: the window is about to
    /// be destroyed and the write must complete before process exit.
    pub fn save_final(&self, canvas: &CanvasView) -> Result<(), store::StoreError> {
        if let Some(id) = self.debounce.borrow_mut().take() {
            id.remove();
        }
        if let Some(id) = self.cap.borrow_mut().take() {
            id.remove();
        }
        let (revision, content) = canvas.content_snapshot();
        if revision <= self.last_saved.get() {
            return Ok(());
        }
        let mut doc = self.doc.borrow().clone();
        doc.content = content;
        store::write_note(&self.note_path, &doc, now_ms())?;
        self.last_saved.set(revision);
        Ok(())
    }
}
