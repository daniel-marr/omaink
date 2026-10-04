//! Stroke engine. Raw input samples in, render-ready outlines out.
//!
//! Data types (`InkPoint`, `Stroke`, `Tool`) live in `omaink-core`; this
//! crate holds the geometry: smoothing/outlines (perfect-freehand via
//! `freedraw`), and later hit-testing, eraser splitting and spatial indexing.

pub mod hit;
pub mod outline;
pub mod shapes;

pub use hit::{distance_to_stroke, erase_samples, point_in_polygon, stroke_hit, stroke_inside_polygon, stroke_inside_rect, stroke_touched};
pub use shapes::{shape_polylines, ShapeKind};
pub use outline::{outline_points, outline_to_bezpath, set_smoothing_passes, stroke_bezpath, stroke_path};
