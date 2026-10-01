//! Stroke engine. Raw input samples in, render-ready outlines out.
//!
//! Strokes keep their raw samples (with per-point timestamps) forever; outlines
//! are recomputed, so rendering can improve without a file-format migration and
//! Ink Replay stays possible.

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_point_roundtrips_through_serde() {
        let p = InkPoint { x: 1.5, y: -2.0, pressure: 0.7, tilt_x: 0.1, tilt_y: -0.2, dt_ms: 16 };
        let json = serde_json::to_string(&p).unwrap();
        let back: InkPoint = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}

#[cfg(test)]
mod test_deps {
    // serde_json is only used by tests in this crate for now.
}
