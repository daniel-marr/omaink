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
    // freedraw's start cap sweeps the wrong way (it bites into wide
    // strokes); we add our own round start cap in `outline_points`.
    o.start = Some(TaperOptions { cap: Some(false), ..Default::default() });
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

/// Pressure stored for a sample the tablet reported with zero pressure
/// (the pen lifting off); see the canvas input path.
const LIFT_PRESSURE: f32 = 0.05;

/// Input cleanup before outlining (render-time only; stored samples are
/// untouched):
/// - drop lift-off samples at the end (zero pressure, often jumped sideways)
///   and exact duplicates — they bend the end cap into a hook;
/// - merge samples closer than a fraction of the pen width: while the pen
///   lands it reports sub-pixel jitter, and outlining that noise at large
///   widths twists the edges and bites notches out of the start cap.
fn clean_samples(points: &[InkPoint], width: f64) -> Vec<InkPoint> {
    let mut end = points.len();
    while end > 2 && points[end - 1].pressure <= LIFT_PRESSURE {
        end -= 1;
    }
    let pts = &points[..end];
    let Some(first) = pts.first() else { return Vec::new() };
    let min_step = (width * 0.15).max(0.25);
    let mut out: Vec<InkPoint> = vec![*first];
    for p in &pts[1..] {
        let last = out.last_mut().unwrap();
        if (p.x - last.x).hypot(p.y - last.y) < min_step {
            // Too close: keep the heavier pressure, don't add a point.
            last.pressure = last.pressure.max(p.pressure);
            continue;
        }
        out.push(*p);
    }
    // Keep the true end position so strokes don't get shorter.
    if let Some(p) = pts.last() {
        let last = *out.last().unwrap();
        if out.len() > 1 && (p.x != last.x || p.y != last.y) {
            let n = out.len();
            out[n - 1].x = p.x;
            out[n - 1].y = p.y;
        }
    }
    out
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
    let cleaned = clean_samples(points, width);
    // Taps and tiny dashes (an i's dot): too few samples for a freehand
    // outline, which degenerates into a sliver. Draw a round dot/capsule.
    if round_start(tool) && !cleaned.is_empty() {
        let len: f64 = cleaned.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
        if cleaned.len() < 3 || len < width * 0.5 {
            let p = cleaned.iter().map(|p| p.pressure).fold(0.0f32, f32::max) as f64;
            let p = if simulate_pressure { 0.5 } else { p };
            let thinning = if tool == Tool::Pen { 0.55 } else { 0.35 };
            let r = (width * (0.5 - thinning * (0.5 - p))).max(width * 0.15);
            let (a, b) = (cleaned[0], cleaned[cleaned.len() - 1]);
            return capsule([a.x, a.y], [b.x, b.y], r);
        }
    }
    let input: Vec<InputPoint> = cleaned
        .iter()
        .map(|p| InputPoint::Array([p.x, p.y], Some(p.pressure as f64)))
        .collect();
    let mut outline = get_stroke(&input, &options_for(tool, width, simulate_pressure, last));
    if round_start(tool) {
        add_round_start_cap(&mut outline);
    }
    outline
}

/// Closed outline of a capsule (a dot when `a == b`) of radius `r`.
fn capsule(a: [f64; 2], b: [f64; 2], r: f64) -> Vec<[f64; 2]> {
    let ang = (b[1] - a[1]).atan2(b[0] - a[0]);
    const STEPS: usize = 12;
    let mut out = Vec::with_capacity(2 * STEPS + 3);
    // Half circle around b (right of travel → left), then around a.
    for i in 0..=STEPS {
        let t = ang - std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / STEPS as f64;
        out.push([b[0] + r * t.cos(), b[1] + r * t.sin()]);
    }
    for i in 0..=STEPS {
        let t = ang + std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / STEPS as f64;
        out.push([a[0] + r * t.cos(), a[1] + r * t.sin()]);
    }
    out.push(out[0]);
    out
}

fn round_start(tool: Tool) -> bool {
    matches!(tool, Tool::Pen | Tool::Pencil)
}

