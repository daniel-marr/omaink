//! Library model for the sidebar: the scanned notebook tree plus the mutation
//! operations the UI invokes. Thin wrapper over `omaink_store`; it keeps
//! the current notebook scan in memory and rescans after each change.
//!
//! Scans here are synchronous (metadata-only, small). Note *content* loading
//! and saving stays on workers in `storage.rs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use omaink_core::{FolderId, NoteId};
use omaink_store as store;
use store::{FolderMeta, NoteMeta, NotebookTree, Settings};

const META_CACHE_FILE: &str = "note-meta.json";

/// Cached note header, valid while the file's mtime and size are unchanged.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct MetaEntry {
    mtime_ns: u64,
    size: u64,
    meta: NoteMeta,
}

/// One row the sidebar can show, already resolved to display data.
#[derive(Debug, Clone)]
pub enum Row {
    Folder { id: FolderId, title: String, depth: u32 },
    Note { path: PathBuf, title: String, depth: u32, conflict: bool },
}

pub struct Library {
    pub root: PathBuf,
    /// All notebooks under the root (names only matter for the switcher).
    pub notebooks: RefCell<Vec<String>>,
    /// The currently selected notebook's full scan.
    pub current: RefCell<Option<NotebookTree>>,
    /// Note headers keyed by path; persisted to the XDG cache so cold starts
    /// only stat files instead of decoding every note.
    meta_cache: RefCell<HashMap<String, MetaEntry>>,
    meta_dirty: Cell<bool>,
}

