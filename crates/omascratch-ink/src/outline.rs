//! Raw samples → render-ready outline, via the perfect-freehand algorithm
//! (`freedraw`). The outline polygon is smoothed into a closed quadratic
//! Bézier path (midpoint smoothing, as upstream perfect-freehand renders it).

use freedraw::{get_stroke, InputPoint, StrokeOptions, TaperOptions};
use kurbo::BezPath;
use omascratch_core::{InkPoint, Tool};

/// Tuning per tool. Values follow perfect-freehand's defaults, adjusted so a
/// pen tapers slightly at the end and a highlighter stays constant-width.
fn options_for(tool: Tool, width: f64, simulate_pressure: bool, last: bool) -> StrokeOptions {
    let mut o = StrokeOptions {
        size: Some(width),
        last: Some(last),
        simulate_pressure: Some(simulate_pressure),
        ..Default::default()
    };
    match tool {
        Tool::Pen => {
            o.thinning = Some(0.55);
            o.smoothing = Some(0.5);
            o.streamline = Some(0.45);
            o.end = Some(TaperOptions { cap: Some(true), ..Default::default() });
        }
        Tool::Pencil => {
            o.thinning = Some(0.35);
            o.smoothing = Some(0.4);
            o.streamline = Some(0.35);
        }
        Tool::Highlighter => {
            // Constant width: pressure must not thin a highlighter, and the
            // ends are flat chisel cuts, not round blobs.
            o.thinning = Some(0.0);
            o.smoothing = Some(0.6);
            o.streamline = Some(0.5);
            o.start = Some(TaperOptions { cap: Some(false), ..Default::default() });
            o.end = Some(TaperOptions { cap: Some(false), ..Default::default() });
        }
        Tool::Shape => {
            // Shapes normally bypass the freehand outline entirely (they are
            // stroked as paths); keep sane constants for any caller that asks.
            o.thinning = Some(0.0);
            o.smoothing = Some(0.0);
            o.streamline = Some(0.0);
        }
    }
    o
}

/// Compute the outline polygon for a stroke (or a live prefix of one).
///
/// `simulate_pressure` is for mouse input, where every sample reports the
/// same synthetic pressure and velocity-based simulation looks better.
pub fn outline_points(
    points: &[InkPoint],
    tool: Tool,
    width: f64,
    simulate_pressure: bool,
    last: bool,
) -> Vec<[f64; 2]> {
    let input: Vec<InputPoint> = points
        .iter()
        .map(|p| InputPoint::Array([p.x, p.y], Some(p.pressure as f64)))
        .collect();
    get_stroke(&input, &options_for(tool, width, simulate_pressure, last))
}

/// Midpoint-smooth a closed outline polygon into a quadratic Bézier path.
/// This is how perfect-freehand's reference renderer draws its outlines.
pub fn outline_to_bezpath(outline: &[[f64; 2]]) -> BezPath {
    let mut path = BezPath::new();
    if outline.len() < 2 {
        return path;
    }
    if outline.len() == 2 {
        path.move_to((outline[0][0], outline[0][1]));
        path.line_to((outline[1][0], outline[1][1]));
        return path;
    }
    let mid = |a: [f64; 2], b: [f64; 2]| ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
    path.move_to(mid(outline[0], outline[1]));
    for i in 1..outline.len() {
        let next = outline[(i + 1) % outline.len()];
        path.quad_to((outline[i][0], outline[i][1]), mid(outline[i], next));
    }
    path.close_path();
    path
}

/// Convenience: full pipeline for a committed stroke.
pub fn stroke_bezpath(points: &[InkPoint], tool: Tool, width: f64, simulate_pressure: bool) -> BezPath {
    outline_to_bezpath(&outline_points(points, tool, width, simulate_pressure, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn pts(n: usize) -> Vec<InkPoint> {
        (0..n)
            .map(|i| InkPoint {
                x: i as f64 * 3.0,
                y: (i as f64 * 0.7).sin() * 10.0,
                pressure: 0.3 + 0.4 * (i as f32 / n as f32),
                tilt_x: 0.0,
                tilt_y: 0.0,
                dt_ms: 8,
            })
            .collect()
    }

    #[test]
    fn outline_is_deterministic() {
        let p = pts(40);
        let a = outline_points(&p, Tool::Pen, 4.0, false, true);
        let b = outline_points(&p, Tool::Pen, 4.0, false, true);
        assert_eq!(a, b);
        assert!(a.len() >= p.len(), "outline wraps both sides of the spine");
    }

    #[test]
    fn empty_and_single_point_do_not_panic() {
        assert!(outline_to_bezpath(&outline_points(&[], Tool::Pen, 4.0, false, true)).elements().len() <= 1);
        let one = pts(1);
        let path = outline_to_bezpath(&outline_points(&one, Tool::Pen, 4.0, false, true));
        // A dot still produces a closed outline (a small disc) or nothing — never a panic.
        let _ = path.bounding_box();
    }

    #[test]
    fn highlighter_outline_width_is_pressure_independent() {
        let mut low = pts(30);
        for p in &mut low {
            p.pressure = 0.1;
        }
        let mut high = pts(30);
        for p in &mut high {
            p.pressure = 1.0;
        }
        let w_low = outline_to_bezpath(&outline_points(&low, Tool::Highlighter, 8.0, false, true))
            .bounding_box()
            .height();
        let w_high = outline_to_bezpath(&outline_points(&high, Tool::Highlighter, 8.0, false, true))
            .bounding_box()
            .height();
        assert!((w_low - w_high).abs() < 0.5, "thinning=0 must ignore pressure: {w_low} vs {w_high}");
    }
}
