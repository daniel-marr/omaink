//! Persistence. On-disk schema structs are kept separate from the domain
//! model (explicit from_disk/to_disk mapping) so schema evolution never
//! contorts the domain. All writes are atomic: same-dir temp file + fsync +
//! rename + parent-dir fsync.
//!
//! This crate performs blocking I/O; the GTK shell must call it from worker
//! threads (`gio::spawn_blocking`), never the UI thread.

pub mod atomic;
pub mod error;
pub mod library;
pub mod note_file;
pub mod notebook;
pub mod schema;
pub mod xdg;

pub use error::{Result, StoreError};
pub use note_file::{assets_dir, read_note, read_note_meta, write_asset, write_note, NoteMeta, NOTE_EXT};
pub use notebook::{
    create_notebook, is_conflict_name, note_path, scan_notebook, scan_root, FolderMeta,
    NotebookMeta, NotebookTree, NoteEntry,
};
pub use library::{
    create_folder, create_note, create_note_with_key, set_new_note_background, delete_folder, delete_note, delete_notebook, order_between,
    reload_notebook, rename_folder, rename_note, rename_notebook, set_folder_order, set_note_order,
};
pub use schema::{NoteDoc, NOTE_SCHEMA};
pub use xdg::{cache_dir, config_dir, default_notebooks_root, migrate_legacy_dirs, state_dir, validate_notebooks_root, load_last_note, save_last_note, AppearanceSettings, GeneralSettings, InkSettings, OnLaunch, PageColor, PageDefaults, PressureCurve, Settings, SideButton, Smoothing, ViewState};
