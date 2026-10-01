//! Notebook directory trees: creation, scanning, conflicted-copy detection.
//!
//! Layout (sync-safe: no central index, every item's ordering lives in its
//! own file, folder nesting is by parent-reference with flat dirs on disk):
//!
//! ```text
//! <root>/<Notebook Name>/
//!   notebook.json
//!   folders/<uuid>/folder.json
//!   notes/<uuid>.omanote
//!         <uuid>.assets/
//!   .trash/
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use omascratch_core::{FolderId, NotebookId, NoteId};

use crate::atomic::{atomic_write, clean_stale_temps};
use crate::error::{Result, StoreError};
use crate::note_file::NOTE_EXT;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookMeta {
    pub schema: u32,
    pub id: NotebookId,
    pub name: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderMeta {
    pub schema: u32,
    pub id: FolderId,
    pub title: String,
    /// `None` = top level of the notebook.
    pub parent: Option<FolderId>,
    pub order_key: String,
    pub created_ms: u64,
}

/// A note as seen by a directory scan (metadata only; content is loaded on open).
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub id: Option<NoteId>,
    pub path: PathBuf,
    /// Set when the filename matches a sync tool's conflict pattern; the UI
    /// surfaces these with a badge instead of hiding them.
    pub conflict: bool,
}

#[derive(Debug)]
pub struct NotebookTree {
    pub meta: NotebookMeta,
    pub dir: PathBuf,
    pub folders: Vec<FolderMeta>,
    pub notes: Vec<NoteEntry>,
}

/// Filename patterns the common sync tools use for conflict siblings.
pub fn is_conflict_name(file_stem: &str) -> bool {
    file_stem.contains(" (conflicted copy")      // Dropbox
        || file_stem.contains(".sync-conflict-") // Syncthing
        || file_stem.contains(" (conflict")      // generic/rclone style
}

pub fn notebook_dir_paths(dir: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    (
        dir.join("notebook.json"),
        dir.join("folders"),
        dir.join("notes"),
        dir.join(".trash"),
    )
}

pub fn create_notebook(root: &Path, name: &str, now_ms: u64) -> Result<NotebookTree> {
    let dir = root.join(name);
    let (meta_path, folders_dir, notes_dir, trash_dir) = notebook_dir_paths(&dir);
    for d in [&dir, &folders_dir, &notes_dir, &trash_dir] {
        std::fs::create_dir_all(d).map_err(|e| StoreError::io(d.as_path(), e))?;
    }
    let meta = NotebookMeta {
        schema: 1,
        id: NotebookId::new(),
        name: name.to_string(),
        created_ms: now_ms,
    };
    let json = serde_json::to_vec_pretty(&meta)
        .map_err(|e| StoreError::corrupt(&meta_path, e.to_string()))?;
    atomic_write(&meta_path, &json)?;
    Ok(NotebookTree { meta, dir, folders: Vec::new(), notes: Vec::new() })
}

/// Scan every notebook under the root. Unreadable items are skipped with
/// their paths collected into `skipped`, never silently deleted.
pub fn scan_root(root: &Path) -> Result<(Vec<NotebookTree>, Vec<PathBuf>)> {
    let mut notebooks = Vec::new();
    let mut skipped = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((notebooks, skipped)),
        Err(e) => return Err(StoreError::io(root, e)),
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        match scan_notebook(&dir) {
            Ok(Some(tree)) => notebooks.push(tree),
            Ok(None) => {} // not a notebook dir; ignore
            Err(_) => skipped.push(dir),
        }
    }
    notebooks.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    Ok((notebooks, skipped))
}

