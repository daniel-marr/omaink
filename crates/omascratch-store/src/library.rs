//! Library operations: create/rename/delete/reorder of notebooks, folders and
//! notes. Every mutation is a single atomic file write to the item's own file
//! (or a move into `.trash`), so concurrent edits on different items never
//! collide and no central index can go stale.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use omascratch_core::{key_after_last, key_between, FolderId, NoteId};

use crate::atomic::atomic_write;
use crate::error::{Result, StoreError};
use crate::note_file::{read_note, write_note, NOTE_EXT};
use crate::notebook::{scan_notebook, FolderMeta, NotebookTree};
use crate::schema::NoteDoc;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| StoreError::corrupt(path, e.to_string()))?;
    atomic_write(path, &bytes)
}

fn trash_dir(notebook_dir: &Path) -> PathBuf {
    notebook_dir.join(".trash")
}

/// Move a file into the notebook's `.trash`, timestamp-prefixed so repeated
/// deletes of same-named items don't collide. Never deletes outright.
fn trash_file(notebook_dir: &Path, path: &Path) -> Result<()> {
    let trash = trash_dir(notebook_dir);
    std::fs::create_dir_all(&trash).map_err(|e| StoreError::io(&trash, e))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let dest = trash.join(format!("{}-{}", now_ms(), name));
    std::fs::rename(path, &dest).map_err(|e| StoreError::io(path, e))
}

// ---- folders ----

fn folder_path(notebook_dir: &Path, id: FolderId) -> PathBuf {
    notebook_dir.join("folders").join(id.0.to_string()).join("folder.json")
}

pub fn create_folder(
    notebook: &NotebookTree,
    title: &str,
    parent: Option<FolderId>,
) -> Result<FolderMeta> {
    let siblings: Vec<String> = notebook
        .folders
        .iter()
        .filter(|f| f.parent == parent)
        .map(|f| f.order_key.clone())
        .collect();
    let meta = FolderMeta {
        schema: 1,
        id: FolderId::new(),
        title: title.to_string(),
        parent,
        order_key: key_after_last(&siblings),
        created_ms: now_ms(),
    };
    let dir = notebook.dir.join("folders").join(meta.id.0.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| StoreError::io(&dir, e))?;
    write_json(&dir.join("folder.json"), &meta)?;
    Ok(meta)
}

pub fn rename_folder(notebook_dir: &Path, mut meta: FolderMeta, title: &str) -> Result<FolderMeta> {
    meta.title = title.to_string();
    write_json(&folder_path(notebook_dir, meta.id), &meta)?;
    Ok(meta)
}

pub fn set_folder_order(
    notebook_dir: &Path,
    mut meta: FolderMeta,
    order_key: String,
    parent: Option<FolderId>,
) -> Result<FolderMeta> {
    meta.order_key = order_key;
    meta.parent = parent;
    write_json(&folder_path(notebook_dir, meta.id), &meta)?;
    Ok(meta)
}

/// Trash a folder. Its direct child notes move to the folder's parent level
/// (no data loss); child folders are reparented the same way.
pub fn delete_folder(notebook: &NotebookTree, id: FolderId) -> Result<()> {
    let Some(target) = notebook.folders.iter().find(|f| f.id == id) else {
        return Ok(());
    };
    let new_parent = target.parent;

    // Reparent child folders.
    for child in notebook.folders.iter().filter(|f| f.parent == Some(id)) {
        let mut m = child.clone();
        m.parent = new_parent;
        write_json(&folder_path(&notebook.dir, m.id), &m)?;
    }
    // Reparent notes currently in this folder.
    for entry in &notebook.notes {
        if let Some(nid) = entry.id {
            if let Ok(mut doc) = read_note(&entry.path) {
                if doc.folder == Some(id) {
                    doc.folder = new_parent;
                    write_note(&entry.path, &doc, now_ms())?;
                }
                let _ = nid;
            }
        }
    }
    // Trash the folder dir itself.
    let dir = notebook.dir.join("folders").join(id.0.to_string());
    if dir.exists() {
        trash_file(&notebook.dir, &dir)?;
    }
    Ok(())
}

// ---- notes ----

fn note_path(notebook_dir: &Path, id: NoteId) -> PathBuf {
    notebook_dir.join("notes").join(format!("{}.{NOTE_EXT}", id.0))
}

/// Order keys of all notes currently in `folder`, read from disk.
fn sibling_note_keys(notebook_dir: &Path, folder: Option<FolderId>) -> Vec<String> {
    let mut keys = Vec::new();
    let notes_dir = notebook_dir.join("notes");
    let Ok(entries) = std::fs::read_dir(&notes_dir) else { return keys };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(NOTE_EXT) {
            continue;
        }
        if let Ok(doc) = read_note(&path) {
            if doc.folder == folder {
                keys.push(doc.order_key);
            }
        }
    }
    keys
}

pub fn create_note(
    notebook: &NotebookTree,
    title: &str,
    folder: Option<FolderId>,
) -> Result<(NoteDoc, PathBuf)> {
    // Order among notes in the same folder. Read sibling order keys straight
    // from disk so the result is correct even if `notebook` is a stale scan.
    let siblings = sibling_note_keys(&notebook.dir, folder);
    let doc = NoteDoc {
        id: NoteId::new(),
        title: title.to_string(),
        folder,
        order_key: key_after_last(&siblings),
        created_ms: now_ms(),
        modified_ms: now_ms(),
        content: omascratch_core::NoteContent::default(),
        opaque_elements: vec![],
    };
    let path = note_path(&notebook.dir, doc.id);
    write_note(&path, &doc, doc.modified_ms)?;
    Ok((doc, path))
}

