//! Print the first/last samples of strokes at least `min_width` wide.
//!   cargo run -p omaink-store --example ink_dump -- <note> <min_width>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let doc = omaink_store::read_note(std::path::Path::new(&a[1])).unwrap();
    let min_w: f64 = a[2].parse().unwrap();
    for (i, s) in doc.content.strokes.iter().filter(|s| s.width >= min_w).enumerate() {
        let len: f64 = s.points.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
        if let Some(k) = std::env::var("FIXTURE").ok().and_then(|v| v.parse::<usize>().ok()) {
            if i == k {
                for p in &s.points {
                    println!("({:.2}, {:.2}, {:.3}),", p.x, p.y, p.pressure);
                }
            }
            continue;
        }
        if std::env::var_os("SHORT").is_some() {
            if len < s.width {
                let o = omaink_ink::outline_points(&s.points, s.tool, s.width, false, true);
                println!("stroke {i}: {} pts, path len {len:.2}, width {:.1}, outline verts {}", s.points.len(), s.width, o.len());
            }
            continue;
        }
        let n = s.points.len();
        println!("stroke {i}: {n} pts, width {:.1}", s.width);
        let row = |j: usize| {
            let p = &s.points[j];
            let d = if j > 0 { let q = &s.points[j - 1]; ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt() } else { 0.0 };
            println!("  [{j:3}] p={:.3} step={d:6.2} dt={}", p.pressure, p.dt_ms);
        };
        for j in 0..n.min(8) { row(j); }
        println!("  ...");
        for j in n.saturating_sub(8)..n { row(j); }
    }
}