pub fn scan_notebook(dir: &Path) -> Result<Option<NotebookTree>> {
    let (meta_path, folders_dir, notes_dir, _) = notebook_dir_paths(dir);
    if !meta_path.exists() {
        return Ok(None);
    }
    let meta: NotebookMeta = serde_json::from_slice(
        &std::fs::read(&meta_path).map_err(|e| StoreError::io(&meta_path, e))?,
    )
    .map_err(|e| StoreError::corrupt(&meta_path, e.to_string()))?;

    let mut folders = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&folders_dir) {
        for entry in entries.flatten() {
            let fj = entry.path().join("folder.json");
            let Ok(bytes) = std::fs::read(&fj) else { continue };
            if let Ok(f) = serde_json::from_slice::<FolderMeta>(&bytes) {
                folders.push(f);
            }
        }
    }
    folders.sort_by(|a, b| a.order_key.cmp(&b.order_key).then(a.id.cmp(&b.id)));

    let mut notes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&notes_dir) {
        clean_stale_temps(&notes_dir);
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(NOTE_EXT) {
                continue;
            }
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let conflict = is_conflict_name(&stem);
            let id = stem.parse::<uuid::Uuid>().ok().map(NoteId);
            notes.push(NoteEntry { id, path, conflict });
        }
    }
    notes.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Some(NotebookTree { meta, dir: dir.to_path_buf(), folders, notes }))
}

pub fn note_path(notebook_dir: &Path, id: NoteId) -> PathBuf {
    notebook_dir.join("notes").join(format!("{}.{NOTE_EXT}", id.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_file::{read_note, write_note};
    use crate::schema::NoteDoc;
    use omascratch_core::NoteContent;

    fn new_doc(title: &str) -> NoteDoc {
        NoteDoc {
            id: NoteId::new(),
            title: title.into(),
            folder: None,
            order_key: "a0".into(),
            created_ms: 1,
            modified_ms: 1,
            content: NoteContent::default(),
            opaque_elements: vec![],
        }
    }

    #[test]
    fn create_scan_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let nb = create_notebook(root.path(), "My Notebook", 1).unwrap();
        let doc = new_doc("First");
        write_note(&note_path(&nb.dir, doc.id), &doc, 1).unwrap();

        let (found, skipped) = scan_root(root.path()).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].meta.name, "My Notebook");
        assert_eq!(found[0].notes.len(), 1);
        assert_eq!(found[0].notes[0].id, Some(doc.id));
        assert!(!found[0].notes[0].conflict);

        let loaded = read_note(&found[0].notes[0].path).unwrap();
        assert_eq!(loaded.title, "First");
    }

    #[test]
    fn conflicted_copies_are_surfaced_not_hidden() {
        let root = tempfile::tempdir().unwrap();
        let nb = create_notebook(root.path(), "NB", 1).unwrap();
        let doc = new_doc("Orig");
        write_note(&note_path(&nb.dir, doc.id), &doc, 1).unwrap();
        let conflict_path = nb
            .dir
            .join("notes")
            .join(format!("{} (conflicted copy 2026-10-01).{NOTE_EXT}", doc.id.0));
        write_note(&conflict_path, &doc, 2).unwrap();

        let tree = scan_notebook(&nb.dir).unwrap().unwrap();
        assert_eq!(tree.notes.len(), 2);
        let conflicts: Vec<_> = tree.notes.iter().filter(|n| n.conflict).collect();
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn two_machine_merge_by_file_copy_is_safe() {
        // Machine A and machine B edit different notes of the same notebook;
        // merging = copying B's note file into A's tree. Nothing corrupts.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let nb_a = create_notebook(a.path(), "NB", 1).unwrap();
        let nb_b = create_notebook(b.path(), "NB", 1).unwrap();
        let doc_a = new_doc("From A");
        let doc_b = new_doc("From B");
        write_note(&note_path(&nb_a.dir, doc_a.id), &doc_a, 1).unwrap();
        write_note(&note_path(&nb_b.dir, doc_b.id), &doc_b, 1).unwrap();

        std::fs::copy(
            note_path(&nb_b.dir, doc_b.id),
            note_path(&nb_a.dir, doc_b.id),
        )
        .unwrap();

        let tree = scan_notebook(&nb_a.dir).unwrap().unwrap();
        assert_eq!(tree.notes.len(), 2);
        for n in &tree.notes {
            read_note(&n.path).unwrap();
        }
    }
}
