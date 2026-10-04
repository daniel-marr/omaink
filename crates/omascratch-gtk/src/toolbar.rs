//! OneNote fluid-style Draw toolbar.
//!
//! Layout: undo/redo │ Select, Lasso │ [eraser chip][pen chips…] + Add Pen ▾ │
//! Shapes ▾, Format Background ▾, ⋯ (the right-side group is stubbed until
//! shapes/M5 land). One mode is active at a time across select/lasso/eraser/
//! pens. Clicking the active pen or eraser chip opens its OneNote-style
//! flyout (stroke preview, − dots +, Recent Colors, Colors grid, Remove Pen /
//! eraser types). The pen set persists in XDG state.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::{gdk, glib, prelude::*};

use omascratch_core::{Rgba, SemanticColor, Tool};
use omascratch_store as store;

use crate::canvas::{ActiveTool, CanvasView, EraserKind};
use omascratch_core::{BackgroundKind, PageBackground};
use omascratch_ink::ShapeKind;


const STATE_FILE: &str = "toolbar.json";

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct PenCfg {
    tool: Tool,
    color: SemanticColor,
    width: f64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ToolbarState {
    schema: u32,
    pens: Vec<PenCfg>,
    active_pen: usize,
    eraser_area: bool,
    eraser_radius: f64,
    recent: Vec<SemanticColor>,
    #[serde(default = "default_shape_color")]
    shape_color: SemanticColor,
    #[serde(default = "default_shape_width")]
    shape_width: f64,
    #[serde(default)]
    canvas_inverted: bool,
}

fn default_shape_color() -> SemanticColor {
    SemanticColor::Foreground
}
fn default_shape_width() -> f64 {
    3.0
}

impl Default for ToolbarState {
    fn default() -> Self {
        Self {
            schema: 1,
            pens: default_pens(),
            active_pen: 0,
            eraser_area: false,
            eraser_radius: 16.0,
            recent: Vec::new(),
            shape_color: default_shape_color(),
            shape_width: default_shape_width(),
            canvas_inverted: false,
        }
    }
}

fn fixed(r: f32, g: f32, b: f32) -> SemanticColor {
    SemanticColor::Fixed(Rgba { r, g, b, a: 1.0 })
}

fn default_pens() -> Vec<PenCfg> {
    vec![
        PenCfg { tool: Tool::Pen, color: SemanticColor::Foreground, width: 3.5 },
        PenCfg { tool: Tool::Pen, color: fixed(0.90, 0.30, 0.35), width: 3.5 },
        PenCfg { tool: Tool::Pen, color: fixed(0.42, 0.62, 0.96), width: 3.5 },
        PenCfg { tool: Tool::Pencil, color: SemanticColor::Foreground, width: 2.0 },
        PenCfg { tool: Tool::Highlighter, color: fixed(0.98, 0.84, 0.25), width: 16.0 },
    ]
}

/// OneNote's Colors grid, arranged by hue family, 5 per row.
fn color_rows() -> Vec<Vec<(&'static str, SemanticColor)>> {
    // Vivid markup colors: two hue-wheel rows, pinks + deep inks, neon
    // highlighters, then one neutrals row (theme-following ink and accent).
    vec![
        vec![
            ("Red", fixed(0.94, 0.27, 0.27)),
            ("Orange", fixed(0.98, 0.45, 0.09)),
            ("Amber", fixed(0.96, 0.62, 0.04)),
            ("Yellow", fixed(0.98, 0.80, 0.08)),
            ("Lime", fixed(0.52, 0.80, 0.09)),
            ("Green", fixed(0.13, 0.77, 0.37)),
            ("Emerald", fixed(0.06, 0.73, 0.51)),
        ],
        vec![
            ("Teal", fixed(0.08, 0.72, 0.65)),
            ("Cyan", fixed(0.02, 0.71, 0.83)),
            ("Sky", fixed(0.05, 0.65, 0.91)),
            ("Blue", fixed(0.23, 0.51, 0.96)),
            ("Indigo", fixed(0.39, 0.40, 0.95)),
            ("Violet", fixed(0.55, 0.36, 0.96)),
            ("Purple", fixed(0.66, 0.33, 0.97)),
        ],
        vec![
            ("Fuchsia", fixed(0.85, 0.27, 0.94)),
            ("Pink", fixed(0.93, 0.28, 0.60)),
            ("Rose", fixed(0.96, 0.25, 0.37)),
            ("Deep red", fixed(0.73, 0.11, 0.11)),
            ("Deep blue", fixed(0.11, 0.31, 0.85)),
            ("Deep green", fixed(0.08, 0.50, 0.24)),
            ("Deep purple", fixed(0.43, 0.16, 0.85)),
        ],
        vec![
            ("Neon yellow", fixed(1.00, 0.94, 0.12)),
            ("Neon orange", fixed(1.00, 0.54, 0.00)),
            ("Neon pink", fixed(1.00, 0.18, 0.58)),
            ("Neon magenta", fixed(0.88, 0.25, 0.98)),
            ("Neon blue", fixed(0.00, 0.70, 1.00)),
            ("Neon cyan", fixed(0.00, 0.90, 0.83)),
            ("Neon green", fixed(0.30, 1.00, 0.30)),
        ],
        vec![
            ("Black", fixed(0.08, 0.08, 0.10)),
            ("Dark grey", fixed(0.32, 0.34, 0.40)),
            ("Grey", fixed(0.55, 0.57, 0.63)),
            ("Light grey", fixed(0.80, 0.82, 0.86)),
            ("White", fixed(0.97, 0.97, 0.98)),
            ("Theme ink", SemanticColor::Foreground),
            ("Accent", SemanticColor::Accent),
        ],
    ]
}

/// Resolve a semantic color against the live canvas palette.
fn rgba_of(canvas: &CanvasView, c: SemanticColor) -> gdk::RGBA {
    canvas.preview_rgba(c)
}

// ---- drawn glyphs (no icon-theme dependency) ----

fn set_source(cr: &gtk::cairo::Context, c: gdk::RGBA, alpha: f64) {
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, alpha);
}

/// Pen chips: dark barrel pointing down with a colored band and nib.
/// Highlighters are chisel-tipped; pencils get a wood collar.
fn pen_glyph(cfg: PenCfg, rgba: gdk::RGBA) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(26);
    area.set_content_height(44);
    area.set_draw_func(move |_, cr, w, h| draw_pen(cr, cfg.tool, rgba, w as f64, h as f64));
    area
}

fn draw_pen(cr: &gtk::cairo::Context, tool: Tool, rgba: gdk::RGBA, w: f64, h: f64) {
        let cx = w / 2.0;
        let set = |cr: &gtk::cairo::Context| {
            cr.set_source_rgba(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64, 1.0)
        };
        match tool {
            Tool::Highlighter => {
                // Same height as the pencil: barrel from 6% down to a
                // tapered collar and an angled chisel tip at 96%.
                let bw = w * 0.56;
                cr.set_source_rgb(0.18, 0.19, 0.25);
                rounded_rect(cr, cx - bw / 2.0, h * 0.06, bw, h * 0.56, 2.5);
                let _ = cr.fill();
                set(cr);
                cr.rectangle(cx - bw / 2.0 + 2.0, h * 0.16, bw - 4.0, h * 0.16);
                let _ = cr.fill();
                let tw = bw * 0.62;
                cr.set_source_rgb(0.30, 0.31, 0.38);
                cr.move_to(cx - bw / 2.0, h * 0.62);
                cr.line_to(cx + bw / 2.0, h * 0.62);
                cr.line_to(cx + tw / 2.0, h * 0.78);
                cr.line_to(cx - tw / 2.0, h * 0.78);
                cr.close_path();
                let _ = cr.fill();
                set(cr);
                cr.move_to(cx - tw / 2.0, h * 0.78);
                cr.line_to(cx + tw / 2.0, h * 0.78);
                cr.line_to(cx + tw / 2.0, h * 0.86);
                cr.line_to(cx - tw / 2.0, h * 0.96);
                cr.close_path();
                let _ = cr.fill();
            }
            Tool::Pencil => {
                let bw = w * 0.40;
                cr.set_source_rgb(0.18, 0.19, 0.25);
                rounded_rect(cr, cx - bw / 2.0, h * 0.06, bw, h * 0.56, 2.0);
                let _ = cr.fill();
                cr.set_source_rgb(0.82, 0.68, 0.46);
                cr.move_to(cx - bw / 2.0, h * 0.62);
                cr.line_to(cx + bw / 2.0, h * 0.62);
                cr.line_to(cx, h * 0.96);
                cr.close_path();
                let _ = cr.fill();
                set(cr);
                cr.move_to(cx - bw * 0.18, h * 0.84);
                cr.line_to(cx + bw * 0.18, h * 0.84);
                cr.line_to(cx, h * 0.96);
                cr.close_path();
                let _ = cr.fill();
            }
            Tool::Pen | Tool::Shape => {
                let bw = w * 0.40;
                cr.set_source_rgb(0.18, 0.19, 0.25);
                rounded_rect(cr, cx - bw / 2.0, h * 0.06, bw, h * 0.56, 2.0);
                let _ = cr.fill();
                set(cr);
                cr.rectangle(cx - bw / 2.0, h * 0.50, bw, h * 0.10);
                let _ = cr.fill();
                cr.set_source_rgb(0.30, 0.31, 0.38);
                cr.move_to(cx - bw / 2.0, h * 0.62);
                cr.line_to(cx + bw / 2.0, h * 0.62);
                cr.line_to(cx, h * 0.92);
                cr.close_path();
                let _ = cr.fill();
                set(cr);
                cr.move_to(cx - bw * 0.20, h * 0.80);
                cr.line_to(cx + bw * 0.20, h * 0.80);
                cr.line_to(cx, h * 0.97);
                cr.close_path();
                let _ = cr.fill();
            }
        }
}

