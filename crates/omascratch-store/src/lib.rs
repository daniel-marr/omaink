//! Persistence. On-disk schema structs are kept separate from the domain
//! model (explicit from_disk/to_disk mapping) so schema evolution never
//! contorts the domain. All writes are atomic: same-dir temp file + fsync +
//! rename + parent-dir fsync.
//!
//! This crate performs blocking I/O; the GTK shell must call it from worker
//! threads (`gio::spawn_blocking`), never the UI thread.

pub mod atomic;
pub mod error;
pub mod note_file;
pub mod notebook;
pub mod schema;
pub mod xdg;

pub use error::{Result, StoreError};
pub use note_file::{read_note, write_note, NOTE_EXT};
pub use notebook::{
    create_notebook, is_conflict_name, note_path, scan_notebook, scan_root, FolderMeta,
    NotebookMeta, NotebookTree, NoteEntry,
};
pub use schema::{NoteDoc, NOTE_SCHEMA};
pub use xdg::{cache_dir, config_dir, default_notebooks_root, state_dir, Settings};
