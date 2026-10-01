//! Note content: the elements on one note's infinite canvas.
//! M1 scope: strokes only. Images, shapes and text boxes join in M4/M6.

use serde::{Deserialize, Serialize};

use crate::stroke::{Stroke, StrokeId};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoteContent {
    pub strokes: Vec<Stroke>,
}

impl NoteContent {
    pub fn stroke_index(&self, id: StrokeId) -> Option<usize> {
        self.strokes.iter().position(|s| s.id == id)
    }
}
