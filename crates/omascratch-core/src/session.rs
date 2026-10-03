//! Application/session boundary: the one open note, command dispatch and
//! undo/redo. Owns authoritative content; the UI holds only transient state.

use crate::note::{ImageItem, NoteContent};
use crate::text::TextBox;
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
    /// Exact before/after replacement of strokes and images, matched by id:
    /// same id = transformed in place (z-order kept), id only in `before` =
    /// removed, id only in `after` = added. Covers move, resize, pin toggle,
    /// image add/delete. Undo swaps before/after, so it is lossless.
    Replace {
        strokes_before: Vec<Stroke>,
        strokes_after: Vec<Stroke>,
        images_before: Vec<ImageItem>,
        images_after: Vec<ImageItem>,
    },
    /// Same exact before/after semantics, for text boxes.
    ReplaceTexts { before: Vec<TextBox>, after: Vec<TextBox> },
    /// Several commands as one undo step (applied in order, reverted in
    /// reverse) — e.g. moving a selection that mixes ink, images and text.
    Batch(Vec<Command>),
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
            Command::Replace { strokes_before, strokes_after, images_before, images_after } => {
                self.replace(strokes_before, strokes_after, images_before, images_after)
            }
            Command::ReplaceTexts { before, after } => self.replace_texts(before, after),
            Command::Batch(cmds) => {
                for c in cmds {
                    self.apply(c);
                }
            }
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
            Command::Replace { strokes_before, strokes_after, images_before, images_after } => {
                self.replace(strokes_after, strokes_before, images_after, images_before)
            }
            Command::ReplaceTexts { before, after } => self.replace_texts(after, before),
            Command::Batch(cmds) => {
                for c in cmds.iter().rev() {
                    self.revert(c);
                }
            }
        }
    }

    fn replace_texts(&mut self, from: &[TextBox], to: &[TextBox]) {
        for b in from {
            if let Some(idx) = self.content.text_index(b.id) {
                match to.iter().find(|a| a.id == b.id) {
                    Some(a) => self.content.texts[idx] = a.clone(),
                    None => {
                        self.content.texts.remove(idx);
                    }
                }
            }
        }
        for a in to {
            if !from.iter().any(|b| b.id == a.id) {
                self.content.texts.push(a.clone());
            }
        }
    }

    fn replace(
        &mut self,
        s_from: &[Stroke],
        s_to: &[Stroke],
        i_from: &[ImageItem],
        i_to: &[ImageItem],
    ) {
        for b in s_from {
            if let Some(idx) = self.content.stroke_index(b.id) {
                match s_to.iter().find(|a| a.id == b.id) {
                    Some(a) => self.content.strokes[idx] = a.clone(),
                    None => {
                        self.content.strokes.remove(idx);
                    }
                }
            }
        }
        for a in s_to {
            if !s_from.iter().any(|b| b.id == a.id) {
                self.content.strokes.push(a.clone());
            }
        }
        for b in i_from {
            if let Some(idx) = self.content.image_index(b.id) {
                match i_to.iter().find(|a| a.id == b.id) {
                    Some(a) => self.content.images[idx] = a.clone(),
                    None => {
                        self.content.images.remove(idx);
                    }
                }
            }
        }
        for a in i_to {
            if !i_from.iter().any(|b| b.id == a.id) {
                self.content.images.push(a.clone());
            }
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

    fn image(x: f64) -> ImageItem {
        ImageItem {
            id: crate::id::ImageId::new(),
            asset: "img-test.png".into(),
            x,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            pinned: false,
        }
    }

    #[test]
    fn replace_add_transform_delete_all_undo_exactly() {
        let mut s = NoteSession::default();
        let a = stroke();
        let img = image(0.0);
        // Add an image + stroke in one step.
        s.dispatch(Command::Replace {
            strokes_before: vec![],
            strokes_after: vec![a.clone()],
            images_before: vec![],
            images_after: vec![img.clone()],
        });
        assert_eq!((s.content.strokes.len(), s.content.images.len()), (1, 1));

        // Transform in place (move image, pin it).
        let mut moved = img.clone();
        moved.x = 50.0;
        moved.pinned = true;
        s.dispatch(Command::Replace {
            strokes_before: vec![],
            strokes_after: vec![],
            images_before: vec![img.clone()],
            images_after: vec![moved.clone()],
        });
        assert_eq!(s.content.images[0], moved);

        // Delete both.
        s.dispatch(Command::Replace {
            strokes_before: vec![a.clone()],
            strokes_after: vec![],
            images_before: vec![moved.clone()],
            images_after: vec![],
        });
        assert!(s.content.strokes.is_empty() && s.content.images.is_empty());

        s.undo();
        assert_eq!(s.content.images[0], moved);
        s.undo();
        assert_eq!(s.content.images[0], img);
        s.undo();
        assert!(s.content.strokes.is_empty() && s.content.images.is_empty());
    }

    #[test]
    fn replace_keeps_z_order_for_transforms() {
        let mut s = NoteSession::default();
        let (a, b, c) = (stroke(), stroke(), stroke());
        s.dispatch(Command::AddStrokes(vec![a.clone(), b.clone(), c.clone()]));
        let mut b2 = b.clone();
        b2.width = 9.0;
        s.dispatch(Command::Replace {
            strokes_before: vec![b.clone()],
            strokes_after: vec![b2.clone()],
            images_before: vec![],
            images_after: vec![],
        });
        assert_eq!(s.content.strokes[1], b2, "transformed stroke stays in place");
    }

    #[test]
    fn batch_with_texts_is_one_exact_undo_step() {
        use crate::text::{ParaKind, Paragraph, Span, TextBox};
        let mut s = NoteSession::default();
        let tb = TextBox {
            id: crate::id::TextId::new(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 20.0,
            font_size: 16.0,
            color: crate::SemanticColor::Foreground,
            paras: vec![Paragraph { kind: ParaKind::Body, spans: vec![Span { text: "hi".into(), ..Default::default() }] }],
        };
        s.dispatch(Command::ReplaceTexts { before: vec![], after: vec![tb.clone()] });
        let a = stroke();
        let mut moved = tb.clone();
        moved.x = 40.0;
        s.dispatch(Command::Batch(vec![
            Command::AddStroke(a.clone()),
            Command::ReplaceTexts { before: vec![tb.clone()], after: vec![moved.clone()] },
        ]));
        assert_eq!(s.content.texts[0], moved);
        assert_eq!(s.content.strokes.len(), 1);
        s.undo();
        assert_eq!(s.content.texts[0], tb);
        assert!(s.content.strokes.is_empty());
        s.redo();
        assert_eq!(s.content.texts[0], moved);
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
