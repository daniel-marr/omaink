//! Stroke engine. Raw input samples in, render-ready outlines out.
//!
//! Data types (`InkPoint`, `Stroke`, `Tool`) live in `omascratch-core`; this
//! crate holds the geometry: smoothing/outlines (perfect-freehand via
//! `freedraw`), and later hit-testing, eraser splitting and spatial indexing.

pub mod outline;

pub use outline::{outline_points, outline_to_bezpath, stroke_bezpath};