/// Eraser: the pencil chip turned over — wood tip up, the same dark
/// barrel, then a metal ferrule and a pink eraser at the bottom.
fn eraser_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(26);
    area.set_content_height(44);
    area.set_draw_func(|_, cr, w, h| draw_eraser(cr, w as f64, h as f64));
    area
}

fn draw_eraser(cr: &gtk::cairo::Context, w: f64, h: f64) {
    let cx = w / 2.0;
    let bw = w * 0.40;
    // Sharpened wood cone with a graphite point, pointing up.
    cr.set_source_rgb(0.82, 0.68, 0.46);
    cr.move_to(cx - bw / 2.0, h * 0.38);
    cr.line_to(cx + bw / 2.0, h * 0.38);
    cr.line_to(cx, h * 0.04);
    cr.close_path();
    let _ = cr.fill();
    cr.set_source_rgb(0.30, 0.31, 0.38);
    cr.move_to(cx - bw * 0.18, h * 0.16);
    cr.line_to(cx + bw * 0.18, h * 0.16);
    cr.line_to(cx, h * 0.04);
    cr.close_path();
    let _ = cr.fill();
    // Barrel.
    cr.set_source_rgb(0.18, 0.19, 0.25);
    cr.rectangle(cx - bw / 2.0, h * 0.38, bw, h * 0.40);
    let _ = cr.fill();
    // Metal ferrule with two crimp lines.
    cr.set_source_rgb(0.66, 0.68, 0.74);
    cr.rectangle(cx - bw / 2.0, h * 0.78, bw, h * 0.07);
    let _ = cr.fill();
    cr.set_source_rgba(0.35, 0.36, 0.42, 0.9);
    cr.set_line_width(0.8);
    for y in [0.80, 0.83] {
        cr.move_to(cx - bw / 2.0, h * y);
        cr.line_to(cx + bw / 2.0, h * y);
    }
    let _ = cr.stroke();
    // Pink eraser, rounded at the end.
    cr.set_source_rgb(0.94, 0.45, 0.55);
    rounded_rect(cr, cx - bw / 2.0, h * 0.85 - 2.0, bw, h * 0.11 + 2.0, 2.5);
    let _ = cr.fill();
    cr.set_source_rgb(0.66, 0.68, 0.74);
    cr.rectangle(cx - bw / 2.0, h * 0.84, bw, 1.0);
    let _ = cr.fill();
}

/// Insert-image icon: a framed picture (mountains + sun) with a plus badge,
/// drawn at full toolbar size so it reads like the other tools.
fn image_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(24);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        let fg = (0.78, 0.82, 0.96);
        // Frame.
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        cr.set_line_width(1.6);
        rounded_rect(cr, w * 0.06, h * 0.12, w * 0.74, h * 0.70, 2.5);
        let _ = cr.stroke();
        // Sun.
        cr.arc(w * 0.27, h * 0.33, w * 0.07, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        // Mountains.
        cr.move_to(w * 0.10, h * 0.78);
        cr.line_to(w * 0.32, h * 0.50);
        cr.line_to(w * 0.46, h * 0.66);
        cr.line_to(w * 0.58, h * 0.48);
        cr.line_to(w * 0.76, h * 0.78);
        cr.close_path();
        let _ = cr.fill();
        // Plus badge (bottom-right), punched out of the frame.
        let (bx, by, br) = (w * 0.80, h * 0.76, w * 0.17);
        cr.set_source_rgb(0.086, 0.086, 0.118);
        cr.arc(bx, by, br + 1.2, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        cr.arc(bx, by, br, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_source_rgb(0.086, 0.086, 0.118);
        cr.set_line_width(1.6);
        cr.move_to(bx - br * 0.55, by);
        cr.line_to(bx + br * 0.55, by);
        cr.move_to(bx, by - br * 0.55);
        cr.line_to(bx, by + br * 0.55);
        let _ = cr.stroke();
    });
    area
}

/// Format-group button: markup label, never steals focus from the editor.
fn fmt_button(markup: &str, tip: &str) -> gtk::Button {
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    let b = gtk::Button::new();
    b.set_child(Some(&label));
    b.add_css_class("flat");
    b.set_focus_on_click(false);
    b.set_tooltip_text(Some(tip));
    b
}

/// Text tool icon: a bold serif-style "T".
fn text_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        cr.set_source_rgb(0.78, 0.82, 0.96);
        // Top bar with small serifs.
        cr.rectangle(w * 0.18, h * 0.14, w * 0.64, h * 0.14);
        cr.rectangle(w * 0.18, h * 0.14, w * 0.07, h * 0.24);
        cr.rectangle(w * 0.75, h * 0.14, w * 0.07, h * 0.24);
        // Stem and foot.
        cr.rectangle(w * 0.43, h * 0.14, w * 0.14, h * 0.70);
        cr.rectangle(w * 0.33, h * 0.78, w * 0.34, h * 0.08);
        let _ = cr.fill();
    });
    area
}

/// Hand (pan) icon: an open-palm outline, after Lucide's `hand` icon (ISC),
/// traced on its 24-unit grid.
fn hand_glyph() -> gtk::DrawingArea {
    use std::f64::consts::PI;
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let k = (w.min(h) as f64) / 24.0;
        cr.scale(k, k);
        cr.set_source_rgb(0.78, 0.82, 0.96);
        cr.set_line_width(1.8);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        // Ring, middle and index fingers.
        cr.move_to(18.0, 11.0);
        cr.line_to(18.0, 6.0);
        cr.arc_negative(16.0, 6.0, 2.0, 0.0, -PI);
        cr.new_sub_path();
        cr.move_to(14.0, 10.0);
        cr.line_to(14.0, 4.0);
        cr.arc_negative(12.0, 4.0, 2.0, 0.0, -PI);
        cr.line_to(10.0, 6.0);
        cr.new_sub_path();
        cr.move_to(10.0, 10.5);
        cr.line_to(10.0, 6.0);
        cr.arc_negative(8.0, 6.0, 2.0, 0.0, -PI);
        cr.line_to(6.0, 14.0);
        // Little finger, palm and thumb.
        cr.new_sub_path();
        cr.move_to(18.0, 8.0);
        cr.arc(20.0, 8.0, 2.0, PI, 2.0 * PI);
        cr.line_to(22.0, 14.0);
        cr.arc(14.0, 14.0, 8.0, 0.0, PI / 2.0);
        cr.line_to(12.0, 22.0);
        cr.curve_to(9.2, 22.0, 7.5, 21.14, 6.01, 19.66);
        cr.line_to(2.41, 16.06);
        cr.arc(3.825, 14.65, 2.0, 0.75 * PI, 1.75 * PI);
        cr.line_to(7.0, 15.0);
        let _ = cr.stroke();
    });
    area
}

/// Format-background icon: a mini ruled page with a margin line.
fn ruled_page_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        // Page outline.
        cr.set_source_rgba(0.78, 0.82, 0.96, 0.9);
        cr.set_line_width(1.3);
        rounded_rect(cr, w * 0.12, h * 0.08, w * 0.76, h * 0.84, 2.5);
        let _ = cr.stroke();
        // Rule lines.
        cr.set_line_width(1.1);
        cr.set_source_rgba(0.62, 0.67, 0.84, 0.8);
        for i in 1..=3 {
            let y = h * (0.22 + 0.20 * i as f64);
            cr.move_to(w * 0.20, y);
            cr.line_to(w * 0.80, y);
            let _ = cr.stroke();
        }
        // Red margin line.
        cr.set_source_rgba(0.93, 0.45, 0.53, 0.95);
        cr.move_to(w * 0.30, h * 0.12);
        cr.line_to(w * 0.30, h * 0.88);
        let _ = cr.stroke();
    });
    area
}

