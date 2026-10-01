//! Application/session boundary: the one open note, command dispatch and
//! undo/redo. Owns authoritative content; the UI holds only transient state.

use crate::note::NoteContent;
use crate::stroke::{Stroke, StrokeId};

/// An undoable edit. Each variant stores exactly what `revert` needs.
#[derive(Debug, Clone)]
pub enum Command {
    AddStroke(Stroke),
    /// Several strokes as one undo step (e.g. a drawn shape with arrowheads).
    AddStrokes(Vec<Stroke>),
    /// Area/stroke eraser: removes strokes, optionally adding split fragments.
    EraseStrokes {
        removed: Vec<Stroke>,
        replacements: Vec<Stroke>,
    },
    /// Move a selection by (dx, dy) in world units.
    TranslateStrokes {
        ids: Vec<StrokeId>,
        dx: f64,
        dy: f64,
    },
}

#[derive(Debug, Default)]
pub struct NoteSession {
    pub content: NoteContent,
    undo: Vec<Command>,
    redo: Vec<Command>,
    /// Bumped on every content change; the renderer and autosave compare it.
    revision: u64,
}

impl NoteSession {
    pub fn new(content: NoteContent) -> Self {
        Self { content, ..Default::default() }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Apply a new command from the user. Clears the redo stack.
    pub fn dispatch(&mut self, cmd: Command) {
        self.apply(&cmd);
        self.undo.push(cmd);
        self.redo.clear();
        self.revision += 1;
    }

    pub fn undo(&mut self) -> bool {
        let Some(cmd) = self.undo.pop() else { return false };
        self.revert(&cmd);
        self.redo.push(cmd);
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(cmd) = self.redo.pop() else { return false };
        self.apply(&cmd);
        self.undo.push(cmd);
        self.revision += 1;
        true
    }

    fn apply(&mut self, cmd: &Command) {
        match cmd {
            Command::AddStroke(s) => self.content.strokes.push(s.clone()),
            Command::AddStrokes(strokes) => self.content.strokes.extend(strokes.iter().cloned()),
            Command::EraseStrokes { removed, replacements } => {
                let ids: Vec<StrokeId> = removed.iter().map(|s| s.id).collect();
                self.content.strokes.retain(|s| !ids.contains(&s.id));
                self.content.strokes.extend(replacements.iter().cloned());
            }
            Command::TranslateStrokes { ids, dx, dy } => self.translate(ids, *dx, *dy),
        }
    }

    fn revert(&mut self, cmd: &Command) {
        match cmd {
            Command::AddStroke(s) => {
                if let Some(i) = self.content.stroke_index(s.id) {
                    self.content.strokes.remove(i);
                }
            }
            Command::AddStrokes(strokes) => {
                let ids: Vec<StrokeId> = strokes.iter().map(|s| s.id).collect();
                self.content.strokes.retain(|s| !ids.contains(&s.id));
            }
            Command::EraseStrokes { removed, replacements } => {
                let ids: Vec<StrokeId> = replacements.iter().map(|s| s.id).collect();
                self.content.strokes.retain(|s| !ids.contains(&s.id));
                self.content.strokes.extend(removed.iter().cloned());
            }
            Command::TranslateStrokes { ids, dx, dy } => self.translate(ids, -*dx, -*dy),
        }
    }

    fn translate(&mut self, ids: &[StrokeId], dx: f64, dy: f64) {
        for s in &mut self.content.strokes {
            if ids.contains(&s.id) {
                for p in &mut s.points {
                    p.x += dx;
                    p.y += dy;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{InkPoint, Tool};
    use crate::SemanticColor;

    fn stroke() -> Stroke {
        Stroke {
            id: StrokeId::new(),
            tool: Tool::Pen,
            color: SemanticColor::Foreground,
            width: 2.0,
            t0_ms: 0,
            points: vec![InkPoint { x: 0.0, y: 0.0, pressure: 0.5, tilt_x: 0.0, tilt_y: 0.0, dt_ms: 0 }],
        }
    }

    #[test]
    fn add_undo_redo_roundtrip() {
        let mut s = NoteSession::default();
        s.dispatch(Command::AddStroke(stroke()));
        assert_eq!(s.content.strokes.len(), 1);
        assert!(s.undo());
        assert_eq!(s.content.strokes.len(), 0);
        assert!(s.redo());
        assert_eq!(s.content.strokes.len(), 1);
        assert!(!s.redo(), "redo stack must be empty after redo");
    }

    #[test]
    fn dispatch_clears_redo() {
        let mut s = NoteSession::default();
        s.dispatch(Command::AddStroke(stroke()));
        s.undo();
        s.dispatch(Command::AddStroke(stroke()));
        assert!(!s.can_redo());
        assert_eq!(s.content.strokes.len(), 1);
    }

    #[test]
    fn erase_with_replacements_undoes_exactly() {
        let mut s = NoteSession::default();
        let a = stroke();
        let a_id = a.id;
        s.dispatch(Command::AddStroke(a.clone()));
        let frag = stroke();
        s.dispatch(Command::EraseStrokes { removed: vec![a], replacements: vec![frag.clone()] });
        assert_eq!(s.content.strokes.len(), 1);
        assert_eq!(s.content.strokes[0].id, frag.id);
        s.undo();
        assert_eq!(s.content.strokes.len(), 1);
        assert_eq!(s.content.strokes[0].id, a_id);
    }

    #[test]
    fn translate_moves_and_undo_restores_exactly() {
        let mut s = NoteSession::default();
        let a = stroke();
        let id = a.id;
        let orig = a.points.clone();
        s.dispatch(Command::AddStroke(a));
        s.dispatch(Command::TranslateStrokes { ids: vec![id], dx: 10.0, dy: -4.0 });
        assert_eq!(s.content.strokes[0].points[0].x, orig[0].x + 10.0);
        assert_eq!(s.content.strokes[0].points[0].y, orig[0].y - 4.0);
        s.undo();
        assert_eq!(s.content.strokes[0].points, orig);
    }

    #[test]
    fn add_strokes_is_one_undo_step() {
        let mut s = NoteSession::default();
        s.dispatch(Command::AddStrokes(vec![stroke(), stroke()]));
        assert_eq!(s.content.strokes.len(), 2);
        s.undo();
        assert_eq!(s.content.strokes.len(), 0);
    }

    #[test]
    fn revision_increments_on_every_change() {
        let mut s = NoteSession::default();
        let r0 = s.revision();
        s.dispatch(Command::AddStroke(stroke()));
        assert!(s.revision() > r0);
        let r1 = s.revision();
        s.undo();
        assert!(s.revision() > r1);
    }
}
