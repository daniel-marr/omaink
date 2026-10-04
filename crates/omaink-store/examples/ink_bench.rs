//! Compare stroke path construction: perfect-freehand outline vs stamped
//! capsules, over every stroke in a note.
//!   cargo run --release -p omaink-store --example ink_bench -- <note>
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let doc = omaink_store::read_note(std::path::Path::new(&a[1])).unwrap();
    let strokes = &doc.content.strokes;
    let report = |name: &str, f: &dyn Fn(&omaink_core::Stroke) -> kurbo::BezPath| {
        let t = Instant::now();
        let mut els = 0usize;
        for s in strokes {
            els += f(s).elements().len();
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("{name:>10}: {ms:8.1} ms for {} strokes ({:.3} ms/stroke), {els} path elements ({:.0}/stroke)",
            strokes.len(), ms / strokes.len() as f64, els as f64 / strokes.len() as f64);
    };
    report("freehand", &|s| omaink_ink::outline_to_bezpath(&omaink_ink::outline_points(&s.points, s.tool, s.width, false, true)));
    report("stamped", &|s| omaink_ink::stroke_bezpath(&s.points, s.tool, s.width, false));
}
