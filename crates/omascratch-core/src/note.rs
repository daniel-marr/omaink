//! Note content: the elements on one note's infinite canvas — ink strokes
//! and images. Text boxes join in M6.

use serde::{Deserialize, Serialize};

use crate::id::ImageId;
use crate::stroke::{Stroke, StrokeId};

/// An image placed on the canvas. `asset` names an immutable file stored
/// beside the note (the core never touches the filesystem).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageItem {
    pub id: ImageId,
    pub asset: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Pinned to the background: stays under ink and is ignored by
    /// select/lasso/eraser until unpinned.
    pub pinned: bool,
}

impl ImageItem {
    pub fn rect(&self) -> kurbo::Rect {
        kurbo::Rect::new(self.x, self.y, self.x + self.w, self.y + self.h)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoteContent {
    pub strokes: Vec<Stroke>,
    #[serde(default)]
    pub images: Vec<ImageItem>,
}

impl NoteContent {
    pub fn stroke_index(&self, id: StrokeId) -> Option<usize> {
        self.strokes.iter().position(|s| s.id == id)
    }

    pub fn image_index(&self, id: ImageId) -> Option<usize> {
        self.images.iter().position(|i| i.id == id)
    }
}
