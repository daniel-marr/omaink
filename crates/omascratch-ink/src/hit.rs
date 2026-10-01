//! Geometry queries over raw stroke samples: distance to a stroke, point/rect
//! containment. Used by the eraser and lasso selection.

use kurbo::{Point, Rect};
use omascratch_core::InkPoint;

fn dist_point_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= f64::EPSILON {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    let proj = a + ab * t;
    (p - proj).hypot()
}

/// Shortest distance from `p` to the polyline through `points`.
pub fn distance_to_stroke(points: &[InkPoint], p: Point) -> f64 {
    if points.is_empty() {
        return f64::INFINITY;
    }
    if points.len() == 1 {
        return (p - Point::new(points[0].x, points[0].y)).hypot();
    }
    let mut best = f64::INFINITY;
    for w in points.windows(2) {
        let a = Point::new(w[0].x, w[0].y);
        let b = Point::new(w[1].x, w[1].y);
        best = best.min(dist_point_to_segment(p, a, b));
    }
    best
}

/// True if `p` lies within `radius` of the stroke (accounting for its width).
pub fn stroke_hit(points: &[InkPoint], width: f64, p: Point, radius: f64) -> bool {
    distance_to_stroke(points, p) <= radius + width / 2.0
}

/// True if every sample of the stroke lies inside `rect` (lasso: fully enclosed).
pub fn stroke_inside_rect(points: &[InkPoint], rect: Rect) -> bool {
    !points.is_empty()
        && points.iter().all(|p| rect.contains(Point::new(p.x, p.y)))
}

/// Area-eraser splitting: remove every sample within `radius` of any point on
/// the eraser path, returning the surviving runs as fragments. A fragment
/// needs at least 2 samples to remain drawable ink.
pub fn erase_samples(points: &[InkPoint], eraser_path: &[Point], radius: f64) -> Vec<Vec<InkPoint>> {
    let mut fragments: Vec<Vec<InkPoint>> = Vec::new();
    let mut current: Vec<InkPoint> = Vec::new();
    for pt in points {
        let p = Point::new(pt.x, pt.y);
        let erased = eraser_path.iter().any(|e| (p - *e).hypot() <= radius);
        if erased {
            if current.len() >= 2 {
                fragments.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        } else {
            current.push(*pt);
        }
    }
    if current.len() >= 2 {
        fragments.push(current);
    }
    fragments
}

/// True if any sample of the stroke is within `radius` of any eraser point.
pub fn stroke_touched(points: &[InkPoint], eraser_path: &[Point], radius: f64, width: f64) -> bool {
    let r = radius + width / 2.0;
    points.iter().any(|pt| {
        let p = Point::new(pt.x, pt.y);
        eraser_path.iter().any(|e| (p - *e).hypot() <= r)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Vec<InkPoint> {
        vec![
            InkPoint { x: 0.0, y: 0.0, pressure: 0.5, tilt_x: 0.0, tilt_y: 0.0, dt_ms: 0 },
            InkPoint { x: 10.0, y: 0.0, pressure: 0.5, tilt_x: 0.0, tilt_y: 0.0, dt_ms: 0 },
        ]
    }

    #[test]
    fn distance_and_hit() {
        let l = line();
        assert!((distance_to_stroke(&l, Point::new(5.0, 3.0)) - 3.0).abs() < 1e-9);
        assert!(stroke_hit(&l, 2.0, Point::new(5.0, 3.0), 2.5)); // 2.5 + 1.0 >= 3.0
        assert!(!stroke_hit(&l, 2.0, Point::new(5.0, 10.0), 2.0));
    }

    #[test]
    fn area_erase_splits_a_stroke_into_fragments() {
        // 11 samples along x=0..10; erase around x=5 with radius 1.2.
        let pts: Vec<InkPoint> = (0..=10)
            .map(|i| InkPoint { x: i as f64, y: 0.0, pressure: 0.5, tilt_x: 0.0, tilt_y: 0.0, dt_ms: i as u16 })
            .collect();
        let frags = erase_samples(&pts, &[Point::new(5.0, 0.0)], 1.2);
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0].len(), 4); // x = 0..=3
        assert_eq!(frags[1].len(), 4); // x = 7..=10
        // Timestamps survive on fragments (Ink Replay stays possible).
        assert_eq!(frags[1][0].dt_ms, 7);

        // Erasing everything leaves no fragments.
        let all = erase_samples(&pts, &[Point::new(5.0, 0.0)], 20.0);
        assert!(all.is_empty());

        // Erasing nothing returns the original run.
        let none = erase_samples(&pts, &[Point::new(5.0, 50.0)], 1.0);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].len(), 11);
    }

    #[test]
    fn enclosure() {
        let l = line();
        assert!(stroke_inside_rect(&l, Rect::new(-1.0, -1.0, 11.0, 1.0)));
        assert!(!stroke_inside_rect(&l, Rect::new(-1.0, -1.0, 5.0, 1.0)));
    }
}

/// Ray-casting point-in-polygon test.
pub fn point_in_polygon(p: Point, poly: &[Point]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x_cross = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Lasso semantics: a stroke is selected when all its samples are inside.
pub fn stroke_inside_polygon(points: &[InkPoint], poly: &[Point]) -> bool {
    !points.is_empty() && points.iter().all(|pt| point_in_polygon(Point::new(pt.x, pt.y), poly))
}
