//! Raw samples → render-ready path.
//!
//! Pen and pencil strokes are the union of tapered round segments ("stamped"
//! capsules) along the lightly smoothed samples, sized by pressure. Every
//! piece is convex and wound the same way, so a nonzero fill is an exact
//! union: no notches, pinches or detached caps however wide the pen is or
//! however sharply it turns.
//!
//! Highlighters keep the perfect-freehand outline (`freedraw`) for their
//! flat chisel ends; at constant width it has none of those problems.

use freedraw::{get_stroke, InputPoint, StrokeOptions, TaperOptions};
use kurbo::BezPath;
use omaink_core::{InkPoint, Tool};

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
    let input: Vec<InputPoint> = cleaned
        .iter()
        .map(|p| InputPoint::Array([p.x, p.y], Some(p.pressure as f64)))
        .collect();
    get_stroke(&input, &options_for(tool, width, simulate_pressure, last))
}

/// Pressure → radius, perfect-freehand style: `thinning` is how much
/// pressure changes the width (0 = constant).
fn radius_for(width: f64, thinning: f64, pressure: f64) -> f64 {
    (width * (0.5 - thinning * (0.5 - pressure))).max(width * 0.08)
}

/// Smoothing passes for pen/pencil strokes (user setting: light 0,
/// normal 1, strong 3). Read at render time.
static SMOOTHING_PASSES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

/// Set the smoothing strength (number of 3-tap passes). Callers must
/// re-render cached strokes afterwards.
pub fn set_smoothing_passes(n: u8) {
    SMOOTHING_PASSES.store(n.min(6), std::sync::atomic::Ordering::Relaxed);
}

fn smooth_n(points: &[InkPoint]) -> Vec<InkPoint> {
    let mut pts = points.to_vec();
    for _ in 0..SMOOTHING_PASSES.load(std::sync::atomic::Ordering::Relaxed) {
        pts = smooth(&pts);
    }
    pts
}

/// 3-tap moving average over positions and pressure, ends pinned: removes
/// sensor jitter without shortening the stroke or lagging behind the pen.
fn smooth(points: &[InkPoint]) -> Vec<InkPoint> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let mut out = points.to_vec();
    for i in 1..n - 1 {
        let (a, b, c) = (points[i - 1], points[i], points[i + 1]);
        out[i].x = (a.x + 2.0 * b.x + c.x) / 4.0;
        out[i].y = (a.y + 2.0 * b.y + c.y) / 4.0;
        out[i].pressure = (a.pressure + 2.0 * b.pressure + c.pressure) / 4.0;
    }
    out
}