/// Rename a note on disk. For the note currently open in the canvas, prefer
/// renaming through the storage adapter's in-memory doc instead, to avoid
/// racing autosave.
pub fn rename_note(path: &Path, title: &str) -> Result<NoteDoc> {
    let mut doc = read_note(path)?;
    doc.title = title.to_string();
    write_note(path, &doc, now_ms())?;
    Ok(doc)
}

pub fn set_note_order(
    path: &Path,
    order_key: String,
    folder: Option<FolderId>,
) -> Result<NoteDoc> {
    let mut doc = read_note(path)?;
    doc.order_key = order_key;
    doc.folder = folder;
    write_note(path, &doc, now_ms())?;
    Ok(doc)
}

pub fn delete_note(notebook_dir: &Path, id: NoteId) -> Result<()> {
    let path = note_path(notebook_dir, id);
    // Trash the note and its assets sidecar together.
    let assets = notebook_dir.join("notes").join(format!("{}.assets", id.0));
    if path.exists() {
        trash_file(notebook_dir, &path)?;
    }
    if assets.exists() {
        trash_file(notebook_dir, &assets)?;
    }
    Ok(())
}

/// Compute an order key placing an item between the items currently at
/// `before_key` and `after_key` (either side may be absent for the ends).
pub fn order_between(before_key: Option<&str>, after_key: Option<&str>) -> String {
    key_between(before_key, after_key)
}

/// Rescan a notebook after a mutation so callers can refresh their model.
pub fn reload_notebook(dir: &Path) -> Result<Option<NotebookTree>> {
    scan_notebook(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::create_notebook;

    fn nb() -> (tempfile::TempDir, NotebookTree) {
        let root = tempfile::tempdir().unwrap();
        let tree = create_notebook(root.path(), "NB", 1).unwrap();
        (root, tree)
    }

    #[test]
    fn create_folders_and_notes_then_rescan() {
        let (_root, mut tree) = nb();
        let f = create_folder(&tree, "Work", None).unwrap();
        tree = reload_notebook(&tree.dir).unwrap().unwrap();
        assert_eq!(tree.folders.len(), 1);

        let (_doc, _p) = create_note(&tree, "Todo", Some(f.id)).unwrap();
        let (_doc2, _p2) = create_note(&tree, "Ideas", Some(f.id)).unwrap();
        tree = reload_notebook(&tree.dir).unwrap().unwrap();
        assert_eq!(tree.notes.len(), 2);
    }

    #[test]
    fn notes_in_a_folder_get_ascending_order_keys() {
        let (_root, mut tree) = nb();
        let f = create_folder(&tree, "F", None).unwrap();
        let (a, _) = create_note(&tree, "A", Some(f.id)).unwrap();
        let (b, _) = create_note(&tree, "B", Some(f.id)).unwrap();
        let (c, _) = create_note(&tree, "C", Some(f.id)).unwrap();
        assert!(a.order_key < b.order_key && b.order_key < c.order_key);
    }

    #[test]
    fn rename_note_persists_title_only() {
        let (_root, tree) = nb();
        let (doc, path) = create_note(&tree, "Old", None).unwrap();
        let renamed = rename_note(&path, "New").unwrap();
        assert_eq!(renamed.id, doc.id);
        assert_eq!(renamed.title, "New");
        assert_eq!(read_note(&path).unwrap().title, "New");
    }

    #[test]
    fn delete_note_moves_to_trash_not_oblivion() {
        let (_root, mut tree) = nb();
        let (doc, path) = create_note(&tree, "Doomed", None).unwrap();
        assert!(path.exists());
        delete_note(&tree.dir, doc.id).unwrap();
        assert!(!path.exists());
        tree = reload_notebook(&tree.dir).unwrap().unwrap();
        assert_eq!(tree.notes.len(), 0);
        let trashed: Vec<_> = std::fs::read_dir(tree.dir.join(".trash")).unwrap().collect();
        assert_eq!(trashed.len(), 1, "note must be recoverable from .trash");
    }

    #[test]
    fn delete_folder_reparents_notes_no_data_loss() {
        let (_root, mut tree) = nb();
        let parent = create_folder(&tree, "Parent", None).unwrap();
        tree = reload_notebook(&tree.dir).unwrap().unwrap();
        let (doc, path) = create_note(&tree, "Child", Some(parent.id)).unwrap();
        tree = reload_notebook(&tree.dir).unwrap().unwrap();

        delete_folder(&tree, parent.id).unwrap();
        tree = reload_notebook(&tree.dir).unwrap().unwrap();
        assert_eq!(tree.folders.len(), 0);
        // The note still exists, now at top level.
        assert!(path.exists());
        let reloaded = read_note(&path).unwrap();
        assert_eq!(reloaded.id, doc.id);
        assert_eq!(reloaded.folder, None);
    }

    #[test]
    fn reorder_between_two_notes_touches_only_the_moved_note() {
        let (_root, mut tree) = nb();
        let (a, pa) = create_note(&tree, "A", None).unwrap();
        let (b, _pb) = create_note(&tree, "B", None).unwrap();
        let (_c, pc) = create_note(&tree, "C", None).unwrap();
        // Move C between A and B. Only C's file is written.
        let before = std::fs::metadata(&pa).unwrap().modified().unwrap();
        let key = order_between(Some(&a.order_key), Some(&b.order_key));
        let moved = set_note_order(&pc, key, None).unwrap();
        assert!(a.order_key < moved.order_key && moved.order_key < b.order_key);
        // A's file untouched.
        assert_eq!(std::fs::metadata(&pa).unwrap().modified().unwrap(), before);
    }
}