/// Shapes toolbar icon: overlapping square + circle.
fn shapes_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        cr.set_source_rgb(0.78, 0.82, 0.96);
        cr.set_line_width(1.5);
        cr.rectangle(w * 0.12, h * 0.12, w * 0.52, h * 0.52);
        let _ = cr.stroke();
        cr.arc(w * 0.62, h * 0.62, w * 0.26, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
    });
    area
}

/// Half-dark / half-light circle: invert the canvas theme.
fn invert_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        let r = w.min(h) * 0.38;
        let (cx, cy) = (w / 2.0, h / 2.0);
        cr.set_source_rgb(0.90, 0.91, 0.95);
        cr.arc(cx, cy, r, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
        let _ = cr.fill();
        cr.set_source_rgb(0.30, 0.32, 0.42);
        cr.arc(cx, cy, r, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
        let _ = cr.fill();
        cr.set_source_rgb(0.78, 0.82, 0.96);
        cr.set_line_width(1.3);
        cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
    });
    area
}

fn select_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        // Classic cursor arrow, recentered from its own bounding box.
        let pts: [(f64, f64); 7] = [
            (0.30, 0.06),
            (0.30, 0.78),
            (0.46, 0.62),
            (0.57, 0.88),
            (0.68, 0.83),
            (0.56, 0.58),
            (0.76, 0.58),
        ];
        let (minx, maxx) = (0.30, 0.76);
        let (miny, maxy) = (0.06, 0.88);
        let dx = 0.5 - (minx + maxx) / 2.0;
        let dy = 0.5 - (miny + maxy) / 2.0;
        let s = w.min(h) as f64;
        cr.set_source_rgb(0.78, 0.82, 0.96);
        cr.move_to((pts[0].0 + dx) * s, (pts[0].1 + dy) * s);
        for p in &pts[1..] {
            cr.line_to((p.0 + dx) * s, (p.1 + dy) * s);
        }
        cr.close_path();
        let _ = cr.fill();
    });
    area
}

fn lasso_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        cr.set_source_rgb(0.78, 0.82, 0.96);
        cr.set_line_width(1.8);
        // Closed dashed loop, centered.
        cr.set_dash(&[3.0, 2.4], 0.0);
        let _ = cr.save();
        cr.translate(w / 2.0, h / 2.0);
        cr.scale(1.0, 0.78);
        cr.arc(0.0, 0.0, w * 0.43, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
        let _ = cr.restore();
        // Solid knot sitting ON the loop (bottom-right) — no floating tail.
        cr.set_dash(&[], 0.0);
        let kx = w / 2.0 + w * 0.43 * 0.707;
        let ky = h / 2.0 + h * 0.78 * 0.43 * 0.707;
        cr.arc(kx, ky, 2.6, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    area
}

/// A 22px outline glyph drawn on Lucide's 24-unit grid (ISC), matching the
/// hand icon's stroke.
fn lucide_glyph(draw: impl Fn(&gtk::cairo::Context) + 'static) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |_, cr, w, h| {
        let k = (w.min(h) as f64) / 24.0;
        cr.scale(k, k);
        cr.set_source_rgb(0.78, 0.82, 0.96);
        cr.set_line_width(1.8);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        draw(cr);
        let _ = cr.stroke();
    });
    area
}

