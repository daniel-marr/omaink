//! Render a note's strokes to SVG with the app's outline pipeline, for
//! inspecting stroke geometry offline.
//!
//!   cargo run -p omascratch-store --example ink_svg -- <note.omanote> <out.svg> [min_width]

use omascratch_store as store;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc = store::read_note(std::path::Path::new(&args[1])).expect("read note");
    let min_w: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let only: Option<usize> = args.get(4).and_then(|s| s.parse().ok());
    let strokes: Vec<_> = doc
        .content
        .strokes
        .iter()
        .filter(|s| s.width >= min_w)
        .enumerate()
        .filter(|(i, _)| only.is_none_or(|o| o == *i))
        .map(|(_, s)| s)
        .collect();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for s in &strokes {
        for p in &s.points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
    }
    let pad = if only.is_some() { 12.0 } else { 40.0 };
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='{} {} {} {}' width='{}' height='{}'><rect x='{}' y='{}' width='100%' height='100%' fill='white'/>",
        x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad,
        x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad, x0 - pad, y0 - pad
    );
    for s in &strokes {
        let path = omascratch_ink::stroke_bezpath(&s.points, s.tool, s.width, false);
        svg.push_str(&format!("<path d='{}' fill='black' fill-opacity='1' fill-rule='nonzero'/>", path.to_svg()));
        if only.is_some() {
            // Debug view: raw samples (red) and outline vertices (blue).
            for p in &s.points {
                svg.push_str(&format!("<circle cx='{}' cy='{}' r='0.25' fill='red'/>", p.x, p.y));
            }
            for v in omascratch_ink::outline_points(&s.points, s.tool, s.width, false, true) {
                svg.push_str(&format!("<circle cx='{}' cy='{}' r='0.2' fill='#08f'/>", v[0], v[1]));
            }
        }
    }
    svg.push_str("</svg>");
    std::fs::write(&args[2], svg).unwrap();
    eprintln!("{} strokes (widths: {:?})", strokes.len(), {
        let mut w: Vec<f64> = strokes.iter().map(|s| s.width).collect();
        w.sort_by(f64::total_cmp);
        w.dedup();
        w
    });
}
