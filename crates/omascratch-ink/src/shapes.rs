//! Basic shape generators: polylines sampled densely enough that the
//! perfect-freehand outline keeps edges straight and corners crisp. Shapes
//! commit as ordinary ink strokes, so the eraser, lasso and file format all
//! handle them with no special cases.

use kurbo::Point;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShapeKind {
    Line,
    Arrow,
    Rect,
    Ellipse,
}

fn sample_segment(out: &mut Vec<(f64, f64)>, a: Point, b: Point, n: usize) {
    for i in 0..=n {
        let t = i as f64 / n as f64;
        out.push((a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
    }
}

/// Returns one or more polylines (an arrow is shaft + head).
pub fn shape_polylines(kind: ShapeKind, start: Point, end: Point) -> Vec<Vec<(f64, f64)>> {
    match kind {
        ShapeKind::Line => {
            let mut l = Vec::new();
            sample_segment(&mut l, start, end, 16);
            vec![l]
        }
        ShapeKind::Arrow => {
            let mut shaft = Vec::new();
            sample_segment(&mut shaft, start, end, 16);
            let v = end - start;
            let len = v.hypot().max(1e-6);
            let head = (len * 0.25).clamp(8.0, 40.0);
            let dir = v / len;
            let perp = kurbo::Vec2::new(-dir.y, dir.x);
            let left = end - dir * head + perp * (head * 0.55);
            let right = end - dir * head - perp * (head * 0.55);
            let mut barbs = Vec::new();
            sample_segment(&mut barbs, left, end, 8);
            let mut right_part = Vec::new();
            sample_segment(&mut right_part, end, right, 8);
            barbs.extend(right_part.into_iter().skip(1));
            vec![shaft, barbs]
        }
        ShapeKind::Rect => {
            let (x0, x1) = (start.x.min(end.x), start.x.max(end.x));
            let (y0, y1) = (start.y.min(end.y), start.y.max(end.y));
            let corners = [
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(x1, y1),
                Point::new(x0, y1),
                Point::new(x0, y0),
            ];
            let mut l = Vec::new();
            for w in corners.windows(2) {
                let mut seg = Vec::new();
                sample_segment(&mut seg, w[0], w[1], 10);
                if !l.is_empty() {
                    seg.remove(0);
                }
                l.extend(seg);
            }
            vec![l]
        }
        ShapeKind::Ellipse => {
            let cx = (start.x + end.x) / 2.0;
            let cy = (start.y + end.y) / 2.0;
            let rx = ((end.x - start.x) / 2.0).abs().max(0.5);
            let ry = ((end.y - start.y) / 2.0).abs().max(0.5);
            let n = 48;
            let mut l = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                l.push((cx + rx * t.cos(), cy + ry * t.sin()));
            }
            vec![l]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_produce_sane_polylines() {
        let s = Point::new(0.0, 0.0);
        let e = Point::new(100.0, 50.0);
        assert_eq!(shape_polylines(ShapeKind::Line, s, e).len(), 1);
        assert_eq!(shape_polylines(ShapeKind::Arrow, s, e).len(), 2);
        let rect = shape_polylines(ShapeKind::Rect, s, e);
        assert_eq!(rect.len(), 1);
        let first = rect[0][0];
        let last = *rect[0].last().unwrap();
        assert!((first.0 - last.0).abs() < 1e-9 && (first.1 - last.1).abs() < 1e-9, "rect closes");
        let ell = shape_polylines(ShapeKind::Ellipse, s, e);
        assert!(ell[0].len() > 40);
    }
}