/// Douglas–Peucker over (x, y, radius): drop samples the stamped outline
/// would not visibly change (deviation under `eps` in position, or in
/// radius). Keeps segment counts low on smooth runs.
fn simplify(pts: &[(kurbo::Point, f64)], eps: f64) -> Vec<(kurbo::Point, f64)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let (a, ra) = pts[i];
        let (b, rb) = pts[j];
        let ab = b - a;
        let len2 = ab.hypot2();
        let mut worst = (0.0, 0usize);
        for (k, &(p, r)) in pts.iter().enumerate().take(j).skip(i + 1) {
            let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let on = a + ab * t;
            let dev = p.distance(on).max((r - (ra + (rb - ra) * t)).abs());
            if dev > worst.0 {
                worst = (dev, k);
            }
        }
        if worst.0 > eps {
            keep[worst.1] = true;
            stack.push((i, worst.1));
            stack.push((worst.1, j));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// Append the convex hull of circles (a, ra) and (b, rb) as one closed
/// subpath, always wound in the same (increasing-angle) direction.
fn push_capsule(path: &mut BezPath, a: kurbo::Point, ra: f64, b: kurbo::Point, rb: f64, tol: f64) {
    use std::f64::consts::TAU;
    let circle = |path: &mut BezPath, c: kurbo::Point, r: f64| {
        path.move_to((c.x + r, c.y));
        let arc = kurbo::Arc::new(c, (r, r), 0.0, TAU, 0.0);
        arc.append_iter(tol).for_each(|el| path.push(el));
        path.close_path();
    };
    let d = a.distance(b);
    if d + ra.min(rb) <= ra.max(rb) {
        // One circle contains the other.
        if ra >= rb { circle(path, a, ra) } else { circle(path, b, rb) }
        return;
    }
    let theta = (b.y - a.y).atan2(b.x - a.x);
    let phi = ((ra - rb) / d).clamp(-1.0, 1.0).acos();
    let at = |c: kurbo::Point, r: f64, ang: f64| kurbo::Point::new(c.x + r * ang.cos(), c.y + r * ang.sin());
    // Front arc around b, tangent line, back arc around a, tangent line.
    path.move_to(at(b, rb, theta - phi));
    kurbo::Arc::new(b, (rb, rb), theta - phi, 2.0 * phi, 0.0).append_iter(tol).for_each(|el| path.push(el));
    path.line_to(at(a, ra, theta + phi));
    kurbo::Arc::new(a, (ra, ra), theta + phi, TAU - 2.0 * phi, 0.0).append_iter(tol).for_each(|el| path.push(el));
    path.close_path();
}

/// Pen/pencil: union of pressure-sized round segments along the samples.
fn stamped_path(points: &[InkPoint], tool: Tool, width: f64, simulate_pressure: bool) -> BezPath {
    let mut path = BezPath::new();
    let pts = smooth_n(&clean_samples(points, width));
    if pts.is_empty() {
        return path;
    }
    let thinning = match tool {
        Tool::Pen => 0.55,
        Tool::Pencil => 0.35,
        _ => 0.0,
    };
    let r = |p: &InkPoint| radius_for(width, thinning, if simulate_pressure { 0.5 } else { p.pressure as f64 });
    let tol = (width * 0.01).clamp(0.01, 0.1);
    let stamps: Vec<(kurbo::Point, f64)> = pts.iter().map(|p| (kurbo::Point::new(p.x, p.y), r(p))).collect();
    let stamps = simplify(&stamps, (width * 0.02).clamp(0.04, 0.25));
    if stamps.len() == 1 {
        push_capsule(&mut path, stamps[0].0, stamps[0].1, stamps[0].0, stamps[0].1, tol);
        return path;
    }
    for w in stamps.windows(2) {
        push_capsule(&mut path, w[0].0, w[0].1, w[1].0, w[1].1, tol);
    }
    path
}

/// Render-ready path for a stroke, or a live prefix of one (`last` false).
/// Fill with the nonzero rule.
pub fn stroke_path(points: &[InkPoint], tool: Tool, width: f64, simulate_pressure: bool, last: bool) -> BezPath {
    match tool {
        Tool::Pen | Tool::Pencil => stamped_path(points, tool, width, simulate_pressure),
        _ => outline_to_bezpath(&outline_points(points, tool, width, simulate_pressure, last)),
    }
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
    stroke_path(points, tool, width, simulate_pressure, true)
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
        for el in path.elements() {
            let Some(v) = el.end_point() else { continue };
            let d = reach(v);
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

    /// A real 10-wide "s" that starts with a landing curl (the old outline
    /// skipped the curl and folded back on itself, notching the fill).
    fn curl_stroke() -> Vec<InkPoint> {
        [
            (285.96, 844.99, 0.047),
            (285.84, 844.93, 0.083),
            (285.62, 844.88, 0.119),
            (285.34, 844.77, 0.155),
            (284.95, 844.71, 0.190),
            (284.62, 844.60, 0.222),
            (284.29, 844.60, 0.253),
            (283.95, 844.55, 0.281),
            (283.67, 844.60, 0.308),
            (283.34, 844.71, 0.333),
            (283.00, 844.82, 0.357),
            (282.66, 845.05, 0.379),
            (282.33, 845.32, 0.399),
            (281.94, 845.66, 0.417),
            (281.55, 846.04, 0.435),
            (281.16, 846.49, 0.452),
            (280.71, 846.99, 0.469),
            (280.32, 847.54, 0.484),
            (279.99, 848.04, 0.498),
            (279.71, 848.60, 0.512),
            (279.54, 849.10, 0.524),
            (279.37, 849.54, 0.535),
            (279.32, 850.04, 0.546),
            (279.37, 850.54, 0.556),
            (279.60, 851.09, 0.566),
            (279.93, 851.82, 0.575),
            (280.60, 852.54, 0.583),
            (281.50, 853.37, 0.591),
            (282.95, 854.42, 0.598),
            (284.79, 855.48, 0.604),
            (287.02, 856.70, 0.610),
            (289.41, 858.03, 0.615),
            (291.54, 859.20, 0.620),
            (293.43, 860.30, 0.625),
            (294.99, 861.30, 0.629),
            (296.28, 862.19, 0.633),
            (297.39, 863.14, 0.636),
            (298.34, 864.13, 0.639),
            (299.07, 865.13, 0.642),
            (299.57, 866.08, 0.645),
            (299.85, 866.96, 0.647),
            (299.96, 867.80, 0.649),
            (299.85, 868.68, 0.651),
            (299.46, 869.68, 0.653),
            (298.79, 870.79, 0.654),
            (297.89, 871.96, 0.655),
            (296.39, 873.23, 0.656),
            (294.55, 874.51, 0.657),
            (292.59, 875.62, 0.657),
            (290.53, 876.68, 0.655),
            (288.75, 877.34, 0.650),
            (287.02, 877.84, 0.643),
            (285.34, 878.12, 0.633),
            (283.78, 878.29, 0.621),
            (282.16, 878.34, 0.607),
            (280.71, 878.29, 0.588),
            (279.37, 878.12, 0.563),
            (278.20, 877.90, 0.546),
            (277.25, 877.56, 0.454),
            (276.59, 877.18, 0.376),
            (276.08, 876.73, 0.312),
            (275.53, 875.40, 0.050),
            (275.53, 875.40, 0.050),
            (275.53, 875.40, 0.050),
        ]
        .iter()
        .map(|&(x, y, p)| sample(x, y, p))
        .collect()
    }

    #[test]
    fn every_drawn_sample_is_inside_the_stroke() {
        for (name, pts, width) in [("t", real_stroke(), 13.8), ("s curl", curl_stroke(), 10.0)] {
            let path = stroke_bezpath(&pts, Tool::Pen, width, false);
            for p in pts.iter().filter(|p| p.pressure > LIFT_PRESSURE) {
                assert_ne!(path.winding(kurbo::Point::new(p.x, p.y)), 0, "{name}: sample ({}, {}) outside the fill", p.x, p.y);
            }
        }
    }
}
