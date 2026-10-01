//! Ink stroke data. Pure data here (so commands can own strokes); the
//! geometry algorithms that consume these live in `omascratch-ink`.

use serde::{Deserialize, Serialize};

use crate::SemanticColor;

/// One raw input sample from the stylus (or a synthesized mouse sample).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InkPoint {
    pub x: f64,
    pub y: f64,
    /// 0.0..=1.0; mouse input synthesizes 0.5.
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// Milliseconds since the stroke's first sample (Ink Replay source data).
    pub dt_ms: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Pen,
    Pencil,
    Highlighter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StrokeId(pub uuid::Uuid);

impl StrokeId {
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for StrokeId {
    fn default() -> Self {
        Self::new()
    }
}

/// A committed ink stroke. Raw samples are kept forever; outlines are
/// recomputed from them, so rendering can improve without file migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub id: StrokeId,
    pub tool: Tool,
    pub color: SemanticColor,
    /// Base width (diameter) in world units at 100% zoom.
    pub width: f64,
    /// Wall-clock start, milliseconds since Unix epoch (Ink Replay ordering).
    pub t0_ms: u64,
    pub points: Vec<InkPoint>,
}

impl Stroke {
    /// Axis-aligned bounds of the raw samples, padded by the stroke width.
    /// Used for culling and spatial indexing; cheap and conservative.
    pub fn bounds(&self) -> Option<kurbo::Rect> {
        let first = self.points.first()?;
        let mut r = kurbo::Rect::new(first.x, first.y, first.x, first.y);
        for p in &self.points[1..] {
            r = r.union_pt(kurbo::Point::new(p.x, p.y));
        }
        Some(r.inflate(self.width, self.width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_cover_all_points_padded_by_width() {
        let s = Stroke {
            id: StrokeId::new(),
            tool: Tool::Pen,
            color: SemanticColor::Foreground,
            width: 2.0,
            t0_ms: 0,
            points: vec![
                InkPoint { x: 0.0, y: 0.0, pressure: 0.5, tilt_x: 0.0, tilt_y: 0.0, dt_ms: 0 },
                InkPoint { x: 10.0, y: -5.0, pressure: 0.5, tilt_x: 0.0, tilt_y: 0.0, dt_ms: 8 },
            ],
        };
        let b = s.bounds().unwrap();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (-2.0, -7.0, 12.0, 2.0));
    }

    #[test]
    fn empty_stroke_has_no_bounds() {
        let s = Stroke {
            id: StrokeId::new(),
            tool: Tool::Pen,
            color: SemanticColor::Foreground,
            width: 2.0,
            t0_ms: 0,
            points: vec![],
        };
        assert!(s.bounds().is_none());
    }
}