/// Close the flat start edge (last outline vertex → first) with an outward
/// semicircle. The outline (start cap off) is: right side from the start,
/// end cap, left side back to the start, then a closing duplicate.
fn add_round_start_cap(outline: &mut Vec<[f64; 2]>) {
    if outline.len() >= 2 && outline.first() == outline.last() {
        outline.pop();
    }
    let n = outline.len();
    if n < 4 {
        return;
    }
    let r_pt = outline[0];
    let l_pt = outline[n - 1];
    let mid = [(r_pt[0] + l_pt[0]) / 2.0, (r_pt[1] + l_pt[1]) / 2.0];
    let radius = (r_pt[0] - l_pt[0]).hypot(r_pt[1] - l_pt[1]) / 2.0;
    if radius < 1e-6 {
        return;
    }
    // Into the stroke: toward the next pair of side points.
    let inner = [(outline[1][0] + outline[n - 2][0]) / 2.0 - mid[0], (outline[1][1] + outline[n - 2][1]) / 2.0 - mid[1]];
    let a0 = (l_pt[1] - mid[1]).atan2(l_pt[0] - mid[0]);
    // Sweep from the left point to the right point through the side facing
    // away from the stroke.
    let probe = |sign: f64| {
        let a = a0 + sign * std::f64::consts::FRAC_PI_2;
        (a.cos() * inner[0] + a.sin() * inner[1]) < 0.0
    };
    let sign = if probe(1.0) { 1.0 } else { -1.0 };
    const STEPS: usize = 8;
    for i in 1..STEPS {
        let a = a0 + sign * std::f64::consts::PI * i as f64 / STEPS as f64;
        outline.push([mid[0] + radius * a.cos(), mid[1] + radius * a.sin()]);
    }
    outline.push(outline[0]);
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

    fn sample(x: f64, y: f64, pressure: f32) -> InkPoint {
        InkPoint { x, y, pressure, tilt_x: 0.0, tilt_y: 0.0, dt_ms: 0 }
    }

    /// A real 13.8-wide pen stroke from an XP-Pen Artist 15.6 Pro (the
    /// downstroke of a "t"): the pen lands with sub-unit jitter while
    /// pressure ramps up, and lifts with zero-pressure samples that jumped.
    fn real_stroke() -> Vec<InkPoint> {
        [
            (134.13, 340.27, 0.073),
            (134.06, 340.11, 0.158),
            (134.06, 340.03, 0.199),
            (133.98, 339.96, 0.239),
            (133.90, 339.80, 0.279),
            (133.83, 339.73, 0.353),
            (133.83, 340.03, 0.424),
            (133.90, 340.65, 0.458),
            (133.98, 341.56, 0.490),
            (134.21, 343.01, 0.520),
            (134.44, 344.70, 0.549),
            (134.67, 346.92, 0.576),
            (135.13, 350.20, 0.602),
            (135.67, 354.40, 0.626),
            (136.29, 359.37, 0.648),
            (136.90, 365.11, 0.668),
            (137.44, 370.76, 0.687),
            (137.90, 376.03, 0.704),
            (138.21, 380.70, 0.718),
            (138.44, 384.75, 0.728),
            (138.59, 388.11, 0.736),
            (138.66, 391.17, 0.739),
            (138.74, 394.07, 0.735),
            (138.82, 396.90, 0.722),
            (138.89, 399.65, 0.704),
            (138.89, 402.25, 0.679),
            (138.89, 404.54, 0.662),
            (138.89, 406.45, 0.630),
            (138.98, 407.98, 0.524),
            (138.98, 409.21, 0.435),
            (139.05, 410.12, 0.360),
            (138.98, 411.43, 0.050),
            (138.98, 411.43, 0.050),
            (138.98, 411.43, 0.050),
        ]
        .iter()
        .map(|&(x, y, p)| sample(x, y, p))
        .collect()
    }

    #[test]
    fn wide_stroke_start_is_round_not_bitten() {
        let pts = real_stroke();
        let path = stroke_bezpath(&pts, Tool::Pen, 13.8, false);
        // 3 units into the stroke from where the pen landed: the old inward
        // start cap cut a notch ~7 units deep here.
        let (a, b) = (pts[0], pts[12]);
        let len = (b.x - a.x).hypot(b.y - a.y);
        let probe = kurbo::Point::new(a.x + (b.x - a.x) / len * 3.0, a.y + (b.y - a.y) / len * 3.0);
        assert_ne!(path.winding(probe), 0, "no notch at the start");
    }

    #[test]
    fn lift_off_sample_does_not_hook_the_end() {
        let pts = real_stroke();
        let path = stroke_bezpath(&pts, Tool::Pen, 13.8, false);
        // Everything drawn must stay within half a pen width of the samples
        // the pen actually drew (not the jumped lift-off ones).
        let drawn: Vec<InkPoint> = pts.iter().copied().filter(|p| p.pressure > LIFT_PRESSURE).collect();
        let reach = |q: kurbo::Point| drawn.iter().map(|p| (p.x - q.x).hypot(p.y - q.y)).fold(f64::MAX, f64::min);
        // Widest pressure-based radius (perfect-freehand: size * (0.5 -
        // thinning * (0.5 - pressure)), pen thinning 0.55).
        let max_p = drawn.iter().map(|p| p.pressure as f64).fold(0.0, f64::max);
        let r_max = 13.8 * (0.5 - 0.55 * (0.5 - max_p));
        for v in outline_points(&pts, Tool::Pen, 13.8, false, true) {
            let d = reach(kurbo::Point::new(v[0], v[1]));
            assert!(d <= r_max + 0.75, "outline vertex {v:?} is {d:.2} from the stroke (hook; max radius {r_max:.2})");
        }
    }

    #[test]
    fn a_tap_draws_a_round_dot() {
        let tap: Vec<InkPoint> = (0..12).map(|i| sample(50.0 + i as f64 * 0.05, 50.0, 0.5)).collect();
        let path = stroke_bezpath(&tap, Tool::Pen, 14.0, false);
        let b = path.bounding_box();
        assert!(b.width() > 6.0 && b.height() > 6.0, "dot, not a sliver: {b:?}");
        assert!((b.width() - b.height()).abs() < 2.0, "roughly round: {b:?}");
        assert_ne!(path.winding(kurbo::Point::new(50.2, 50.0)), 0, "dot is filled");
    }
}