impl Library {
    pub fn open() -> Rc<Library> {
        let settings = Settings::load_or_default(&store::config_dir());
        let root = settings.notebooks_root.clone();
        std::fs::create_dir_all(&root).ok();
        let (notebooks, _skipped) = store::scan_root(&root).unwrap_or_default();
        let names: Vec<String> = notebooks.iter().map(|n| n.meta.name.clone()).collect();
        let current = notebooks.into_iter().next();
        let meta_cache = std::fs::read(store::cache_dir().join(META_CACHE_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Rc::new(Library {
            root,
            notebooks: RefCell::new(names),
            current: RefCell::new(current),
            meta_cache: RefCell::new(meta_cache),
            meta_dirty: Cell::new(false),
        })
    }

    /// A note's header: from cache if the file is unchanged, else decoded
    /// (header only) and cached.
    fn note_meta(&self, path: &Path) -> Option<NoteMeta> {
        let md = std::fs::metadata(path).ok()?;
        let mtime_ns = md
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos() as u64;
        let size = md.len();
        let key = path.to_string_lossy().to_string();
        if let Some(e) = self.meta_cache.borrow().get(&key) {
            if e.mtime_ns == mtime_ns && e.size == size {
                return Some(e.meta.clone());
            }
        }
        let meta = store::read_note_meta(path).ok()?;
        self.meta_cache
            .borrow_mut()
            .insert(key, MetaEntry { mtime_ns, size, meta: meta.clone() });
        self.meta_dirty.set(true);
        Some(meta)
    }

    /// Write the cache if it changed, dropping entries for vanished files.
    fn persist_meta_cache(&self) {
        if !self.meta_dirty.replace(false) {
            return;
        }
        self.meta_cache.borrow_mut().retain(|k, _| Path::new(k).exists());
        let dir = store::cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(bytes) = serde_json::to_vec(&*self.meta_cache.borrow()) {
            let _ = store::atomic::atomic_write(&dir.join(META_CACHE_FILE), &bytes);
        }
    }

    pub fn current_notebook_name(&self) -> String {
        self.current
            .borrow()
            .as_ref()
            .map(|n| n.meta.name.clone())
            .unwrap_or_else(|| "No notebook".to_string())
    }

    pub fn notebook_names(&self) -> Vec<String> {
        self.notebooks.borrow().clone()
    }

    fn rescan_current(&self) {
        let dir = self.current.borrow().as_ref().map(|n| n.dir.clone());
        if let Some(dir) = dir {
            if let Ok(Some(tree)) = store::reload_notebook(&dir) {
                *self.current.borrow_mut() = Some(tree);
            }
        }
    }

    pub fn select_notebook(&self, name: &str) {
        let dir = self.root.join(name);
        if let Ok(Some(tree)) = store::scan_notebook(&dir) {
            *self.current.borrow_mut() = Some(tree);
        }
    }

    /// Flatten the current notebook into display rows: folders (depth-first by
    /// order key) each followed by their notes, then top-level notes.
    pub fn rows(&self) -> Vec<Row> {
        crate::perf::time("sidebar rows()", || self.rows_inner())
    }

    fn rows_inner(&self) -> Vec<Row> {
        let guard = self.current.borrow();
        let Some(nb) = guard.as_ref() else { return Vec::new() };

        // Note headers via the metadata cache. Rows are keyed by path, so
        // sync conflict copies (whose filenames aren't plain ids) show up too.
        let mut notes: Vec<(NoteId, PathBuf, String, Option<FolderId>, String, bool)> = Vec::new();
        for entry in &nb.notes {
            if let Some(m) = self.note_meta(&entry.path) {
                let title = if entry.conflict { format!("{} (sync conflict)", m.title) } else { m.title };
                notes.push((m.id, entry.path.clone(), title, m.folder, m.order_key, entry.conflict));
            }
        }
        drop(guard);
        self.persist_meta_cache();
        let guard = self.current.borrow();
        let Some(nb) = guard.as_ref() else { return Vec::new() };

        let mut rows = Vec::new();
        fn push_folder(
            rows: &mut Vec<Row>,
            folders: &[FolderMeta],
            notes: &[(NoteId, PathBuf, String, Option<FolderId>, String, bool)],
            parent: Option<FolderId>,
            depth: u32,
        ) {
            let mut here: Vec<&FolderMeta> = folders.iter().filter(|f| f.parent == parent).collect();
            here.sort_by(|a, b| a.order_key.cmp(&b.order_key).then(a.id.cmp(&b.id)));
            for f in here {
                rows.push(Row::Folder { id: f.id, title: f.title.clone(), depth });
                push_folder(rows, folders, notes, Some(f.id), depth + 1);
                let mut child_notes: Vec<_> =
                    notes.iter().filter(|n| n.3 == Some(f.id)).collect();
                child_notes.sort_by(|a, b| a.4.cmp(&b.4));
                for n in child_notes {
                    rows.push(Row::Note {
                        path: n.1.clone(),
                        title: n.2.clone(),
                        depth: depth + 1,
                        conflict: n.5,
                    });
                }
            }
        }
        push_folder(&mut rows, &nb.folders, &notes, None, 0);

        // Top-level notes last (OneNote/Obsidian both show loose notes under the tree).
        let mut top: Vec<_> = notes.iter().filter(|n| n.3.is_none()).collect();
        top.sort_by(|a, b| a.4.cmp(&b.4));
        for n in top {
            rows.push(Row::Note {
                path: n.1.clone(),
                title: n.2.clone(),
                depth: 0,
                conflict: n.5,
            });
        }
        rows
    }

    // ---- mutations (each rescans the current notebook) ----

    pub fn ensure_notebook(&self) {
        if self.current.borrow().is_none() {
            if let Ok(tree) = store::create_notebook(&self.root, "My Notebook", now_ms()) {
                self.notebooks.borrow_mut().push(tree.meta.name.clone());
                *self.current.borrow_mut() = Some(tree);
            }
        }
    }

    /// Rename the current notebook. Returns (old_dir, new_dir) on success.
    pub fn rename_current_notebook(&self, new_name: &str) -> Option<(PathBuf, PathBuf)> {
        let old_name = self.current.borrow().as_ref()?.meta.name.clone();
        if old_name == new_name || new_name.trim().is_empty() {
            return None;
        }
        let old_dir = self.current.borrow().as_ref()?.dir.clone();
        let new_dir = store::rename_notebook(&self.root, &old_name, new_name).ok()?;
        {
            let mut names = self.notebooks.borrow_mut();
            if let Some(n) = names.iter_mut().find(|n| **n == old_name) {
                *n = new_name.to_string();
            }
        }
        if let Ok(Some(tree)) = store::scan_notebook(&new_dir) {
            *self.current.borrow_mut() = Some(tree);
        }
        Some((old_dir, new_dir))
    }

    /// Trash the current notebook. Returns its old dir; selects another
    /// notebook when one exists.
    pub fn delete_current_notebook(&self) -> Option<PathBuf> {
        let name = self.current.borrow().as_ref()?.meta.name.clone();
        let old_dir = self.current.borrow().as_ref()?.dir.clone();
        store::delete_notebook(&self.root, &name).ok()?;
        self.notebooks.borrow_mut().retain(|n| *n != name);
        let next = self.notebooks.borrow().first().cloned();
        *self.current.borrow_mut() = None;
        if let Some(next) = next {
            self.select_notebook(&next);
        }
        Some(old_dir)
    }

    pub fn create_notebook(&self, name: &str) {
        if let Ok(tree) = store::create_notebook(&self.root, name, now_ms()) {
            self.notebooks.borrow_mut().push(tree.meta.name.clone());
            *self.current.borrow_mut() = Some(tree);
        }
    }

    pub fn new_note(&self, folder: Option<FolderId>) -> Option<(NoteId, PathBuf)> {
        self.ensure_notebook();
        let keys: Vec<String> = self.notes_in_folder(folder, None).into_iter().map(|(_, k)| k).collect();
        let key = omaink_core::key_after_last(&keys);
        let result = {
            let guard = self.current.borrow();
            let nb = guard.as_ref()?;
            store::create_note_with_key(nb, "Untitled", folder, key).ok()
        };
        self.rescan_current();
        result.map(|(doc, path)| (doc.id, path))
    }

    pub fn new_folder(&self, parent: Option<FolderId>) {
        self.ensure_notebook();
        {
            let guard = self.current.borrow();
            if let Some(nb) = guard.as_ref() {
                let _ = store::create_folder(nb, "New folder", parent);
            }
        }
        self.rescan_current();
    }

    pub fn rename_note_on_disk(&self, path: &std::path::Path, title: &str) {
        let _ = store::rename_note(path, title);
        self.rescan_current();
    }

    pub fn rename_folder(&self, id: FolderId, title: &str) {
        let (dir, meta) = {
            let guard = self.current.borrow();
            let Some(nb) = guard.as_ref() else { return };
            (nb.dir.clone(), nb.folders.iter().find(|f| f.id == id).cloned())
        };
        if let Some(meta) = meta {
            let _ = store::rename_folder(&dir, meta, title);
        }
        self.rescan_current();
    }

    pub fn delete_note(&self, id: NoteId) {
        let dir = self.current.borrow().as_ref().map(|n| n.dir.clone());
        if let Some(dir) = dir {
            let _ = store::delete_note(&dir, id);
        }
        self.rescan_current();
    }

    pub fn delete_folder(&self, id: FolderId) {
        {
            let guard = self.current.borrow();
            if let Some(nb) = guard.as_ref() {
                let _ = store::delete_folder(nb, id);
            }
        }
        self.rescan_current();
    }

    // ---- drag-and-drop moves ----

    fn notebook_dir(&self) -> Option<PathBuf> {
        self.current.borrow().as_ref().map(|n| n.dir.clone())
    }

    /// (path, order_key) of notes in `folder`, sorted, optionally excluding one.
    fn notes_in_folder(&self, folder: Option<FolderId>, exclude: Option<&Path>) -> Vec<(PathBuf, String)> {
        let paths: Vec<PathBuf> = match self.current.borrow().as_ref() {
            Some(nb) => nb.notes.iter().map(|e| e.path.clone()).collect(),
            None => return Vec::new(),
        };
        let mut v: Vec<(PathBuf, String)> = paths
            .into_iter()
            .filter(|p| exclude != Some(p.as_path()))
            .filter_map(|p| {
                let m = self.note_meta(&p)?;
                (m.folder == folder).then(|| (p, m.order_key))
            })
            .collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    }

    /// Drop a note into a folder (or top level with `None`), appended at the end.
    pub fn drop_note_into_folder(&self, src: &Path, folder: Option<FolderId>) {
        let siblings = self.notes_in_folder(folder, Some(src));
        let last = siblings.last().map(|(_, k)| k.clone());
        let key = store::order_between(last.as_deref(), None);
        let _ = store::set_note_order(src, key, folder);
        self.rescan_current();
    }

    /// Drop `src` just before/after `target` (a note), landing in target's folder.
    pub fn drop_note_near_note(&self, src: &Path, target: &Path, after: bool) {
        if src == target {
            return;
        }
        let Some(tdoc) = self.note_meta(target) else { return };
        let folder = tdoc.folder;
        let siblings = self.notes_in_folder(folder, Some(src));
        let Some(idx) = siblings.iter().position(|(p, _)| p == target) else { return };
        let (before, after_k) = if after {
            (Some(siblings[idx].1.as_str()), siblings.get(idx + 1).map(|(_, k)| k.as_str()))
        } else {
            let lo = if idx > 0 { Some(siblings[idx - 1].1.as_str()) } else { None };
            (lo, Some(siblings[idx].1.as_str()))
        };
        let key = store::order_between(before, after_k);
        let _ = store::set_note_order(src, key, folder);
        self.rescan_current();
    }

    fn folders_with_parent(&self, parent: Option<FolderId>, exclude: Option<FolderId>) -> Vec<FolderMeta> {
        let guard = self.current.borrow();
        let Some(nb) = guard.as_ref() else { return Vec::new() };
        let mut v: Vec<FolderMeta> = nb
            .folders
            .iter()
            .filter(|f| f.parent == parent && Some(f.id) != exclude)
            .cloned()
            .collect();
        v.sort_by(|a, b| a.order_key.cmp(&b.order_key).then(a.id.cmp(&b.id)));
        v
    }

    fn is_descendant(&self, maybe_child: FolderId, ancestor: FolderId) -> bool {
        let guard = self.current.borrow();
        let Some(nb) = guard.as_ref() else { return false };
        let mut cur = Some(maybe_child);
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            cur = nb.folders.iter().find(|f| f.id == id).and_then(|f| f.parent);
        }
        false
    }

    /// Nest `src` folder inside `target`, appended at the end. No-ops on cycles.
    /// (Currently unused: sub-folders are disabled in the UI by product choice.)
    #[allow(dead_code)]
    pub fn drop_folder_into_folder(&self, src: FolderId, target: FolderId) {
        if src == target || self.is_descendant(target, src) {
            return;
        }
        let Some(dir) = self.notebook_dir() else { return };
        let siblings = self.folders_with_parent(Some(target), Some(src));
        let last = siblings.last().map(|f| f.order_key.clone());
        let key = store::order_between(last.as_deref(), None);
        let meta = self.current.borrow().as_ref().and_then(|nb| nb.folders.iter().find(|f| f.id == src).cloned());
        if let Some(meta) = meta {
            let _ = store::set_folder_order(&dir, meta, key, Some(target));
        }
        self.rescan_current();
    }

    /// Reorder `src` folder just before/after `target` within target's parent.
    pub fn drop_folder_near_folder(&self, src: FolderId, target: FolderId, after: bool) {
        if src == target || self.is_descendant(target, src) {
            return;
        }
        let Some(dir) = self.notebook_dir() else { return };
        let parent = self
            .current
            .borrow()
            .as_ref()
            .and_then(|nb| nb.folders.iter().find(|f| f.id == target).map(|f| f.parent))
            .flatten();
        let siblings = self.folders_with_parent(parent, Some(src));
        let Some(idx) = siblings.iter().position(|f| f.id == target) else { return };
        let (before, after_k) = if after {
            (Some(siblings[idx].order_key.as_str()), siblings.get(idx + 1).map(|f| f.order_key.as_str()))
        } else {
            let lo = if idx > 0 { Some(siblings[idx - 1].order_key.as_str()) } else { None };
            (lo, Some(siblings[idx].order_key.as_str()))
        };
        let key = store::order_between(before, after_k);
        let meta = self.current.borrow().as_ref().and_then(|nb| nb.folders.iter().find(|f| f.id == src).cloned());
        if let Some(meta) = meta {
            let _ = store::set_folder_order(&dir, meta, key, parent);
        }
        self.rescan_current();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