/// Copy: two overlapping sheets (Lucide `copy`).
fn copy_glyph() -> gtk::DrawingArea {
    lucide_glyph(|cr| {
        rounded_rect(cr, 8.0, 8.0, 14.0, 14.0, 2.0);
        // Back sheet: only the parts not hidden by the front one.
        cr.new_sub_path();
        cr.move_to(4.0, 16.0);
        cr.arc(4.0, 14.0, 2.0, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.line_to(2.0, 4.0);
        cr.arc(4.0, 4.0, 2.0, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.line_to(14.0, 2.0);
        cr.arc(14.0, 4.0, 2.0, 1.5 * std::f64::consts::PI, 0.0);
    })
}

/// Paste: a clipboard (Lucide `clipboard`).
fn paste_glyph() -> gtk::DrawingArea {
    lucide_glyph(|cr| {
        rounded_rect(cr, 8.0, 2.0, 8.0, 4.0, 1.0);
        cr.new_sub_path();
        cr.move_to(16.0, 4.0);
        cr.line_to(18.0, 4.0);
        cr.arc(18.0, 6.0, 2.0, 1.5 * std::f64::consts::PI, 0.0);
        cr.line_to(20.0, 20.0);
        cr.arc(18.0, 20.0, 2.0, 0.0, std::f64::consts::FRAC_PI_2);
        cr.line_to(6.0, 22.0);
        cr.arc(6.0, 20.0, 2.0, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.line_to(4.0, 6.0);
        cr.arc(6.0, 6.0, 2.0, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.line_to(8.0, 4.0);
    })
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

fn color_swatch(color: gdk::RGBA, size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_draw_func(move |_, cr, w, h| {
        set_source(cr, color, color.alpha() as f64);
        rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
        let _ = cr.fill();
    });
    area
}

// ---- the toolbar ----

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Select,
    Lasso,
    Eraser,
    Pen(usize),
    Shape(ShapeKind),
    Pan,
    Text,
}

struct Inner {
    canvas: CanvasView,
    pens: RefCell<Vec<PenCfg>>,
    recent: RefCell<Vec<SemanticColor>>,
    eraser_area: Cell<bool>,
    eraser_radius: Cell<f64>,
    shape_color: RefCell<SemanticColor>,
    shape_width: Cell<f64>,
    last_shape: Cell<ShapeKind>,
    on_background: RefCell<Option<Box<dyn Fn(PageBackground)>>>,
    mode: Cell<Mode>,
    /// Last non-eraser mode, restored when the barrel button toggles back.
    prev_mode: Cell<Mode>,
    /// Pen-chip reordering: armed by long-press (or right-click drag),
    /// source index while dragging, and a one-shot click suppressor so a
    /// long-press that ends without a drag doesn't also select the pen.
    reorder_armed: Cell<Option<usize>>,
    drag_src: Cell<Option<usize>>,
    suppress_click: Cell<bool>,
    gallery: gtk::Box,
    select_btn: gtk::Button,
    lasso_btn: gtk::Button,
    pan_btn: gtk::Button,
    shapes_btn: gtk::MenuButton,
    text_btn: gtk::Button,
}

#[derive(Clone)]
pub struct Toolbar {
    pub widget: gtk::Box,
    inner: Rc<Inner>,
}

pub fn build(canvas: &CanvasView) -> Toolbar {
    Toolbar::new(canvas)
}

impl Toolbar {
    fn new(canvas: &CanvasView) -> Toolbar {
        let st = load_state();

        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        widget.add_css_class("draw-toolbar");
        widget.set_margin_start(8);
        widget.set_valign(gtk::Align::Center);

        let select_btn = gtk::Button::new();
        select_btn.add_css_class("flat");
        select_btn.set_child(Some(&select_glyph()));
        select_btn.set_tooltip_text(Some("Select"));
        let lasso_btn = gtk::Button::new();
        lasso_btn.add_css_class("flat");
        lasso_btn.set_child(Some(&lasso_glyph()));
        lasso_btn.set_tooltip_text(Some("Lasso select"));
        let pan_btn = gtk::Button::new();
        pan_btn.add_css_class("flat");
        pan_btn.set_child(Some(&hand_glyph()));
        pan_btn.set_tooltip_text(Some("Pan canvas (hand)"));
        let text_btn = gtk::Button::new();
        text_btn.add_css_class("flat");
        text_btn.set_child(Some(&text_glyph()));
        text_btn.set_tooltip_text(Some("Text — click the page to type; click again for text settings"));
        text_btn.set_focus_on_click(false);
        let shapes_btn = gtk::MenuButton::new();
        shapes_btn.add_css_class("flat");
        shapes_btn.set_child(Some(&shapes_glyph()));
        shapes_btn.set_tooltip_text(Some("Shapes"));

        let inner = Rc::new(Inner {
            canvas: canvas.clone(),
            pens: RefCell::new(st.pens.clone()),
            recent: RefCell::new(st.recent.clone()),
            eraser_area: Cell::new(st.eraser_area),
            eraser_radius: Cell::new(st.eraser_radius),
            shape_color: RefCell::new(st.shape_color),
            shape_width: Cell::new(st.shape_width),
            last_shape: Cell::new(ShapeKind::Line),
            on_background: RefCell::new(None),
            mode: Cell::new(Mode::Pen(st.active_pen.min(st.pens.len().saturating_sub(1)))),
            prev_mode: Cell::new(Mode::Pen(0)),
            reorder_armed: Cell::new(None),
            drag_src: Cell::new(None),
            suppress_click: Cell::new(false),
            gallery: gtk::Box::new(gtk::Orientation::Horizontal, 1),
            select_btn: select_btn.clone(),
            lasso_btn: lasso_btn.clone(),
            pan_btn: pan_btn.clone(),
            shapes_btn: shapes_btn.clone(),
            text_btn: text_btn.clone(),
        });
        let tb = Toolbar { widget, inner };

        // Undo / redo.
        let undo = gtk::Button::from_icon_name("edit-undo-symbolic");
        undo.add_css_class("flat");
        undo.set_tooltip_text(Some("Undo (Ctrl+Z)"));
        let c = canvas.clone();
        undo.connect_clicked(move |_| c.undo());
        let redo = gtk::Button::from_icon_name("edit-redo-symbolic");
        redo.add_css_class("flat");
        redo.set_tooltip_text(Some("Redo (Ctrl+Shift+Z)"));
        let c = canvas.clone();
        redo.connect_clicked(move |_| c.redo());
        tb.widget.append(&undo);
        tb.widget.append(&redo);
        tb.widget.append(&vsep());

        // Select + Lasso.
        {
            let t = tb.clone();
            select_btn.connect_clicked(move |_| t.set_mode(Mode::Select));
        }
        {
            let t = tb.clone();
            lasso_btn.connect_clicked(move |_| t.set_mode(Mode::Lasso));
        }
        {
            let t = tb.clone();
            pan_btn.connect_clicked(move |_| t.set_mode(Mode::Pan));
        }
        tb.widget.append(&select_btn);
        tb.widget.append(&pan_btn);
        tb.widget.append(&lasso_btn);
        let copy_btn = gtk::Button::new();
        copy_btn.set_child(Some(&copy_glyph()));
        copy_btn.add_css_class("flat");
        copy_btn.set_tooltip_text(Some("Copy selection (Ctrl+C)"));
        let c = canvas.clone();
        copy_btn.connect_clicked(move |_| {
            c.copy_selection();
        });
        let paste_btn = gtk::Button::new();
        paste_btn.set_child(Some(&paste_glyph()));
        paste_btn.add_css_class("flat");
        paste_btn.set_tooltip_text(Some("Paste (Ctrl+V)"));
        let c = canvas.clone();
        paste_btn.connect_clicked(move |_| c.paste());
        let image_btn = gtk::Button::new();
        image_btn.set_child(Some(&image_glyph()));
        image_btn.add_css_class("flat");
        image_btn.set_tooltip_text(Some("Insert image from file…"));
        let c = canvas.clone();
        image_btn.connect_clicked(move |b| {
            let c = c.clone();
            let win = b.root().and_downcast::<gtk::Window>();
            glib::spawn_future_local(async move {
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("Images"));
                filter.add_mime_type("image/*");
                for ext in ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "svg"] {
                    filter.add_suffix(ext);
                }
                let filters = gtk4::gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::new();
                dialog.set_title("Insert image");
                dialog.set_filters(Some(&filters));
                dialog.set_default_filter(Some(&filter));
                if let Ok(model) = dialog.open_multiple_future(win.as_ref()).await {
                    let files: Vec<gtk4::gio::File> = (0..model.n_items())
                        .filter_map(|i| model.item(i).and_downcast::<gtk4::gio::File>())
                        .collect();
                    if !files.is_empty() {
                        c.insert_image_files(files, None);
                    }
                }
            });
        });
        tb.widget.append(&copy_btn);
        tb.widget.append(&paste_btn);
        tb.widget.append(&image_btn);
        {
            let t = tb.clone();
            text_btn.connect_clicked(move |b| {
                let editing = t.inner.canvas.is_editing_text();
                if t.inner.mode.get() == Mode::Text || editing {
                    if t.inner.mode.get() != Mode::Text {
                        t.set_mode(Mode::Text);
                    }
                    t.open_text_flyout(b.clone().upcast());
                } else {
                    t.set_mode(Mode::Text);
                }
            });
        }
        tb.widget.append(&text_btn);
        tb.widget.append(&vsep());

        // Gallery: eraser chip first, then pens.
        tb.widget.append(&tb.inner.gallery);

        // Add Pen ▾.
        let add_pen = gtk::MenuButton::new();
        add_pen.add_css_class("flat");
        add_pen.set_icon_name("list-add-symbolic");
        add_pen.set_tooltip_text(Some("Add pen"));
        let pop = gtk::Popover::new();
        let vb = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vb.set_margin_top(4);
        vb.set_margin_bottom(4);
        vb.set_margin_start(4);
        vb.set_margin_end(4);
        for (label, tool, color, width) in [
            ("Pen", Tool::Pen, SemanticColor::Foreground, 3.5),
            ("Pencil", Tool::Pencil, SemanticColor::Foreground, 2.0),
            ("Highlighter", Tool::Highlighter, fixed(0.98, 0.84, 0.25), 16.0),
        ] {
            let b = gtk::Button::with_label(label);
            b.add_css_class("flat");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let t = tb.clone();
            let p = pop.clone();
            b.connect_clicked(move |_| {
                t.inner.pens.borrow_mut().push(PenCfg { tool, color, width });
                let idx = t.inner.pens.borrow().len() - 1;
                t.rebuild_gallery();
                t.set_mode(Mode::Pen(idx));
                t.save();
                p.popdown();
            });
            vb.append(&b);
        }
        pop.set_child(Some(&vb));
        add_pen.set_popover(Some(&pop));
        tb.widget.append(&add_pen);
        tb.widget.append(&vsep());

        // Right-side group: Shapes ▾ / Format Background ▾ / ⋯ — present,
        // wired to "coming soon" popovers until shapes and M5 land.
        // Shapes ▾ — rebuilt each open: shape kind + thickness + colors.
        {
            let t = tb.clone();
            shapes_btn.set_create_popup_func(move |btn| {
                let pop = t.build_shapes_popover();
                btn.set_popover(Some(&pop));
            });
            tb.widget.append(&shapes_btn);
        }

        // Format Background ▾ — rule/grid lines, margin.
        let bg_btn = gtk::MenuButton::new();
        bg_btn.add_css_class("flat");
        bg_btn.set_child(Some(&ruled_page_glyph()));
        bg_btn.set_tooltip_text(Some("Format background"));
        {
            let t = tb.clone();
            bg_btn.set_create_popup_func(move |btn| {
                btn.set_popover(Some(&t.build_background_popover()));
            });
        }
        tb.widget.append(&bg_btn);

        // Invert canvas light/dark.
        let invert_btn = gtk::Button::new();
        invert_btn.add_css_class("flat");
        invert_btn.set_child(Some(&invert_glyph()));
        invert_btn.set_tooltip_text(Some("Invert canvas (light/dark page)"));
        {
            let t = tb.clone();
            invert_btn.connect_clicked(move |_| {
                let now = !t.inner.canvas.inverted();
                t.inner.canvas.set_inverted(now);
                t.save();
            });
        }
        tb.widget.append(&invert_btn);


        tb.rebuild_gallery();
        canvas.set_inverted(st.canvas_inverted);
        // Apply the restored mode.
        tb.set_mode(tb.inner.mode.get());
        tb
    }

    /// Text settings flyout (same pattern as the pen flyout): style toggles,
    /// lists, size and the color grid. Acts on the selection / next typed
    /// text while editing, otherwise sets the defaults for new text.
    fn open_text_flyout(&self, anchor: gtk::Widget) {
        use crate::text::{Fmt, ListKind};
        let canvas = self.inner.canvas.clone();
        let popover = gtk::Popover::new();
        popover.set_parent(&anchor);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
        vbox.set_margin_top(10);
        vbox.set_margin_bottom(10);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        // Style toggles.
        vbox.append(&section_label("Style"));
        let srow = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let toggles: Vec<(gtk::Button, Fmt)> = vec![
            (fmt_button("<b>B</b>", "Bold (Ctrl+B)"), Fmt::Bold),
            (fmt_button("<i>I</i>", "Italic (Ctrl+I)"), Fmt::Italic),
            (fmt_button("<u>U</u>", "Underline (Ctrl+U)"), Fmt::Underline),
            (
                fmt_button("<span background=\"#fadf6b\" foreground=\"#1a1b26\"> ab </span>", "Highlight (Ctrl+Shift+H)"),
                Fmt::Highlight,
            ),
        ];
        let refresh = {
            let canvas = canvas.clone();
            let toggles: Vec<(gtk::Button, Fmt)> = toggles.iter().map(|(b, f)| (b.clone(), *f)).collect();
            std::rc::Rc::new(move || {
                let st = canvas.text_style_state();
                for (b, f) in &toggles {
                    let on = match f {
                        Fmt::Bold => st.bold,
                        Fmt::Italic => st.italic,
                        Fmt::Underline => st.underline,
                        Fmt::Highlight => st.highlight,
                    };
                    set_selected_css(b, on);
                }
            })
        };
        for (b, f) in &toggles {
            let c = canvas.clone();
            let f = *f;
            let r = refresh.clone();
            b.connect_clicked(move |_| {
                c.text_format(f);
                r();
            });
            srow.append(b);
        }
        refresh();
        vbox.append(&srow);

        // Lists.
        vbox.append(&section_label("Lists"));
        let lrow = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let list_btns: Rc<RefCell<Vec<(gtk::Button, ListKind)>>> = Rc::default();
        let refresh_lists = {
            let canvas = canvas.clone();
            let list_btns = list_btns.clone();
            Rc::new(move || {
                let cur = canvas.text_list_kind();
                if std::env::var_os("OMASCRATCH_DEBUG_TEXT").is_some() {
                    eprintln!(
                        "[text] lists: editing={} target={} kind={cur:?}",
                        canvas.is_editing_text(),
                        canvas.text_has_target()
                    );
                }
                for (b, k) in list_btns.borrow().iter() {
                    set_selected_css(b, cur == Some(*k));
                }
            })
        };
        for (label, tip, k) in [
            ("•", "Bulleted list", ListKind::Bullet),
            ("1.", "Numbered list", ListKind::Number),
            ("☐", "Checklist (Ctrl+1)", ListKind::Check),
        ] {
            let b = fmt_button(label, tip);
            b.add_css_class("list-kind-btn");
            list_btns.borrow_mut().push((b.clone(), k));
            let c = canvas.clone();
            let r = refresh_lists.clone();
            let rs = refresh.clone();
            b.connect_clicked(move |_| {
                c.text_list(k);
                r();
                rs();
            });
            lrow.append(&b);
        }
        refresh_lists();
        vbox.append(&lrow);

        // Size.
        vbox.append(&section_label("Size"));
        let zrow = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let smaller = fmt_button("A−", "Smaller");
        let size_lbl = gtk::Label::new(Some(&format!("{:.0}", canvas.text_font_size())));
        size_lbl.set_width_chars(3);
        let larger = fmt_button("A+", "Larger");
        for (b, up) in [(&smaller, false), (&larger, true)] {
            let c = canvas.clone();
            let l = size_lbl.clone();
            b.connect_clicked(move |_| {
                if let Some(sz) = c.text_font_step(up) {
                    l.set_text(&format!("{sz:.0}"));
                }
            });
        }
        zrow.append(&smaller);
        zrow.append(&size_lbl);
        zrow.append(&larger);
        vbox.append(&zrow);

        // Colors (same grid as the pens). "Theme ink" clears the override.
        vbox.append(&section_label("Color"));
        let grid_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let swatches: Swatches = Rc::default();
        for row in color_rows() {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            for (name, color) in row {
                let b = gtk::Button::new();
                b.add_css_class("flat");
                b.add_css_class("swatch-btn");
                b.set_focus_on_click(false);
                b.set_child(Some(&color_swatch(rgba_of(&canvas, color), 22)));
                b.set_tooltip_text(Some(name));
                let c = canvas.clone();
                let value = if color == SemanticColor::Foreground { None } else { Some(color) };
                swatches.borrow_mut().push((b.clone(), value));
                let sw = swatches.clone();
                b.connect_clicked(move |_| {
                    c.text_set_color(value);
                    mark_selected(&sw, value);
                });
                r.append(&b);
            }
            grid_box.append(&r);
        }
        vbox.append(&grid_box);
        mark_selected(&swatches, canvas.text_style_state().color);

        popover.set_child(Some(&vbox));
        {
            let c = canvas.clone();
            popover.connect_closed(move |p| {
                c.focus_text_editor();
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
        }
        popover.popup();
    }

    /// Switch to the Select tool (e.g. right after pasting an image).
    pub fn select_tool(&self) {
        self.set_mode(Mode::Select);
    }

    /// Stylus barrel button: flip between the eraser and the previous tool.
    pub fn toggle_eraser(&self) {
        if self.inner.mode.get() == Mode::Eraser {
            self.set_mode(self.inner.prev_mode.get());
        } else {
            self.set_mode(Mode::Eraser);
        }
    }

    /// Re-render palette-dependent glyphs after a theme change.
    pub fn refresh_theme(&self) {
        self.rebuild_gallery();
    }

    /// App hook: persist a background change into the open note.
    pub fn set_on_background(&self, f: impl Fn(PageBackground) + 'static) {
        *self.inner.on_background.borrow_mut() = Some(Box::new(f));
    }

    fn apply_background(&self, bg: PageBackground) {
        self.inner.canvas.set_background(bg);
        if let Some(cb) = self.inner.on_background.borrow().as_ref() {
            cb(bg);
        }
    }

    /// Format Background flyout: rule-line and grid styles plus a margin toggle.
    fn build_background_popover(&self) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_top(8);
        vbox.set_margin_bottom(8);
        vbox.set_margin_start(8);
        vbox.set_margin_end(8);
        let cur = self.inner.canvas.background();

        let add = |label: String, selected: bool, bg: PageBackground, pop: &gtk::Popover| {
            let text = if selected { format!("✓ {label}") } else { format!("   {label}") };
            let b = gtk::Button::with_label(&text);
            b.add_css_class("flat");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let t = self.clone();
            let pop = pop.clone();
            b.connect_clicked(move |_| {
                t.apply_background(bg);
                pop.popdown();
            });
            b
        };

        vbox.append(&section_label("Rule lines"));
        let none = PageBackground { kind: BackgroundKind::None, spacing: cur.spacing, margin: cur.margin };
        vbox.append(&add("None".into(), cur.kind == BackgroundKind::None, none, &popover));
        for (name, sp) in [("Narrow", 24.0), ("Standard", 32.0), ("Wide", 44.0)] {
            let bg = PageBackground { kind: BackgroundKind::Rules, spacing: sp, margin: cur.margin };
            let sel = cur.kind == BackgroundKind::Rules && (cur.spacing - sp).abs() < 0.1;
            vbox.append(&add(name.to_string(), sel, bg, &popover));
        }
        vbox.append(&section_label("Grid"));
        for (name, sp) in [("Small", 20.0), ("Medium", 32.0), ("Large", 48.0)] {
            let bg = PageBackground { kind: BackgroundKind::Grid, spacing: sp, margin: cur.margin };
            let sel = cur.kind == BackgroundKind::Grid && (cur.spacing - sp).abs() < 0.1;
            vbox.append(&add(name.to_string(), sel, bg, &popover));
        }
        vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let margin_bg = PageBackground { margin: !cur.margin, ..cur };
        vbox.append(&add("Margin line".into(), cur.margin, margin_bg, &popover));

        popover.set_child(Some(&vbox));
        popover
    }

    // -- mode handling (one active mode across select/lasso/eraser/pens) --

    fn set_mode(&self, mode: Mode) {
        if mode != Mode::Eraser {
            self.inner.prev_mode.set(mode);
        }
        self.inner.mode.set(mode);
        match mode {
            Mode::Select => {
                self.inner.canvas.set_active_tool(ActiveTool::Select);
            }
            Mode::Lasso => {
                self.inner.canvas.set_active_tool(ActiveTool::Lasso);
            }
            Mode::Pan => {
                self.inner.canvas.set_active_tool(ActiveTool::Pan);
            }
            Mode::Text => {
                self.inner.canvas.set_active_tool(ActiveTool::Text);
            }
            Mode::Shape(kind) => {
                self.inner.last_shape.set(kind);
                self.inner.canvas.set_shape_tool(kind);
                self.inner
                    .canvas
                    .set_draw_style(*self.inner.shape_color.borrow(), self.inner.shape_width.get());
            }
            Mode::Eraser => {
                let kind = if self.inner.eraser_area.get() { EraserKind::Area } else { EraserKind::Stroke };
                self.inner.canvas.set_eraser(kind, self.inner.eraser_radius.get());
            }
            Mode::Pen(i) => {
                if let Some(p) = self.inner.pens.borrow().get(i).copied() {
                    self.inner.canvas.set_pen(p.tool, p.color, p.width);
                }
            }
        }
        self.refresh_mode_styles();
        self.save();
    }

    fn refresh_mode_styles(&self) {
        let mode = self.inner.mode.get();
        set_active_css(&self.inner.select_btn, mode == Mode::Select);
        set_active_css(&self.inner.lasso_btn, mode == Mode::Lasso);
        set_active_css(&self.inner.pan_btn, mode == Mode::Pan);
        set_active_css(&self.inner.text_btn, mode == Mode::Text);
        if matches!(mode, Mode::Shape(_)) {
            self.inner.shapes_btn.add_css_class("mode-active");
        } else {
            self.inner.shapes_btn.remove_css_class("mode-active");
        }
        // Gallery children: index 0 is the eraser chip, 1.. are pens.
        let mut idx: i32 = -1;
        let mut child = self.inner.gallery.first_child();
        while let Some(w) = child {
            idx += 1;
            let active = match (idx, mode) {
                (0, Mode::Eraser) => true,
                (i, Mode::Pen(p)) if i >= 1 => (i - 1) as usize == p,
                _ => false,
            };
            if active {
                w.add_css_class("pen-active");
            } else {
                w.remove_css_class("pen-active");
            }
            child = w.next_sibling();
        }
    }

    // -- gallery --

    fn rebuild_gallery(&self) {
        let g = &self.inner.gallery;
        while let Some(c) = g.first_child() {
            g.remove(&c);
        }

        // Eraser chip (first, like OneNote's fluid toolbar).
        let eraser = gtk::Button::new();
        eraser.add_css_class("flat");
        eraser.add_css_class("pen-chip");
        eraser.set_child(Some(&eraser_glyph()));
        eraser.set_tooltip_text(Some("Eraser — click again for eraser options"));
        let t = self.clone();
        eraser.connect_clicked(move |b| {
            if t.inner.mode.get() == Mode::Eraser {
                t.open_eraser_flyout(b.clone().upcast());
            } else {
                t.set_mode(Mode::Eraser);
            }
        });
        g.append(&eraser);

        // Pen chips.
        let pens = self.inner.pens.borrow().clone();
        for (idx, cfg) in pens.into_iter().enumerate() {
            let btn = gtk::Button::new();
            btn.add_css_class("flat");
            btn.add_css_class("pen-chip");
            btn.set_child(Some(&pen_glyph(cfg, rgba_of(&self.inner.canvas, cfg.color))));
            btn.set_tooltip_text(Some(match cfg.tool {
                Tool::Pencil => "Pencil — click again for thickness & color · hold or right-click to drag",
                Tool::Highlighter => "Highlighter — click again for thickness & color · hold or right-click to drag",
                _ => "Pen — click again for thickness & color · hold or right-click to drag",
            }));
            let t = self.clone();
            btn.connect_clicked(move |b| {
                if t.inner.suppress_click.replace(false) {
                    return;
                }
                if t.inner.mode.get() == Mode::Pen(idx) {
                    t.open_pen_flyout(idx, b.clone().upcast());
                } else {
                    t.set_mode(Mode::Pen(idx));
                }
            });
            self.attach_reorder(&btn, idx);
            g.append(&btn);
        }
        self.refresh_mode_styles();
    }

    // -- pen reordering (long-press + drag, or right-click drag) --

    fn attach_reorder(&self, btn: &gtk::Button, idx: usize) {
        // Long press arms the chip; it lifts to show it can be dragged now.
        let long = gtk::GestureLongPress::new();
        let t = self.clone();
        let b = btn.clone();
        long.connect_pressed(move |_, _, _| {
            t.inner.reorder_armed.set(Some(idx));
            t.inner.suppress_click.set(true);
            b.add_css_class("pen-armed");
        });
        let t = self.clone();
        let b = btn.clone();
        long.connect_end(move |_, _| {
            // Released without dragging: disarm (the drag path clears too).
            if t.inner.drag_src.get().is_none() {
                t.inner.reorder_armed.set(None);
                b.remove_css_class("pen-armed");
            }
        });
        btn.add_controller(long);

        // Primary-button drag: only allowed once armed by the long press.
        // Secondary-button (right-click) drag: always allowed.
        for button in [gdk::BUTTON_PRIMARY, gdk::BUTTON_SECONDARY] {
            let source = gtk::DragSource::new();
            source.set_button(button);
            source.set_actions(gdk::DragAction::MOVE);
            let t = self.clone();
            let b = btn.clone();
            source.connect_prepare(move |src, _, _| {
                let allowed = button == gdk::BUTTON_SECONDARY
                    || t.inner.reorder_armed.get() == Some(idx);
                if !allowed {
                    return None;
                }
                t.inner.drag_src.set(Some(idx));
                t.inner.suppress_click.set(false);
                src.set_icon(Some(&gtk::WidgetPaintable::new(Some(&b))), 12, 20);
                Some(gdk::ContentProvider::for_value(&"omascratch-pen".to_value()))
            });
            let t = self.clone();
            let b = btn.clone();
            source.connect_drag_end(move |_, _, _| {
                t.inner.drag_src.set(None);
                t.inner.reorder_armed.set(None);
                b.remove_css_class("pen-armed");
                t.clear_drop_marks();
            });
            btn.add_controller(source);
        }

        // Every pen chip is a drop target: left half = before, right = after.
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let t = self.clone();
        let b = btn.clone();
        target.connect_motion(move |_, x, _| {
            t.clear_drop_marks();
            let after = x > b.width() as f64 / 2.0;
            b.add_css_class(if after { "drop-after" } else { "drop-before" });
            gdk::DragAction::MOVE
        });
        let t = self.clone();
        target.connect_leave(move |_| t.clear_drop_marks());
        let t = self.clone();
        let b = btn.clone();
        target.connect_drop(move |_, _, x, _| {
            let Some(src) = t.inner.drag_src.take() else { return false };
            let after = x > b.width() as f64 / 2.0;
            t.inner.reorder_armed.set(None);
            t.clear_drop_marks();
            // Defer the rebuild: it destroys the widgets whose drag/drop
            // signals are still on the stack.
            let t2 = t.clone();
            glib::idle_add_local_once(move || t2.move_pen(src, idx, after));
            true
        });
        btn.add_controller(target);
    }

    fn clear_drop_marks(&self) {
        let mut child = self.inner.gallery.first_child();
        while let Some(w) = child {
            w.remove_css_class("drop-before");
            w.remove_css_class("drop-after");
            child = w.next_sibling();
        }
    }

    /// Move pen `src` next to pen `target` (before or after), keeping the
    /// active pen (and the eraser-toggle return target) pointing at the same
    /// pen after the shuffle.
    fn move_pen(&self, src: usize, target: usize, after: bool) {
        let len = self.inner.pens.borrow().len();
        if src >= len || target >= len {
            return;
        }
        let mut dst = if after { target + 1 } else { target };
        if src < dst {
            dst -= 1;
        }
        if dst == src {
            return;
        }
        {
            let mut pens = self.inner.pens.borrow_mut();
            let pen = pens.remove(src);
            pens.insert(dst, pen);
        }
        let remap = |i: usize| -> usize {
            if i == src {
                return dst;
            }
            let i = if i > src { i - 1 } else { i };
            if i >= dst { i + 1 } else { i }
        };
        if let Mode::Pen(i) = self.inner.mode.get() {
            self.inner.mode.set(Mode::Pen(remap(i)));
        }
        if let Mode::Pen(i) = self.inner.prev_mode.get() {
            self.inner.prev_mode.set(Mode::Pen(remap(i)));
        }
        self.rebuild_gallery();
        self.save();
    }

    // -- pen flyout (OneNote: preview, thickness, recent, colors, more, remove) --

    fn open_pen_flyout(&self, idx: usize, anchor: gtk::Widget) {
        let Some(cfg0) = self.inner.pens.borrow().get(idx).copied() else { return };
        let cfg = Rc::new(RefCell::new(cfg0));

        let popover = gtk::Popover::new();
        popover.set_parent(&anchor);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
        vbox.set_margin_top(10);
        vbox.set_margin_bottom(10);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        // Live stroke preview.
        let preview = gtk::DrawingArea::new();
        preview.set_content_width(190);
        preview.set_content_height(30);
        {
            let cfg = cfg.clone();
            let canvas = self.inner.canvas.clone();
            preview.set_draw_func(move |_, cr, w, h| {
                let c = *cfg.borrow();
                let rgba = rgba_of(&canvas, c.color);
                let alpha = if matches!(c.tool, Tool::Highlighter) { 0.55 } else { 1.0 };
                set_source(cr, rgba, alpha);
                cr.set_line_width(c.width.min(h as f64 * 0.8));
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                let (w, h) = (w as f64, h as f64);
                cr.move_to(10.0, h * 0.62);
                cr.curve_to(w * 0.35, h * 0.15, w * 0.6, h * 0.95, w - 10.0, h * 0.42);
                let _ = cr.stroke();
            });
        }
        vbox.append(&preview);

        // Thickness: − [dots] +
        let tlabel = section_label("Thickness");
        vbox.append(&tlabel);
        let trow = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let widths: [f64; 5] = if matches!(cfg0.tool, Tool::Highlighter) {
            [8.0, 12.0, 16.0, 24.0, 36.0]
        } else {
            [1.0, 2.0, 3.5, 6.0, 10.0]
        };
        let minus = small_label_button("−");
        trow.append(&minus);
        for w in widths {
            let dot = gtk::Button::new();
            dot.add_css_class("flat");
            let d = (5.0_f64 + w * 1.4).min(24.0) as i32;
            let dot_area = gtk::DrawingArea::new();
            dot_area.set_content_width(24);
            dot_area.set_content_height(24);
            let cfg_for_draw = cfg.clone();
            let canvas_for_draw = self.inner.canvas.clone();
            dot_area.set_draw_func(move |_, cr, aw, ah| {
                let c = *cfg_for_draw.borrow();
                let rgba = rgba_of(&canvas_for_draw, c.color);
                set_source(cr, rgba, 1.0);
                cr.arc(aw as f64 / 2.0, ah as f64 / 2.0, d as f64 / 2.0, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
                if (c.width - w).abs() < 0.01 {
                    cr.set_source_rgb(0.48, 0.64, 0.97);
                    cr.set_line_width(1.5);
                    cr.arc(aw as f64 / 2.0, ah as f64 / 2.0, (aw as f64 / 2.0) - 1.5, 0.0, std::f64::consts::TAU);
                    let _ = cr.stroke();
                }
            });
            dot.set_child(Some(&dot_area));
            let t = self.clone();
            let cfg2 = cfg.clone();
            let preview2 = preview.clone();
            let trow2 = trow.clone();
            dot.connect_clicked(move |_| {
                cfg2.borrow_mut().width = w;
                t.commit_pen(idx, &cfg2, &preview2);
                redraw_children(&trow2);
            });
            trow.append(&dot);
        }
        let plus = small_label_button("+");
        trow.append(&plus);
        vbox.append(&trow);

        // − / + steppers.
        {
            let t = self.clone();
            let cfg2 = cfg.clone();
            let preview2 = preview.clone();
            let trow2 = trow.clone();
            minus.connect_clicked(move |_| {
                let w = (cfg2.borrow().width * 0.8).max(0.5);
                cfg2.borrow_mut().width = w;
                t.commit_pen(idx, &cfg2, &preview2);
                redraw_children(&trow2);
            });
        }
        {
            let t = self.clone();
            let cfg2 = cfg.clone();
            let preview2 = preview.clone();
            let trow2 = trow.clone();
            plus.connect_clicked(move |_| {
                let w = (cfg2.borrow().width * 1.25).min(60.0);
                cfg2.borrow_mut().width = w;
                t.commit_pen(idx, &cfg2, &preview2);
                redraw_children(&trow2);
            });
        }

        // Recent colors.
        let swatches: Swatches = Rc::default();
        let recent = self.inner.recent.borrow().clone();
        if !recent.is_empty() {
            vbox.append(&section_label("Recent Colors"));
            let rrow = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            for color in recent.into_iter().take(7) {
                rrow.append(&self.color_button(color, idx, &cfg, &preview, &trow, &swatches));
            }
            vbox.append(&rrow);
        }

        // Colors grid.
        vbox.append(&section_label("Colors"));
        let grid_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        for row in color_rows() {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            for (name, color) in row {
                let b = self.color_button(color, idx, &cfg, &preview, &trow, &swatches);
                b.set_tooltip_text(Some(name));
                r.append(&b);
            }
            grid_box.append(&r);
        }
        vbox.append(&grid_box);
        mark_selected(&swatches, Some(cfg0.color));


        // Remove pen.
        if self.inner.pens.borrow().len() > 1 {
            vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let remove = gtk::Button::with_label("Remove Pen");
            remove.add_css_class("flat");
            remove.add_css_class("destructive-action");
            if let Some(l) = remove.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let t = self.clone();
            let pop = popover.clone();
            remove.connect_clicked(move |_| {
                {
                    let mut pens = t.inner.pens.borrow_mut();
                    if pens.len() > 1 && idx < pens.len() {
                        pens.remove(idx);
                    }
                }
                pop.popdown();
                t.rebuild_gallery();
                t.set_mode(Mode::Pen(0));
                t.save();
            });
            vbox.append(&remove);
        }

        popover.set_child(Some(&vbox));
        popover.popup();
    }

    /// Shapes flyout: kind picker, thickness dots and the color grid, all
    /// bound to the shape tool's own persistent style.
    fn build_shapes_popover(&self) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
        vbox.set_margin_top(10);
        vbox.set_margin_bottom(10);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let areas: Rc<RefCell<Vec<gtk::DrawingArea>>> = Rc::new(RefCell::new(Vec::new()));

        vbox.append(&section_label("Shape"));
        let krow = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for (tip, kind) in [
            ("Line", ShapeKind::Line),
            ("Arrow", ShapeKind::Arrow),
            ("Rectangle", ShapeKind::Rect),
            ("Ellipse", ShapeKind::Ellipse),
        ] {
            let b = gtk::Button::new();
            b.add_css_class("flat");
            b.set_tooltip_text(Some(tip));
            let icon = self.shape_icon(kind);
            areas.borrow_mut().push(icon.clone());
            b.set_child(Some(&icon));
            let t = self.clone();
            let pop = popover.clone();
            b.connect_clicked(move |_| {
                t.set_mode(Mode::Shape(kind));
                pop.popdown();
            });
            krow.append(&b);
        }
        vbox.append(&krow);

        vbox.append(&section_label("Thickness"));
        let trow = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for w in [1.5_f64, 2.5, 3.5, 6.0, 10.0] {
            let dot = gtk::Button::new();
            dot.add_css_class("flat");
            let area = gtk::DrawingArea::new();
            area.set_content_width(24);
            area.set_content_height(24);
            let t = self.clone();
            area.set_draw_func(move |_, cr, aw, ah| {
                let rgba = rgba_of(&t.inner.canvas, *t.inner.shape_color.borrow());
                set_source(cr, rgba, 1.0);
                let d = (5.0_f64 + w * 1.4).min(22.0);
                cr.arc(aw as f64 / 2.0, ah as f64 / 2.0, d / 2.0, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
                if (t.inner.shape_width.get() - w).abs() < 0.01 {
                    cr.set_source_rgb(0.48, 0.64, 0.97);
                    cr.set_line_width(1.5);
                    cr.arc(aw as f64 / 2.0, ah as f64 / 2.0, (aw as f64 / 2.0) - 1.5, 0.0, std::f64::consts::TAU);
                    let _ = cr.stroke();
                }
            });
            areas.borrow_mut().push(area.clone());
            dot.set_child(Some(&area));
            let t = self.clone();
            let areas2 = areas.clone();
            dot.connect_clicked(move |_| {
                t.inner.shape_width.set(w);
                t.apply_shape_style();
                for a in areas2.borrow().iter() {
                    a.queue_draw();
                }
            });
            trow.append(&dot);
        }
        vbox.append(&trow);

        vbox.append(&section_label("Colors"));
        let grid_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let swatches: Swatches = Rc::default();
        for row in color_rows() {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            for (name, color) in row {
                let b = gtk::Button::new();
                b.add_css_class("flat");
                b.add_css_class("swatch-btn");
                b.set_child(Some(&color_swatch(rgba_of(&self.inner.canvas, color), 22)));
                b.set_tooltip_text(Some(name));
                swatches.borrow_mut().push((b.clone(), Some(color)));
                let t = self.clone();
                let areas2 = areas.clone();
                let sw = swatches.clone();
                b.connect_clicked(move |_| {
                    *t.inner.shape_color.borrow_mut() = color;
                    t.apply_shape_style();
                    for a in areas2.borrow().iter() {
                        a.queue_draw();
                    }
                    mark_selected(&sw, Some(color));
                });
                r.append(&b);
            }
            grid_box.append(&r);
        }
        vbox.append(&grid_box);
        mark_selected(&swatches, Some(*self.inner.shape_color.borrow()));

        popover.set_child(Some(&vbox));
        popover
    }

    fn apply_shape_style(&self) {
        if matches!(self.inner.mode.get(), Mode::Shape(_)) {
            self.inner
                .canvas
                .set_draw_style(*self.inner.shape_color.borrow(), self.inner.shape_width.get());
        }
        self.save();
    }

    /// Small drawn icon for a shape kind, in the current shape color; the
    /// currently selected kind gets an accent ring.
    fn shape_icon(&self, kind: ShapeKind) -> gtk::DrawingArea {
        let area = gtk::DrawingArea::new();
        area.set_content_width(26);
        area.set_content_height(26);
        let t = self.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let w = w as f64;
            let h = h as f64;
            let rgba = rgba_of(&t.inner.canvas, *t.inner.shape_color.borrow());
            set_source(cr, rgba, 1.0);
            cr.set_line_width(1.8);
            cr.set_line_cap(gtk::cairo::LineCap::Round);
            match kind {
                ShapeKind::Line => {
                    cr.move_to(w * 0.18, h * 0.78);
                    cr.line_to(w * 0.82, h * 0.22);
                    let _ = cr.stroke();
                }
                ShapeKind::Arrow => {
                    cr.move_to(w * 0.18, h * 0.78);
                    cr.line_to(w * 0.78, h * 0.26);
                    let _ = cr.stroke();
                    cr.move_to(w * 0.56, h * 0.24);
                    cr.line_to(w * 0.80, h * 0.24);
                    cr.line_to(w * 0.80, h * 0.48);
                    let _ = cr.stroke();
                }
                ShapeKind::Rect => {
                    cr.rectangle(w * 0.18, h * 0.26, w * 0.64, h * 0.48);
                    let _ = cr.stroke();
                }
                ShapeKind::Ellipse => {
                    let _ = cr.save();
                    cr.translate(w / 2.0, h / 2.0);
                    cr.scale(1.0, 0.68);
                    cr.arc(0.0, 0.0, w * 0.32, 0.0, std::f64::consts::TAU);
                    let _ = cr.restore();
                    let _ = cr.stroke();
                }
            }
            if t.inner.mode.get() == Mode::Shape(kind) {
                cr.set_source_rgb(0.48, 0.64, 0.97);
                cr.set_line_width(1.2);
                rounded_rect(cr, 1.0, 1.0, w - 2.0, h - 2.0, 5.0);
                let _ = cr.stroke();
            }
        });
        area
    }

    fn color_button(
        &self,
        color: SemanticColor,
        idx: usize,
        cfg: &Rc<RefCell<PenCfg>>,
        preview: &gtk::DrawingArea,
        trow: &gtk::Box,
        swatches: &Swatches,
    ) -> gtk::Button {
        let b = gtk::Button::new();
        b.add_css_class("flat");
        b.add_css_class("swatch-btn");
        b.set_child(Some(&color_swatch(rgba_of(&self.inner.canvas, color), 22)));
        swatches.borrow_mut().push((b.clone(), Some(color)));
        let t = self.clone();
        let cfg = cfg.clone();
        let preview = preview.clone();
        let trow = trow.clone();
        let sw = swatches.clone();
        b.connect_clicked(move |_| {
            cfg.borrow_mut().color = color;
            t.push_recent(color);
            t.commit_pen(idx, &cfg, &preview);
            redraw_children(&trow);
            mark_selected(&sw, Some(color));
        });
        b
    }

    /// Write the edited cfg back to the pen list, apply it to the canvas,
    /// refresh the chip glyph and the flyout preview, persist.
    fn commit_pen(&self, idx: usize, cfg: &Rc<RefCell<PenCfg>>, preview: &gtk::DrawingArea) {
        let c = *cfg.borrow();
        if let Some(p) = self.inner.pens.borrow_mut().get_mut(idx) {
            *p = c;
        }
        self.inner.canvas.set_pen(c.tool, c.color, c.width);
        preview.queue_draw();
        // Update the chip glyph in place (child index = idx + 1; eraser is 0).
        let mut i: i32 = -1;
        let mut child = self.inner.gallery.first_child();
        while let Some(w) = child {
            i += 1;
            if i == idx as i32 + 1 {
                if let Some(btn) = w.downcast_ref::<gtk::Button>() {
                    btn.set_child(Some(&pen_glyph(c, rgba_of(&self.inner.canvas, c.color))));
                }
                break;
            }
            child = w.next_sibling();
        }
        self.save();
    }

    fn push_recent(&self, color: SemanticColor) {
        let mut r = self.inner.recent.borrow_mut();
        r.retain(|c| *c != color);
        r.insert(0, color);
        r.truncate(7);
    }

    // -- eraser flyout --

    fn open_eraser_flyout(&self, anchor: gtk::Widget) {
        let popover = gtk::Popover::new();
        popover.set_parent(&anchor);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_top(8);
        vbox.set_margin_bottom(8);
        vbox.set_margin_start(8);
        vbox.set_margin_end(8);
        vbox.append(&section_label("Eraser"));
        let options: [(&str, bool, f64); 4] = [
            ("Stroke eraser", false, 16.0),
            ("Small", true, 8.0),
            ("Medium", true, 16.0),
            ("Large", true, 28.0),
        ];
        let cur_area = self.inner.eraser_area.get();
        let cur_r = self.inner.eraser_radius.get();
        for (label, area, radius) in options {
            let selected = cur_area == area && (!area || (cur_r - radius).abs() < 0.1);
            let text = if selected { format!("✓ {label}") } else { format!("   {label}") };
            let b = gtk::Button::with_label(&text);
            b.add_css_class("flat");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let t = self.clone();
            let pop = popover.clone();
            b.connect_clicked(move |_| {
                t.inner.eraser_area.set(area);
                t.inner.eraser_radius.set(radius);
                t.set_mode(Mode::Eraser);
                pop.popdown();
            });
            vbox.append(&b);
        }
        popover.set_child(Some(&vbox));
        popover.popup();
    }

    // -- persistence --

    fn save(&self) {
        let active_pen = match self.inner.mode.get() {
            Mode::Pen(i) => i,
            _ => 0,
        };
        let st = ToolbarState {
            schema: 1,
            pens: self.inner.pens.borrow().clone(),
            active_pen,
            eraser_area: self.inner.eraser_area.get(),
            eraser_radius: self.inner.eraser_radius.get(),
            recent: self.inner.recent.borrow().clone(),
            shape_color: *self.inner.shape_color.borrow(),
            shape_width: self.inner.shape_width.get(),
            canvas_inverted: self.inner.canvas.inverted(),
        };
        let dir = store::state_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(bytes) = serde_json::to_vec_pretty(&st) {
            let _ = store::atomic::atomic_write(&dir.join(STATE_FILE), &bytes);
        }
    }
}

fn load_state() -> ToolbarState {
    let path = store::state_dir().join(STATE_FILE);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<ToolbarState>(&t).ok())
        .filter(|s| !s.pens.is_empty())
        .unwrap_or_default()
}

/// Swatch buttons in a flyout with the color each one represents
/// (None = theme ink / default), so the current one can be outlined.
type Swatches = Rc<RefCell<Vec<(gtk::Button, Option<SemanticColor>)>>>;

fn mark_selected(swatches: &Swatches, current: Option<SemanticColor>) {
    for (b, c) in swatches.borrow().iter() {
        set_selected_css(b, *c == current);
    }
}

/// Accent border around the chosen option in a flyout.
fn set_selected_css(w: &gtk::Button, selected: bool) {
    if selected {
        w.add_css_class("option-selected");
    } else {
        w.remove_css_class("option-selected");
    }
}

fn set_active_css(w: &gtk::Button, active: bool) {
    if active {
        w.add_css_class("mode-active");
    } else {
        w.remove_css_class("mode-active");
    }
}

fn redraw_children(row: &gtk::Box) {
    let mut child = row.first_child();
    while let Some(w) = child {
        w.queue_draw();
        if let Some(inner) = w.first_child() {
            inner.queue_draw();
        }
        child = w.next_sibling();
    }
}

fn section_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("dim-label");
    l
}

