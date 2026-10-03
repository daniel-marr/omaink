//! Domain core: notebooks, folders, notes, canvas elements, commands, undo.
//!
//! This crate is deliberately free of GTK, cairo and filesystem access so the
//! same model can later sit behind an FFI boundary for a mobile shell.

pub mod background;
pub mod color;
pub mod id;
pub mod note;
pub mod order;
pub mod session;
pub mod stroke;
pub mod text;

pub use background::{BackgroundKind, PageBackground};
pub use color::{Rgba, SemanticColor};
pub use id::{FolderId, ImageId, NotebookId, NoteId, TextId};
pub use note::{ImageItem, NoteContent};
pub use order::{key_after_last, key_between};
pub use session::{Command, NoteSession};
pub use stroke::{InkPoint, Stroke, StrokeId, Tool};
pub use text::{ParaKind, Paragraph, Span, TextBox};
