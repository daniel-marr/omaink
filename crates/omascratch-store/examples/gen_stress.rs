//! Generate a stress-test notebook for performance work.
//!
//!   cargo run -p omascratch-store --example gen_stress -- <notebooks_root> <strokes> <notes>
//!
//! Creates "Stress Test" with one big note (`<strokes>` handwriting-like
//! strokes spread down a long page) first in the sidebar, plus `<notes>`
//! smaller notes spread across 10 folders to load the sidebar.

use std::path::PathBuf;

use omascratch_core::{
    key_after_last, InkPoint, NoteContent, NoteId, SemanticColor, Stroke, StrokeId, Tool,
};
use omascratch_store as store;
use store::NoteDoc;

/// Tiny deterministic PRNG (xorshift) — no extra dependencies.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 1_000_000) as f64 / 1_000_000.0
    }
}

fn scribble(rng: &mut Rng, x0: f64, y0: f64) -> Stroke {
    let n = 60 + (rng.next() * 90.0) as usize;
    let mut x = x0;
    let mut y = y0;
    let mut pts = Vec::with_capacity(n);
    for i in 0..n {
        x += 0.8 + rng.next() * 1.6;
        y += (rng.next() - 0.5) * 3.0 + ((i as f64) * 0.35).sin() * 1.4;
        pts.push(InkPoint {
            x,
            y,
            pressure: 0.35 + rng.next() as f32 * 0.5,
            tilt_x: 0.0,
            tilt_y: 0.0,
            dt_ms: (i * 5) as u16,
        });
    }
    Stroke {
        id: StrokeId::new(),
        tool: Tool::Pen,
        color: SemanticColor::Foreground,
        width: 3.5,
        t0_ms: 0,
        points: pts,
    }
}

fn page(rng: &mut Rng, strokes: usize) -> NoteContent {
    // Lines of handwriting: ~12 "words" per line, 40 units line height.
    let mut out = Vec::with_capacity(strokes);
    let per_line = 12;
    for i in 0..strokes {
        let line = i / per_line;
        let col = i % per_line;
        let x = 100.0 + col as f64 * 120.0 + rng.next() * 20.0;
        let y = 60.0 + line as f64 * 40.0;
        out.push(scribble(rng, x, y));
    }
    NoteContent { strokes: out, images: vec![] }
}

fn doc(title: &str, folder: Option<omascratch_core::FolderId>, order_key: String, content: NoteContent) -> NoteDoc {
    NoteDoc {
        id: NoteId::new(),
        title: title.to_string(),
        folder,
        order_key,
        created_ms: 0,
        modified_ms: 0,
        background: Default::default(),
        content,
        opaque_elements: vec![],
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).expect("notebooks_root"));
    let strokes: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let notes: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(300);
    std::fs::create_dir_all(&root).unwrap();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut tree = store::create_notebook(&root, "Stress Test", 0).unwrap();

    // Folder "000 Big" first: holds the big note, so it opens on launch.
    let big_folder = store::create_folder(&tree, "000 Big", None).unwrap();
    tree = store::scan_notebook(&tree.dir).unwrap().unwrap();
    let big = doc("Big note", Some(big_folder.id), key_after_last(&[]), page(&mut rng, strokes));
    store::write_note(&store::note_path(&tree.dir, big.id), &big, 0).unwrap();

    // 10 more folders with small notes (~40 strokes each).
    let mut folders = Vec::new();
    for f in 0..10 {
        let meta = store::create_folder(&tree, &format!("Folder {f:02}"), None).unwrap();
        tree = store::scan_notebook(&tree.dir).unwrap().unwrap();
        folders.push(meta.id);
    }
    let mut keys: Vec<Vec<String>> = vec![Vec::new(); folders.len()];
    for n in 0..notes {
        let fi = n % folders.len();
        let key = key_after_last(&keys[fi]);
        keys[fi].push(key.clone());
        let d = doc(&format!("Note {n:03}"), Some(folders[fi]), key, page(&mut rng, 40));
        store::write_note(&store::note_path(&tree.dir, d.id), &d, 0).unwrap();
    }
    println!(
        "generated: 1 note x {strokes} strokes + {notes} notes in 10 folders under {}",
        tree.dir.display()
    );
}