fn small_label_button(text: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("flat");
    b
}

fn vsep() -> gtk::Separator {
    let s = gtk::Separator::new(gtk::Orientation::Vertical);
    s.set_margin_top(8);
    s.set_margin_bottom(8);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders pencil + highlighter + pen chips (PPM) to $OMASCRATCH_GLYPH_PPM for review.
    #[test]
    fn render_pen_glyphs() {
        let Some(out) = std::env::var_os("OMASCRATCH_GLYPH_PPM") else { return };
        let surf = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 26 * 4 * 4, 44 * 4).unwrap();
        let cr = gtk::cairo::Context::new(&surf).unwrap();
        cr.scale(4.0, 4.0);
        cr.set_source_rgb(0.1, 0.1, 0.14);
        let _ = cr.paint();
        for (i, (tool, c)) in [
            (Tool::Pencil, gdk::RGBA::new(0.4, 0.7, 0.5, 1.0)),
            (Tool::Highlighter, gdk::RGBA::new(0.98, 0.84, 0.25, 1.0)),
            (Tool::Pen, gdk::RGBA::new(0.6, 0.5, 0.9, 1.0)),
        ]
        .into_iter()
        .enumerate()
        {
            let _ = cr.save();
            cr.translate(26.0 * i as f64, 0.0);
            draw_pen(&cr, tool, c, 26.0, 44.0);
            let _ = cr.restore();
        }
        let _ = cr.save();
        cr.translate(26.0 * 3.0, 0.0);
        draw_eraser(&cr, 26.0, 44.0);
        let _ = cr.restore();
        drop(cr);
        // Raw BGRA -> binary PPM (no PNG support in this cairo build).
        let (w, h, stride) = (surf.width() as usize, surf.height() as usize, surf.stride() as usize);
        let mut surf = surf;
        let data = surf.data().unwrap();
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        for y in 0..h {
            for x in 0..w {
                let i = y * stride + x * 4;
                ppm.extend_from_slice(&[data[i + 2], data[i + 1], data[i]]);
            }
        }
        std::fs::write(out, ppm).unwrap();
    }
}
