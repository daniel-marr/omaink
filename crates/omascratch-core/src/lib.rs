//! Domain core: notebooks, folders, notes, canvas elements, commands, undo.
//!
//! This crate is deliberately free of GTK, cairo and filesystem access so the
//! same model can later sit behind an FFI boundary for a mobile shell.

pub mod color;
pub mod id;

pub use color::SemanticColor;
pub use id::{FolderId, NotebookId, NoteId};
