//! CanvasView: the infinite-canvas ink widget.
//!
//! UI-boundary rules: this widget owns only transient state (live stroke
//! buffer, viewport, node cache). Authoritative content lives in the
//! `NoteSession` (core). Input handlers only buffer samples and queue a
//! redraw; geometry runs once per frame in `snapshot()`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Instant, SystemTime};

use gtk4 as gtk;
use gtk4::{gdk, gio, glib, graphene, gsk, prelude::*, subclass::prelude::*};
use kurbo::PathEl;

use omascratch_core::{
    BackgroundKind, Command, ImageId, ImageItem, InkPoint, NoteSession, PageBackground, ParaKind,
    Rgba, SemanticColor, Stroke, StrokeId, TextBox, TextId, Tool,
};
use crate::text::{self as textmod, TextEditor, TextLayout};
use std::path::PathBuf;

// M1 fixed palette (Tokyo Night-ish). Replaced by the Omarchy theme adapter in M5.
const BG: gdk::RGBA = gdk::RGBA::new(0.102, 0.106, 0.149, 1.0);
const BG_LIGHT: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
const INK: gdk::RGBA = gdk::RGBA::new(0.753, 0.792, 0.961, 1.0);
const INK_DARK: gdk::RGBA = gdk::RGBA::new(0.14, 0.15, 0.22, 1.0);
const ACCENT: gdk::RGBA = gdk::RGBA::new(0.478, 0.635, 0.968, 1.0);
const RULE_DARK: gdk::RGBA = gdk::RGBA::new(0.26, 0.29, 0.40, 0.85);
const RULE_LIGHT: gdk::RGBA = gdk::RGBA::new(0.55, 0.63, 0.80, 0.75);
const MARGIN_DARK: gdk::RGBA = gdk::RGBA::new(0.72, 0.36, 0.42, 0.85);
const MARGIN_LIGHT: gdk::RGBA = gdk::RGBA::new(0.84, 0.36, 0.42, 0.85);
const DEBUG_TEXT: gdk::RGBA = gdk::RGBA::new(1.0, 0.62, 0.39, 1.0);
const DEBUG_BG: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.55);

/// Live canvas colors, derived from the Omarchy palette by the theme manager.
#[derive(Clone, Copy)]
pub struct CanvasPalette {
    pub bg: gdk::RGBA,
    pub bg_inv: gdk::RGBA,
    pub ink: gdk::RGBA,
    pub ink_inv: gdk::RGBA,
    pub accent: gdk::RGBA,
    pub rule: gdk::RGBA,
    pub rule_inv: gdk::RGBA,
    pub margin: gdk::RGBA,
    pub margin_inv: gdk::RGBA,
}

impl Default for CanvasPalette {
    fn default() -> Self {
        Self {
            bg: BG,
            bg_inv: BG_LIGHT,
            ink: INK,
            ink_inv: INK_DARK,
            accent: ACCENT,
            rule: RULE_DARK,
            rule_inv: RULE_LIGHT,
            margin: MARGIN_DARK,
            margin_inv: MARGIN_LIGHT,
        }
    }
}

const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 16.0;

/// Which tool the pointer currently drives. Pen/Pencil/Highlighter carry the
/// current color and width in the canvas state (set from the selected preset).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActiveTool {
    Pen,
    Pencil,
    Highlighter,
    Eraser,
    /// Pointer: click a stroke to select it, drag a selection to move it.
    Select,
    /// Freehand lasso: encircled strokes become the selection.
    Lasso,
    /// Drag out a shape (kind held in `State::shape_kind`).
    Shape,
    /// Hand tool: drag anywhere to pan the canvas.
    Pan,
    /// Text tool: click to create or edit a text box.
    Text,
}

impl ActiveTool {
    fn as_draw_tool(self) -> Option<Tool> {
        match self {
            ActiveTool::Pen => Some(Tool::Pen),
            ActiveTool::Pencil => Some(Tool::Pencil),
            ActiveTool::Highlighter => Some(Tool::Highlighter),
            _ => None,
        }
    }
}

/// OneNote eraser types: stroke eraser removes whole strokes; area erasers
/// rub out samples, splitting strokes into fragments.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EraserKind {
    Stroke,
    Area,
}

fn tool_to_active(tool: Tool) -> ActiveTool {
    match tool {
        Tool::Pen => ActiveTool::Pen,
        Tool::Pencil => ActiveTool::Pencil,
        Tool::Highlighter => ActiveTool::Highlighter,
        Tool::Shape => ActiveTool::Shape,
    }
}

/// A text box open in the overlay editor. `original` is the last committed
/// version (None for a brand-new box).
struct EditSession {
    original: Option<TextBox>,
    working: TextBox,
    last_view: (kurbo::Vec2, f64),
}

/// An in-progress corner-handle resize: proportional scale about `anchor`
/// (the opposite corner) by `factor`.
#[derive(Clone, Copy)]
struct ResizeDrag {
    anchor: kurbo::Point,
    start: kurbo::Point,
    factor: f64,
}

struct LiveStroke {
    tool: Tool,
    color: SemanticColor,
    width: f64,
    points: Vec<InkPoint>,
    started: Instant,
    t0_ms: u64,
    /// True for mouse input (constant synthetic pressure).
    simulate_pressure: bool,
}

#[derive(Default)]
struct DebugStats {
    enabled: bool,
    last_pressure: f64,
    min_pressure: f64,
    max_pressure: f64,
    last_tilt: (f64, f64),
    tool_name: String,
    buttons: String,
    sample_times: VecDeque<Instant>,
}

pub struct State {
    session: NoteSession,
    /// Shell-installed hook, fired after every content change (used to
    /// schedule autosave). Never called while `state` is borrowed.
    on_change: Option<Box<dyn Fn()>>,
    /// Shell hook: zoom changed (drives the toolbar's zoom indicator).
    on_zoom: Option<Box<dyn Fn(f64)>>,
    /// Shell hook: stylus barrel button clicked — toolbar toggles the eraser.
    on_eraser_toggle: Option<Box<dyn Fn()>>,
    /// World coordinate at the widget's top-left corner.
    offset: kurbo::Vec2,
    zoom: f64,
    live: Option<LiveStroke>,
    /// Committed-stroke render nodes, rebuilt lazily per stroke.
    node_cache: HashMap<StrokeId, gsk::RenderNode>,
    active: ActiveTool,
    cur_color: SemanticColor,
    cur_width: f64,
    eraser_kind: EraserKind,
    eraser_radius: f64,
    /// Strokes the current eraser drag affects (stroke eraser: hidden live;
    /// area eraser: previewed as fragments). Committed on release.
    erasing: bool,
    live_erased: HashSet<StrokeId>,
    /// World-space eraser path for the area eraser (thinned).
    erase_path: Vec<kurbo::Point>,
    /// Current selection + in-progress selection gestures.
    selection: HashSet<StrokeId>,
    sel_images: HashSet<ImageId>,
    sel_texts: HashSet<TextId>,
    /// Pango layouts per text box, valid for the current revision.
    text_layouts: HashMap<TextId, TextLayout>,
    clipboard_texts: Vec<TextBox>,
    /// The text box being edited in the overlay editor (hidden on canvas).
    editing: Option<EditSession>,
    /// Font size for new text boxes (follows the last size used).
    text_font_size: f64,
    /// Style + color for new text (set from the text flyout with nothing
    /// selected or edited).
    text_defaults: textmod::StyleState,
    /// List type new text boxes start with (picked with nothing to apply to).
    text_list_default: Option<textmod::ListKind>,
    /// Shell hook: text editing state for the toolbar format group
    /// (None = not editing).
    on_text_state: Option<Box<dyn Fn(Option<textmod::StyleState>)>>,
    sel_resize: Option<ResizeDrag>,
    /// Select tool rubber-band rectangle: (start, current) in world units.
    marquee: Option<(kurbo::Point, kurbo::Point)>,
    /// Image assets: directory beside the open note + decoded texture cache.
    asset_dir: Option<PathBuf>,
    textures: HashMap<String, Option<gdk::Texture>>,
    /// App-installed: persist image bytes (with file extension) beside the
    /// note, return the asset name.
    asset_writer: Option<Box<dyn Fn(&[u8], &str) -> Option<String>>>,
    /// App-installed: switch the toolbar to Select (after pasting an image).
    on_request_select: Option<Box<dyn Fn()>>,
    /// Images copied with a selection, and the asset dir they came from (so
    /// pasting into another note can bring the image file along).
    clipboard_images: Vec<ImageItem>,
    clipboard_asset_dir: Option<PathBuf>,
    lassoing: bool,
    lasso_path: Vec<kurbo::Point>,
    /// While dragging a selection: last world point + accumulated offset.
    sel_drag: Option<kurbo::Point>,
    sel_offset: kurbo::Vec2,
    /// Shape tool state.
    shape_kind: omascratch_ink::ShapeKind,
    shape_drag: Option<(kurbo::Point, kurbo::Point)>,
    /// Shift held: constrain shapes (square/circle, 45-degree lines).
    shift_down: bool,
    /// Hand-tool drag: (start widget x, start widget y, offset at start).
    panning: Option<(f64, f64, kurbo::Vec2)>,
    /// App-internal stroke clipboard (copy/cut/paste of selections).
    clipboard: Vec<Stroke>,
    /// Perf mode: snapshot() durations (ms).
    perf_frames: Vec<f64>,
    /// Per-stroke bounds aligned with `session.content.strokes`, valid while
    /// `bounds_rev == session.revision()`. Used for culling and as a cheap
    /// prefilter before exact hit tests (eraser, select).
    bounds: Vec<Option<kurbo::Rect>>,
    bounds_rev: u64,
    /// Light-canvas inversion (page + semantic ink flip; fixed colors stay).
    inverted: bool,
    background: PageBackground,
    palette: CanvasPalette,
    pan_anchor: Option<kurbo::Vec2>,
    pointer: (f64, f64),
    debug: DebugStats,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: NoteSession::default(),
            on_change: None,
            on_zoom: None,
            on_eraser_toggle: None,
            offset: kurbo::Vec2::ZERO,
            zoom: 1.0,
            live: None,
            node_cache: HashMap::new(),
            active: ActiveTool::Pen,
            cur_color: SemanticColor::Foreground,
            cur_width: 3.5,
            eraser_kind: EraserKind::Stroke,
            eraser_radius: 12.0,
            erasing: false,
            live_erased: HashSet::new(),
            erase_path: Vec::new(),
            selection: HashSet::new(),
            sel_images: HashSet::new(),
            sel_texts: HashSet::new(),
            text_layouts: HashMap::new(),
            clipboard_texts: Vec::new(),
            editing: None,
            text_font_size: textmod::DEFAULT_FONT_SIZE,
            text_defaults: textmod::StyleState::default(),
            text_list_default: None,
            on_text_state: None,
            sel_resize: None,
            marquee: None,
            asset_dir: None,
            textures: HashMap::new(),
            asset_writer: None,
            on_request_select: None,
            clipboard_images: Vec::new(),
            clipboard_asset_dir: None,
            lassoing: false,
            lasso_path: Vec::new(),
            sel_drag: None,
            sel_offset: kurbo::Vec2::ZERO,
            shape_kind: omascratch_ink::ShapeKind::Line,
            shape_drag: None,
            shift_down: false,
            panning: None,
            clipboard: Vec::new(),
            perf_frames: Vec::new(),
            bounds: Vec::new(),
            bounds_rev: u64::MAX,
            inverted: false,
            background: PageBackground::default(),
            palette: CanvasPalette::default(),
            pan_anchor: None,
            pointer: (0.0, 0.0),
            debug: DebugStats::default(),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CanvasView {
        pub state: RefCell<State>,
        pub editor: RefCell<Option<std::rc::Rc<TextEditor>>>,
        pub checkpoint: RefCell<Option<glib::SourceId>>,
        pub editor_tick: std::cell::Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CanvasView {
        const NAME: &'static str = "OmaScratchCanvasView";
        type Type = super::CanvasView;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CanvasView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.setup_input();
        }
    }

    impl WidgetImpl for CanvasView {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if crate::perf::enabled() {
                let t = Instant::now();
                self.obj().draw(snapshot);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                self.state.borrow_mut().perf_frames.push(ms);
            } else {
                self.obj().draw(snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct CanvasView(ObjectSubclass<imp::CanvasView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for CanvasView {
    fn default() -> Self {
        glib::Object::new()
    }
}

fn axis_index(axis: gdk::AxisUse) -> usize {
    use glib::translate::IntoGlib;
    axis.into_glib() as usize
}

fn bezpath_to_gsk(path: &kurbo::BezPath) -> gsk::Path {
    let b = gsk::PathBuilder::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => b.move_to(p.x as f32, p.y as f32),
            PathEl::LineTo(p) => b.line_to(p.x as f32, p.y as f32),
            PathEl::QuadTo(c, p) => b.quad_to(c.x as f32, c.y as f32, p.x as f32, p.y as f32),
            PathEl::CurveTo(c1, c2, p) => {
                b.cubic_to(c1.x as f32, c1.y as f32, c2.x as f32, c2.y as f32, p.x as f32, p.y as f32)
            }
            PathEl::ClosePath => b.close(),
        }
    }
    b.to_path()
}

/// Shift constraint: squares/circles for rect/ellipse, 45-degree snapping
/// for lines and arrows.
fn constrain_shape(kind: omascratch_ink::ShapeKind, start: kurbo::Point, end: kurbo::Point) -> kurbo::Point {
    use omascratch_ink::ShapeKind;
    let d = end - start;
    match kind {
        ShapeKind::Rect | ShapeKind::Ellipse => {
            let m = d.x.abs().max(d.y.abs());
            kurbo::Point::new(start.x + m * d.x.signum(), start.y + m * d.y.signum())
        }
        ShapeKind::Line | ShapeKind::Arrow => {
            let len = d.hypot();
            if len < 1e-6 {
                return end;
            }
            let step = std::f64::consts::FRAC_PI_4;
            let angle = (d.y.atan2(d.x) / step).round() * step;
            kurbo::Point::new(start.x + len * angle.cos(), start.y + len * angle.sin())
        }
    }
}

fn polyline_to_gsk(points: &[InkPoint]) -> gsk::Path {
    let b = gsk::PathBuilder::new();
    if let Some(first) = points.first() {
        b.move_to(first.x as f32, first.y as f32);
        for p in &points[1..] {
            b.line_to(p.x as f32, p.y as f32);
        }
    }
    b.to_path()
}

fn shape_stroke(width: f64) -> gsk::Stroke {
    let s = gsk::Stroke::new(width as f32);
    s.set_line_cap(gsk::LineCap::Round);
    s.set_line_join(gsk::LineJoin::Round);
    s
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve a semantic color to an on-screen RGBA. Highlighter strokes render
/// translucent. (M5's theme adapter will replace the fixed Foreground/Accent.)
fn resolve_color_inv(c: SemanticColor, tool: Tool, inverted: bool, pal: &CanvasPalette) -> gdk::RGBA {
    let page = if inverted { pal.bg_inv } else { pal.bg };
    let page_dark = luminance(page.red(), page.green(), page.blue()) < 0.5;
    let base = match c {
        SemanticColor::Foreground => {
            if inverted {
                pal.ink_inv
            } else {
                pal.ink
            }
        }
        SemanticColor::Accent => pal.accent,
        SemanticColor::Fixed(Rgba { r, g, b, a }) => {
            // Neutral inks (black/white/greys) track the page polarity like
            // OneNote's automatic ink color: black on light paper renders
            // white on a dark page and vice versa. Colored inks stay put.
            let chroma = r.max(g).max(b) - r.min(g).min(b);
            if chroma < 0.12 {
                let lum = luminance(r, g, b);
                let clash = (page_dark && lum < 0.4) || (!page_dark && lum > 0.6);
                if clash {
                    return apply_highlight_alpha(gdk::RGBA::new(1.0 - r, 1.0 - g, 1.0 - b, a), tool);
                }
            }
            gdk::RGBA::new(r, g, b, a)
        }
    };
    apply_highlight_alpha(base, tool)
}

fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.299 * r + 0.587 * g + 0.114 * b
}

fn apply_highlight_alpha(base: gdk::RGBA, tool: Tool) -> gdk::RGBA {
    if matches!(tool, Tool::Highlighter) {
        gdk::RGBA::new(base.red(), base.green(), base.blue(), base.alpha() * 0.4)
    } else {
        base
    }
}

impl CanvasView {
    // ---- public API used by the shell ----

    pub fn undo(&self) {
        self.commit_text_edit();
        let mut st = self.imp().state.borrow_mut();
        let changed = st.session.undo();
        if changed {
            // A translate keeps ids but moves points; drop all cached nodes.
            st.node_cache.clear();
            drop(st);
            self.queue_draw();
            self.notify_changed();
        }
    }

    pub fn redo(&self) {
        self.commit_text_edit();
        let mut st = self.imp().state.borrow_mut();
        let changed = st.session.redo();
        if changed {
            st.node_cache.clear();
            drop(st);
            self.queue_draw();
            self.notify_changed();
        }
    }

    /// Replace the canvas content (opening a note). Resets undo history and
    /// the render cache; keeps the viewport.
    pub fn set_content(&self, content: omascratch_core::NoteContent) {
        let mut st = self.imp().state.borrow_mut();
        st.session = NoteSession::new(content);
        st.node_cache.clear();
        st.bounds_rev = u64::MAX;
        st.live = None;
        st.erasing = false;
        st.live_erased.clear();
        st.selection.clear();
        st.sel_images.clear();
        st.sel_texts.clear();
        st.text_layouts.clear();
        st.editing = None;
        st.sel_resize = None;
        st.marquee = None;
        st.lassoing = false;
        st.lasso_path.clear();
        st.sel_drag = None;
        st.sel_offset = kurbo::Vec2::ZERO;
        st.shape_drag = None;
        st.panning = None;
        // A fresh note opens at its top-left home at 100%.
        st.offset = kurbo::Vec2::ZERO;
        st.zoom = 1.0;
        drop(st);
        self.queue_draw();
        self.notify_zoom();
    }

    /// Current revision + a clone of the content, for a generation-tagged save.
    pub fn content_snapshot(&self) -> (u64, omascratch_core::NoteContent) {
        let st = self.imp().state.borrow();
        (st.session.revision(), st.session.content.clone())
    }

    pub fn revision(&self) -> u64 {
        self.imp().state.borrow().session.revision()
    }

    pub fn set_on_change(&self, f: impl Fn() + 'static) {
        self.imp().state.borrow_mut().on_change = Some(Box::new(f));
    }

    pub fn set_on_zoom(&self, f: impl Fn(f64) + 'static) {
        self.imp().state.borrow_mut().on_zoom = Some(Box::new(f));
    }

    pub fn set_on_eraser_toggle(&self, f: impl Fn() + 'static) {
        self.imp().state.borrow_mut().on_eraser_toggle = Some(Box::new(f));
    }

    fn fire_eraser_toggle(&self) {
        let hook = self.imp().state.borrow_mut().on_eraser_toggle.take();
        if let Some(hook) = hook {
            hook();
            let mut st = self.imp().state.borrow_mut();
            if st.on_eraser_toggle.is_none() {
                st.on_eraser_toggle = Some(hook);
            }
        }
    }

    fn notify_zoom(&self) {
        let zoom = self.imp().state.borrow().zoom;
        let hook = self.imp().state.borrow_mut().on_zoom.take();
        if let Some(hook) = hook {
            hook(zoom);
            let mut st = self.imp().state.borrow_mut();
            if st.on_zoom.is_none() {
                st.on_zoom = Some(hook);
            }
        }
    }

    /// Perf mode: scripted benchmark — first frames, a 120-frame pan, an
    /// eraser sweep and a full-page lasso — then quit.
    pub fn run_perf_bench(&self) {
        let view = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            let n = view.imp().state.borrow().session.content.strokes.len();
            eprintln!("[perf] note strokes: {n}");
            let mut first = std::mem::take(&mut view.imp().state.borrow_mut().perf_frames);
            crate::perf::summarize("frames during open (incl. cache build)", &mut first);

            let ticks = std::rc::Rc::new(std::cell::Cell::new(0u32));
            let v = view.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                let t = ticks.get();
                if t >= 120 {
                    let mut f = std::mem::take(&mut v.imp().state.borrow_mut().perf_frames);
                    crate::perf::summarize("pan frames (120 x 35 units)", &mut f);
                    v.bench_hit_tests();
                    if let Some(win) = v.root().and_downcast::<gtk::Window>() {
                        win.close();
                    }
                    return glib::ControlFlow::Break;
                }
                ticks.set(t + 1);
                {
                    let mut st = v.imp().state.borrow_mut();
                    st.offset.y += 35.0;
                }
                v.queue_draw();
                glib::ControlFlow::Continue
            });
        });
    }

    fn bench_hit_tests(&self) {
        let mut st = self.imp().state.borrow_mut();
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        // Eraser sweep: 300 samples across the middle of the view.
        let cy = st.offset.y + vh / 2.0 / st.zoom;
        let x0 = st.offset.x;
        let t = Instant::now();
        let kind = st.eraser_kind;
        st.eraser_kind = EraserKind::Stroke;
        for i in 0..300 {
            let wx = x0 + (i as f64) * (vw / st.zoom) / 300.0;
            Self::erase_at(&mut st, wx, cy);
        }
        let hits = st.live_erased.len();
        st.live_erased.clear();
        st.eraser_kind = kind;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("[perf] eraser sweep: 300 samples in {ms:.2} ms ({:.3} ms/sample, {hits} hits)", ms / 300.0);

        // Lasso around the visible area.
        let poly = vec![
            kurbo::Point::new(st.offset.x, st.offset.y),
            kurbo::Point::new(st.offset.x + vw / st.zoom, st.offset.y),
            kurbo::Point::new(st.offset.x + vw / st.zoom, st.offset.y + vh / st.zoom),
            kurbo::Point::new(st.offset.x, st.offset.y + vh / st.zoom),
        ];
        let t = Instant::now();
        let sel = st
            .session
            .content
            .strokes
            .iter()
            .filter(|s| omascratch_ink::stroke_inside_polygon(&s.points, &poly))
            .count();
        eprintln!("[perf] lasso visible area: {sel} selected in {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);

        // Select-tool click hit test at the view center.
        let wp = kurbo::Point::new(st.offset.x + vw / 2.0 / st.zoom, cy);
        let t = Instant::now();
        let radius = 6.0 / st.zoom;
        let _ = st
            .session
            .content
            .strokes
            .iter()
            .rev()
            .find(|s| omascratch_ink::stroke_hit(&s.points, s.width, wp, radius));
        eprintln!("[perf] select click hit-test: {:.3} ms", t.elapsed().as_secs_f64() * 1000.0);
    }

    // ---- text boxes ----

    /// Host the rich-text editor in the overlay above the canvas.
    pub fn attach_editor_host(&self, overlay: &gtk::Overlay) {
        let ed = TextEditor::new();
        overlay.add_overlay(&ed.view);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(v) = weak.upgrade() {
                    v.commit_text_edit();
                    v.grab_focus();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        ed.view.add_controller(keys);

        let weak = self.downgrade();
        ed.buffer.connect_changed(move |_| {
            if let Some(v) = weak.upgrade() {
                v.schedule_checkpoint();
                // List prefixes are drawn by the canvas: redraw now and once
                // the TextView has re-laid out its lines.
                v.queue_draw();
                let w = v.downgrade();
                glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
                    if let Some(v) = w.upgrade() {
                        v.queue_draw();
                    }
                });
            }
        });
        *self.imp().editor.borrow_mut() = Some(ed);
    }

    pub fn set_on_text_state(&self, f: impl Fn(Option<textmod::StyleState>) + 'static) {
        self.imp().state.borrow_mut().on_text_state = Some(Box::new(f));
    }

    fn emit_text_state(&self, s: Option<textmod::StyleState>) {
        let hook = self.imp().state.borrow_mut().on_text_state.take();
        if let Some(hook) = hook {
            hook(s);
            let mut st = self.imp().state.borrow_mut();
            if st.on_text_state.is_none() {
                st.on_text_state = Some(hook);
            }
        }
    }

    pub fn is_editing_text(&self) -> bool {
        self.imp().state.borrow().editing.is_some()
    }

    fn editor(&self) -> Option<std::rc::Rc<TextEditor>> {
        self.imp().editor.borrow().clone()
    }

    /// Open the editor on `existing` (or a new box at `at`), optionally
    /// placing the cursor at widget point `cursor_at`.
    fn start_edit(&self, existing: Option<TextBox>, at: kurbo::Point, cursor_at: Option<(f64, f64)>) {
        self.commit_text_edit();
        let Some(ed) = self.editor() else { return };
        let working = {
            let mut st = self.imp().state.borrow_mut();
            let fs = st.text_font_size;
            // New boxes snap to the rule row under the click.
            let row = st.background.spacing.max(8.0);
            let working = existing.clone().unwrap_or_else(|| TextBox {
                id: TextId::new(),
                x: at.x,
                y: (at.y / row).floor() * row,
                w: textmod::DEFAULT_WIDTH,
                h: fs * 1.2,
                font_size: fs,
                color: SemanticColor::Foreground,
                paras: vec![omascratch_core::Paragraph { kind: ParaKind::Body, spans: vec![] }],
            });
            st.selection.clear();
            st.sel_images.clear();
            st.sel_texts.clear();
            st.editing = Some(EditSession {
                original: existing,
                working: working.clone(),
                last_view: (st.offset, st.zoom),
            });
            working
        };
        let is_new = self.imp().state.borrow().editing.as_ref().is_some_and(|e| e.original.is_none());
        ed.load(&working);
        if is_new {
            let (defaults, list) = {
                let st = self.imp().state.borrow();
                (st.text_defaults, st.text_list_default)
            };
            if let Some(k) = list {
                ed.toggle_list(k);
            }
            ed.set_pending(defaults);
        }
        self.sync_editor_geometry(true);
        ed.view.set_visible(true);
        ed.view.grab_focus();
        if let Some((x, y)) = cursor_at {
            let ed2 = ed.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
                let bx = (x - ed2.view.margin_start() as f64) as i32;
                let by = (y - ed2.view.margin_top() as f64) as i32;
                let (bx, by) = ed2.view.window_to_buffer_coords(gtk::TextWindowType::Widget, bx, by);
                if let Some(it) = ed2.view.iter_at_location(bx, by) {
                    ed2.buffer.place_cursor(&it);
                }
            });
        }
        // Follow pan/zoom while editing.
        if !self.imp().editor_tick.replace(true) {
            self.add_tick_callback(|w, _| {
                if !w.is_editing_text() {
                    w.imp().editor_tick.set(false);
                    return glib::ControlFlow::Break;
                }
                w.sync_editor_geometry(false);
                glib::ControlFlow::Continue
            });
        }
        self.queue_draw();
        self.emit_text_state(Some(ed.current_style()));
    }

    /// Position, size and style the overlay editor over its box.
    fn sync_editor_geometry(&self, force: bool) {
        let Some(ed) = self.editor() else { return };
        let (x, y, w, fs, offset, zoom, color, accent) = {
            let mut st = self.imp().state.borrow_mut();
            let (offset, zoom, inverted, palette) = (st.offset, st.zoom, st.inverted, st.palette);
            let Some(sess) = st.editing.as_mut() else { return };
            if !force && sess.last_view == (offset, zoom) {
                return;
            }
            sess.last_view = (offset, zoom);
            let t = &sess.working;
            let color = resolve_color_inv(t.color, Tool::Pen, inverted, &palette);
            (t.x, t.y, t.w, t.font_size, offset, zoom, color, palette.accent)
        };
        let (cell, resolver) = {
            let st = self.imp().state.borrow();
            (Self::text_cell(&st, fs), Self::text_resolver(&st))
        };
        let ctx = self.pango_context();
        let (asc, desc) = textmod::font_extents(&ctx, fs * zoom);
        ed.set_resolver(resolver);
        ed.view.set_margin_start(((x - offset.x) * zoom).max(0.0) as i32);
        ed.view.set_margin_top(((y - offset.y) * zoom).max(0.0) as i32);
        ed.view.set_size_request((w * zoom).max(40.0) as i32, -1);
        ed.restyle(fs * zoom, cell * zoom, asc + desc, &color, &accent);
        if std::env::var_os("OMASCRATCH_DEBUG_TEXT").is_some() {
            eprintln!("[text] editor geometry: zoom={zoom:.3} font_px={:.1} cell_px={:.1} at=({:.0},{:.0}) w={:.0}", fs * zoom, cell * zoom, (x - offset.x) * zoom, (y - offset.y) * zoom, w * zoom);
        }
    }

    /// Finish the current edit: one exact ReplaceTexts (add / change /
    /// delete-when-blank). Safe to call when nothing is being edited.
    pub fn commit_text_edit(&self) {
        let session = self.imp().state.borrow_mut().editing.take();
        let Some(session) = session else { return };
        if let Some(id) = self.imp().checkpoint.borrow_mut().take() {
            id.remove();
        }
        let Some(ed) = self.editor() else { return };
        let mut working = session.working;
        working.paras = ed.to_paras();
        ed.view.set_visible(false);
        let ctx = self.pango_context();
        working.h = {
            let st = self.imp().state.borrow();
            textmod::layout_text(&ctx, &working, Self::text_cell(&st, working.font_size), &Self::text_resolver(&st)).height
        };
        let changed = {
            let mut st = self.imp().state.borrow_mut();
            st.text_font_size = working.font_size;
            match (session.original, working.is_blank()) {
                (None, true) => false,
                (Some(orig), true) => {
                    st.session.dispatch(Command::ReplaceTexts { before: vec![orig], after: vec![] });
                    true
                }
                (Some(orig), false) if orig == working => false,
                (orig, false) => {
                    st.session.dispatch(Command::ReplaceTexts {
                        before: orig.into_iter().collect(),
                        after: vec![working],
                    });
                    true
                }
            }
        };
        self.emit_text_state(None);
        self.queue_draw();
        if changed {
            self.notify_changed();
        }
    }

    fn schedule_checkpoint(&self) {
        if !self.is_editing_text() {
            return;
        }
        if let Some(id) = self.imp().checkpoint.borrow_mut().take() {
            id.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(2500), move || {
            if let Some(v) = weak.upgrade() {
                *v.imp().checkpoint.borrow_mut() = None;
                v.checkpoint_text();
            }
        });
        *self.imp().checkpoint.borrow_mut() = Some(id);
    }

    /// Crash protection while typing: after a pause, commit the current text
    /// into the note (the edit stays open), so autosave picks it up.
    fn checkpoint_text(&self) {
        let Some(ed) = self.editor() else { return };
        let ctx = self.pango_context();
        let mut st = self.imp().state.borrow_mut();
        let Some(font) = st.editing.as_ref().map(|e| e.working.font_size) else { return };
        let cell = Self::text_cell(&st, font);
        let resolver = Self::text_resolver(&st);
        let Some(sess) = st.editing.as_mut() else { return };
        let mut w = sess.working.clone();
        w.paras = ed.to_paras();
        if w.is_blank() || sess.original.as_ref() == Some(&w) {
            return;
        }
        w.h = textmod::layout_text(&ctx, &w, cell, &resolver).height;
        let before: Vec<TextBox> = sess.original.clone().into_iter().collect();
        sess.original = Some(w.clone());
        sess.working = w.clone();
        st.session.dispatch(Command::ReplaceTexts { before, after: vec![w] });
        drop(st);
        self.notify_changed();
    }

    /// Text-aware press handling, run before any gesture starts. Returns true
    /// when the press was consumed (editing, checkbox toggle, new box).
    fn text_press(&self, x: f64, y: f64) -> bool {
        let (wp, active, editing_rect, zoom) = {
            let st = self.imp().state.borrow();
            let (wx, wy) = Self::widget_to_world(&st, x, y);
            let rect = st.editing.as_ref().map(|e| {
                let h = self
                    .editor()
                    .map(|ed| ed.view.height() as f64 / st.zoom)
                    .unwrap_or(e.working.h)
                    .max(e.working.h);
                kurbo::Rect::new(e.working.x, e.working.y, e.working.x + e.working.w, e.working.y + h)
            });
            (kurbo::Point::new(wx, wy), st.active, rect, st.zoom)
        };
        if let Some(r) = editing_rect {
            if r.inflate(6.0 / zoom, 6.0 / zoom).contains(wp) {
                return true;
            }
            self.commit_text_edit();
        }
        if !matches!(active, ActiveTool::Select | ActiveTool::Text) {
            return false;
        }
        if self.toggle_checkbox_at(wp) {
            return true;
        }
        if active == ActiveTool::Text {
            let hit = {
                let st = self.imp().state.borrow();
                Self::text_at(&st, wp)
                    .and_then(|id| st.session.content.text_index(id))
                    .map(|i| st.session.content.texts[i].clone())
            };
            match hit {
                Some(tb) => self.start_edit(Some(tb), wp, Some((x, y))),
                None => self.start_edit(None, wp, None),
            }
            return true;
        }
        false
    }

    /// Click on a rendered checkbox toggles it (one undo step).
    fn toggle_checkbox_at(&self, wp: kurbo::Point) -> bool {
        let ctx = self.pango_context();
        let mut st = self.imp().state.borrow_mut();
        Self::ensure_bounds(&mut st);
        let candidates: Vec<TextBox> = st
            .session
            .content
            .texts
            .iter()
            .rev()
            .filter(|t| t.rect().contains(wp))
            .cloned()
            .collect();
        for tb in candidates {
            if !st.text_layouts.contains_key(&tb.id) {
                let tl = textmod::layout_text(&ctx, &tb, Self::text_cell(&st, tb.font_size), &Self::text_resolver(&st));
                st.text_layouts.insert(tb.id, tl);
            }
            let hit = st.text_layouts[&tb.id]
                .check_rects(tb.font_size)
                .into_iter()
                .find(|(_, r)| r.inflate(2.0, 2.0).contains(kurbo::Point::new(wp.x - tb.x, wp.y - tb.y)))
                .map(|(i, _)| i);
            if let Some(pi) = hit {
                let mut after = tb.clone();
                if let ParaKind::Check { checked } = after.paras[pi].kind {
                    after.paras[pi].kind = ParaKind::Check { checked: !checked };
                }
                st.session.dispatch(Command::ReplaceTexts { before: vec![tb], after: vec![after] });
                drop(st);
                self.queue_draw();
                self.notify_changed();
                return true;
            }
        }
        false
    }

    // -- text settings API (toolbar flyout) --
    //
    // Three targets, in priority order: the open editor (selection / next
    // typed text), selected text boxes (whole boxes, one undo step), or —
    // with neither — the defaults for new text.

    /// Give keyboard focus back to the open text editor (after the text
    /// flyout closes; grabbing it while the flyout is open dismisses it).
    pub fn focus_text_editor(&self) {
        if let (true, Some(ed)) = (self.is_editing_text(), self.editor()) {
            ed.view.grab_focus();
        }
    }

    fn selected_texts(&self) -> Vec<TextBox> {
        let st = self.imp().state.borrow();
        st.session.content.texts.iter().filter(|t| st.sel_texts.contains(&t.id)).cloned().collect()
    }

    /// True when list/size changes have something to act on.
    pub fn text_has_target(&self) -> bool {
        self.is_editing_text() || !self.imp().state.borrow().sel_texts.is_empty()
    }

    /// Transform every selected box (one exact undo step).
    fn update_selected_texts(&self, f: impl Fn(&mut TextBox)) -> bool {
        let before = self.selected_texts();
        if before.is_empty() {
            return false;
        }
        let ctx = self.pango_context();
        let after: Vec<TextBox> = {
            let st = self.imp().state.borrow();
            let resolver = Self::text_resolver(&st);
            before
                .iter()
                .map(|t| {
                    let mut a = t.clone();
                    f(&mut a);
                    a.h = textmod::layout_text(&ctx, &a, Self::text_cell(&st, a.font_size), &resolver).height;
                    a
                })
                .collect()
        };
        if after == before {
            return true;
        }
        self.imp().state.borrow_mut().session.dispatch(Command::ReplaceTexts { before, after });
        self.queue_draw();
        self.notify_changed();
        true
    }

    /// Aggregate style of whole boxes: a flag is on when every character has it.
    fn boxes_style(boxes: &[TextBox]) -> textmod::StyleState {
        let spans: Vec<&omascratch_core::Span> = boxes
            .iter()
            .flat_map(|t| t.paras.iter().flat_map(|p| p.spans.iter()))
            .filter(|s| !s.text.is_empty())
            .collect();
        let Some(first) = spans.first() else { return textmod::StyleState::default() };
        textmod::StyleState {
            bold: spans.iter().all(|s| s.bold),
            italic: spans.iter().all(|s| s.italic),
            underline: spans.iter().all(|s| s.underline),
            highlight: spans.iter().all(|s| s.highlight),
            color: if spans.iter().all(|s| s.color == first.color) { first.color } else { None },
        }
    }

    /// Current style for the flyout's indicators.
    pub fn text_style_state(&self) -> textmod::StyleState {
        if self.is_editing_text() {
            if let Some(ed) = self.editor() {
                return ed.current_style();
            }
        }
        let sel = self.selected_texts();
        if !sel.is_empty() {
            return Self::boxes_style(&sel);
        }
        self.imp().state.borrow().text_defaults
    }

    pub fn text_format(&self, f: textmod::Fmt) {
        use textmod::Fmt;
        if let (true, Some(ed)) = (self.is_editing_text(), self.editor()) {
            ed.toggle(f);
            self.schedule_checkpoint();
            return;
        }
        let sel = self.selected_texts();
        if !sel.is_empty() {
            let cur = Self::boxes_style(&sel);
            let on = !match f {
                Fmt::Bold => cur.bold,
                Fmt::Italic => cur.italic,
                Fmt::Underline => cur.underline,
                Fmt::Highlight => cur.highlight,
            };
            self.update_selected_texts(|t| {
                for sp in t.paras.iter_mut().flat_map(|p| p.spans.iter_mut()) {
                    match f {
                        Fmt::Bold => sp.bold = on,
                        Fmt::Italic => sp.italic = on,
                        Fmt::Underline => sp.underline = on,
                        Fmt::Highlight => sp.highlight = on,
                    }
                }
            });
            return;
        }
        let mut st = self.imp().state.borrow_mut();
        let d = &mut st.text_defaults;
        match f {
            Fmt::Bold => d.bold = !d.bold,
            Fmt::Italic => d.italic = !d.italic,
            Fmt::Underline => d.underline = !d.underline,
            Fmt::Highlight => d.highlight = !d.highlight,
        }
    }

    /// Color the editor selection / next text, or whole selected boxes; it
    /// also becomes the color for new text.
    pub fn text_set_color(&self, color: Option<SemanticColor>) {
        self.imp().state.borrow_mut().text_defaults.color = color;
        if let (true, Some(ed)) = (self.is_editing_text(), self.editor()) {
            ed.set_color(color);
            self.schedule_checkpoint();
            return;
        }
        self.update_selected_texts(|t| {
            for sp in t.paras.iter_mut().flat_map(|p| p.spans.iter_mut()) {
                sp.color = color;
            }
        });
    }

    fn para_list_kind(k: ParaKind) -> Option<textmod::ListKind> {
        match k {
            ParaKind::Bullet => Some(textmod::ListKind::Bullet),
            ParaKind::Number => Some(textmod::ListKind::Number),
            ParaKind::Check { .. } => Some(textmod::ListKind::Check),
            ParaKind::Body => None,
        }
    }

    /// List kind at the cursor, or shared by every paragraph of the selected boxes.
    pub fn text_list_kind(&self) -> Option<textmod::ListKind> {
        if self.is_editing_text() {
            return self.editor()?.current_list_kind();
        }
        let sel = self.selected_texts();
        if sel.is_empty() {
            return self.imp().state.borrow().text_list_default;
        }
        // Blank lines don't count (a list usually ends with one).
        let mut kinds = sel.iter().flat_map(|t| {
            t.paras.iter().filter(|p| !p.plain_text().trim().is_empty()).map(|p| Self::para_list_kind(p.kind))
        });
        let first = kinds.next()??;
        kinds.all(|k| k == Some(first)).then_some(first)
    }

    pub fn text_list(&self, k: textmod::ListKind) {
        if let (true, Some(ed)) = (self.is_editing_text(), self.editor()) {
            ed.toggle_list(k);
            self.schedule_checkpoint();
            return;
        }
        let remove = self.text_list_kind() == Some(k);
        if self.selected_texts().is_empty() {
            self.imp().state.borrow_mut().text_list_default = if remove { None } else { Some(k) };
            return;
        }
        self.update_selected_texts(|t| {
            for p in t.paras.iter_mut().filter(|p| !p.plain_text().trim().is_empty()) {
                p.kind = if remove {
                    ParaKind::Body
                } else {
                    match k {
                        textmod::ListKind::Bullet => ParaKind::Bullet,
                        textmod::ListKind::Number => ParaKind::Number,
                        textmod::ListKind::Check => match p.kind {
                            ParaKind::Check { checked } => ParaKind::Check { checked },
                            _ => ParaKind::Check { checked: false },
                        },
                    }
                };
            }
        });
    }

    fn step_size(cur: f64, up: bool) -> f64 {
        const SIZES: [f64; 11] = [10.0, 12.0, 14.0, 16.0, 18.0, 22.0, 26.0, 32.0, 40.0, 48.0, 64.0];
        if up {
            SIZES.iter().copied().find(|s| *s > cur + 0.01).unwrap_or(cur)
        } else {
            SIZES.iter().rev().copied().find(|s| *s < cur - 0.01).unwrap_or(cur)
        }
    }

    /// Step the font size of the edited box, the selected boxes, or the
    /// new-text default. Returns the resulting size.
    pub fn text_font_step(&self, up: bool) -> Option<f64> {
        if self.is_editing_text() {
            let size = {
                let mut st = self.imp().state.borrow_mut();
                let sess = st.editing.as_mut()?;
                sess.working.font_size = Self::step_size(sess.working.font_size, up);
                sess.working.font_size
            };
            self.imp().state.borrow_mut().text_font_size = size;
            self.sync_editor_geometry(true);
            self.schedule_checkpoint();
            return Some(size);
        }
        if self.update_selected_texts(|t| t.font_size = Self::step_size(t.font_size, up)) {
            return Some(self.text_font_size());
        }
        let mut st = self.imp().state.borrow_mut();
        st.text_font_size = Self::step_size(st.text_font_size, up);
        Some(st.text_font_size)
    }

    pub fn text_font_size(&self) -> f64 {
        if let Some(t) = self.selected_texts().first().filter(|_| !self.is_editing_text()) {
            return t.font_size;
        }
        let st = self.imp().state.borrow();
        st.editing.as_ref().map(|e| e.working.font_size).unwrap_or(st.text_font_size)
    }

    /// Keyboard zoom (Ctrl+= / Ctrl+-): one 1.25× step, anchored at the
    /// viewport center.
    pub fn zoom_step(&self, zoom_in: bool) {
        let mut st = self.imp().state.borrow_mut();
        let old = st.zoom;
        let factor = if zoom_in { 1.25 } else { 1.0 / 1.25 };
        let new = (old * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if (new - old).abs() < f64::EPSILON {
            return;
        }
        let (w, h) = (self.width() as f64 / 2.0, self.height() as f64 / 2.0);
        st.offset += kurbo::Vec2::new(w / old, h / old) - kurbo::Vec2::new(w / new, h / new);
        st.zoom = new;
        Self::clamp_offset(&mut st);
        drop(st);
        self.queue_draw();
        self.notify_zoom();
    }

    /// Return to 100% zoom, anchored at the viewport center.
    pub fn zoom_to_100(&self) {
        let mut st = self.imp().state.borrow_mut();
        let old = st.zoom;
        if (old - 1.0).abs() < f64::EPSILON {
            return;
        }
        let (w, h) = (self.width() as f64 / 2.0, self.height() as f64 / 2.0);
        let before = kurbo::Vec2::new(w / old, h / old);
        let after = kurbo::Vec2::new(w, h);
        st.offset += before - after;
        st.zoom = 1.0;
        Self::clamp_offset(&mut st);
        drop(st);
        self.queue_draw();
        self.notify_zoom();
    }

    // ---- tool selection (driven by the draw toolbar) ----

    pub fn set_active_tool(&self, tool: ActiveTool) {
        if tool != ActiveTool::Text {
            self.commit_text_edit();
        }
        self.imp().state.borrow_mut().active = tool;
    }

    pub fn active_tool(&self) -> ActiveTool {
        self.imp().state.borrow().active
    }

    /// Select a pen preset: sets the active drawing tool, color and width.
    pub fn set_pen(&self, tool: Tool, color: SemanticColor, width: f64) {
        self.commit_text_edit();
        let mut st = self.imp().state.borrow_mut();
        st.active = tool_to_active(tool);
        st.cur_color = color;
        st.cur_width = width;
    }

    /// Activate the shape tool with the given shape kind.
    pub fn set_shape_tool(&self, kind: omascratch_ink::ShapeKind) {
        self.commit_text_edit();
        let mut st = self.imp().state.borrow_mut();
        st.active = ActiveTool::Shape;
        st.shape_kind = kind;
    }

    /// Install a new canvas palette (theme switch). Clears the render cache.
    pub fn set_palette(&self, palette: CanvasPalette) {
        let mut st = self.imp().state.borrow_mut();
        st.palette = palette;
        st.node_cache.clear();
        st.text_layouts.clear();
        drop(st);
        self.queue_draw();
    }

    /// Resolve a semantic color for toolbar previews (non-inverted view).
    pub fn preview_rgba(&self, c: SemanticColor) -> gdk::RGBA {
        let st = self.imp().state.borrow();
        match c {
            SemanticColor::Foreground => st.palette.ink,
            SemanticColor::Accent => st.palette.accent,
            SemanticColor::Fixed(Rgba { r, g, b, a }) => gdk::RGBA::new(r, g, b, a),
        }
    }

    pub fn set_inverted(&self, inverted: bool) {
        let mut st = self.imp().state.borrow_mut();
        if st.inverted != inverted {
            st.inverted = inverted;
            st.node_cache.clear();
            st.text_layouts.clear();
            drop(st);
            self.queue_draw();
        }
    }

    pub fn inverted(&self) -> bool {
        self.imp().state.borrow().inverted
    }

    pub fn set_background(&self, bg: PageBackground) {
        {
            let mut st = self.imp().state.borrow_mut();
            st.background = bg;
            st.text_layouts.clear();
        }
        self.sync_editor_geometry(true);
        self.queue_draw();
    }

    pub fn background(&self) -> PageBackground {
        self.imp().state.borrow().background
    }

    /// Update the current drawing color/width without changing the tool
    /// (used by the shapes flyout).
    pub fn set_draw_style(&self, color: SemanticColor, width: f64) {
        let mut st = self.imp().state.borrow_mut();
        st.cur_color = color;
        st.cur_width = width;
    }

    /// Copy the selection (ink + images) into the app clipboard. Also puts a
    /// text marker on the system clipboard so a stale screenshot there can't
    /// hijack the next paste. Returns the number of items copied.
    pub fn copy_selection(&self) -> usize {
        let mut st = self.imp().state.borrow_mut();
        let strokes: Vec<Stroke> = st
            .session
            .content
            .strokes
            .iter()
            .filter(|s| st.selection.contains(&s.id))
            .cloned()
            .collect();
        let images: Vec<ImageItem> = st
            .session
            .content
            .images
            .iter()
            .filter(|i| st.sel_images.contains(&i.id))
            .cloned()
            .collect();
        let texts: Vec<TextBox> = st
            .session
            .content
            .texts
            .iter()
            .filter(|t| st.sel_texts.contains(&t.id))
            .cloned()
            .collect();
        let n = strokes.len() + images.len() + texts.len();
        if n > 0 {
            // Selected text also goes to the system clipboard as plain text.
            let plain: Vec<String> = texts.iter().map(|t| t.plain_text()).collect();
            st.clipboard = strokes;
            st.clipboard_images = images;
            st.clipboard_texts = texts;
            st.clipboard_asset_dir = st.asset_dir.clone();
            drop(st);
            if plain.is_empty() {
                self.clipboard().set_text("OmaScratch selection");
            } else {
                self.clipboard().set_text(&plain.join("\n\n"));
            }
        }
        n
    }

    pub fn cut_selection(&self) {
        if self.copy_selection() > 0 {
            self.delete_selection();
        }
    }

    /// Paste: an image on the system clipboard wins (screenshots, browser
    /// copies); otherwise the app's copied ink/images.
    pub fn paste(&self) {
        let clip = self.clipboard();
        let formats = clip.formats();
        let has_image = formats.contains_type(gdk::Texture::static_type())
            || formats.mime_types().iter().any(|m| m.starts_with("image/"));
        if has_image {
            let weak = self.downgrade();
            clip.read_texture_async(gio::Cancellable::NONE, move |res| {
                let Some(view) = weak.upgrade() else { return };
                match res {
                    Ok(Some(tex)) => view.insert_image_texture(tex),
                    Ok(None) => view.paste_clipboard(),
                    Err(e) => {
                        tracing::warn!("clipboard image read failed: {e}");
                        view.paste_clipboard();
                    }
                }
            });
        } else {
            self.paste_clipboard();
        }
    }

    /// Paste the app clipboard slightly offset, select the pasted items.
    pub fn paste_clipboard(&self) {
        let mut st = self.imp().state.borrow_mut();
        if st.clipboard.is_empty() && st.clipboard_images.is_empty() && st.clipboard_texts.is_empty() {
            return;
        }
        let offset = 24.0 / st.zoom;
        let pasted: Vec<Stroke> = st
            .clipboard
            .iter()
            .map(|s| {
                let mut c = s.clone();
                c.id = StrokeId::new();
                for p in &mut c.points {
                    p.x += offset;
                    p.y += offset;
                }
                c
            })
            .collect();
        // Images keep their asset; bring the file along if it lives in
        // another note's asset dir.
        let mut images: Vec<ImageItem> = Vec::new();
        for img in st.clipboard_images.clone() {
            if let (Some(src), Some(dst)) = (st.clipboard_asset_dir.clone(), st.asset_dir.clone()) {
                if src != dst && !dst.join(&img.asset).exists() {
                    let _ = std::fs::create_dir_all(&dst);
                    if std::fs::copy(src.join(&img.asset), dst.join(&img.asset)).is_err() {
                        continue;
                    }
                }
            }
            let mut c = img.clone();
            c.id = ImageId::new();
            c.x += offset;
            c.y += offset;
            images.push(c);
        }
        let texts: Vec<TextBox> = st
            .clipboard_texts
            .iter()
            .map(|t| {
                let mut c = t.clone();
                c.id = TextId::new();
                c.x += offset;
                c.y += offset;
                c
            })
            .collect();
        st.selection = pasted.iter().map(|s| s.id).collect();
        st.sel_images = images.iter().map(|i| i.id).collect();
        st.sel_texts = texts.iter().map(|t| t.id).collect();
        Self::dispatch_combined(&mut st, vec![], pasted, vec![], images, vec![], texts);
        drop(st);
        self.queue_draw();
        self.notify_changed();
    }

    pub fn clipboard_has_strokes(&self) -> bool {
        let st = self.imp().state.borrow();
        !st.clipboard.is_empty() || !st.clipboard_images.is_empty() || !st.clipboard_texts.is_empty()
    }

    // ---- images ----

    /// Where the open note's image assets live (set when a note opens).
    pub fn set_asset_dir(&self, dir: PathBuf) {
        let mut st = self.imp().state.borrow_mut();
        st.asset_dir = Some(dir);
        st.textures.clear();
        drop(st);
        self.queue_draw();
    }

    pub fn set_asset_writer(&self, f: impl Fn(&[u8], &str) -> Option<String> + 'static) {
        self.imp().state.borrow_mut().asset_writer = Some(Box::new(f));
    }

    pub fn set_on_request_select(&self, f: impl Fn() + 'static) {
        self.imp().state.borrow_mut().on_request_select = Some(Box::new(f));
    }

    fn write_asset(&self, bytes: &[u8], ext: &str) -> Option<String> {
        let writer = self.imp().state.borrow_mut().asset_writer.take();
        let name = writer.as_ref().and_then(|w| w(bytes, ext));
        if let Some(w) = writer {
            self.imp().state.borrow_mut().asset_writer = Some(w);
        }
        name
    }

    /// Pasted texture (screenshots, browser copies): stored as PNG, placed at
    /// the view center.
    fn insert_image_texture(&self, tex: gdk::Texture) {
        self.insert_texture_at(tex, None);
    }

    fn insert_texture_at(&self, tex: gdk::Texture, at_widget: Option<(f64, f64)>) {
        let bytes = tex.save_to_png_bytes();
        let Some(name) = self.write_asset(&bytes, "png") else {
            tracing::error!("could not save pasted image");
            return;
        };
        self.place_image(tex, name, at_widget, false);
    }

    /// Image files (file picker): original bytes and format are kept, so
    /// photos stay JPEG-sized. Multiple files stagger from the view center
    /// and end up selected together.
    pub fn insert_image_files(&self, files: Vec<gio::File>, at_widget: Option<(f64, f64)>) {
        let view = self.clone();
        glib::spawn_future_local(async move {
            let mut placed = 0usize;
            let mut skipped = 0usize;
            for file in files {
                let Ok((bytes, _)) = file.load_bytes_future().await else {
                    skipped += 1;
                    continue;
                };
                let Ok(tex) = gdk::Texture::from_bytes(&bytes) else {
                    skipped += 1; // not an image this GTK can decode
                    continue;
                };
                let ext = file
                    .basename()
                    .and_then(|b| b.extension().map(|e| e.to_string_lossy().to_lowercase()))
                    .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
                    .unwrap_or_else(|| "img".to_string());
                let Some(name) = view.write_asset(&bytes, &ext) else {
                    skipped += 1;
                    continue;
                };
                let stagger = 24.0 * placed as f64;
                let at = at_widget.map(|(x, y)| (x + stagger, y + stagger));
                view.place_image(tex, name, at, placed > 0);
                placed += 1;
            }
            if skipped > 0 {
                tracing::warn!("{skipped} dropped/selected file(s) were not readable images");
            }
        });
    }

    /// Add an image element centered on `at_widget` (or the view center),
    /// scaled 1:1 on screen and capped to 60% of the viewport; select it
    /// (optionally adding to the current selection) and switch to Select.
    fn place_image(&self, tex: gdk::Texture, name: String, at_widget: Option<(f64, f64)>, add_to_selection: bool) {
        let (vw, vh) = (self.width().max(1) as f64, self.height().max(1) as f64);
        let mut st = self.imp().state.borrow_mut();
        let (pw, ph) = (tex.width().max(1) as f64, tex.height().max(1) as f64);
        let fit = (0.6 * vw / pw).min(0.6 * vh / ph).min(1.0);
        let (w, h) = (pw * fit / st.zoom, ph * fit / st.zoom);
        let (sx, sy) = at_widget.unwrap_or((vw / 2.0, vh / 2.0));
        let (cx, cy) = Self::widget_to_world(&st, sx, sy);
        let item = ImageItem {
            id: ImageId::new(),
            asset: name.clone(),
            x: cx - w / 2.0,
            y: cy - h / 2.0,
            w,
            h,
            pinned: false,
        };
        st.textures.insert(name, Some(tex));
        if !add_to_selection {
            st.selection.clear();
            st.sel_images.clear();
        }
        st.sel_images.insert(item.id);
        st.session.dispatch(Command::Replace {
            strokes_before: vec![],
            strokes_after: vec![],
            images_before: vec![],
            images_after: vec![item],
        });
        let request = st.on_request_select.take();
        drop(st);
        if let Some(r) = request {
            r();
            self.imp().state.borrow_mut().on_request_select = Some(r);
        }
        self.queue_draw();
        self.notify_changed();
    }

    /// Topmost image under a world point; pinned images only if asked.
    fn image_at(st: &State, wp: kurbo::Point, include_pinned: bool) -> Option<ImageId> {
        st.session
            .content
            .images
            .iter()
            .rev()
            .find(|i| (include_pinned || !i.pinned) && i.rect().contains(wp))
            .map(|i| i.id)
    }

    fn set_image_pinned(&self, id: ImageId, pinned: bool) {
        let mut st = self.imp().state.borrow_mut();
        let Some(idx) = st.session.content.image_index(id) else { return };
        let before = st.session.content.images[idx].clone();
        if before.pinned == pinned {
            return;
        }
        let mut after = before.clone();
        after.pinned = pinned;
        if pinned {
            st.sel_images.remove(&id);
        }
        st.session.dispatch(Command::Replace {
            strokes_before: vec![],
            strokes_after: vec![],
            images_before: vec![before],
            images_after: vec![after],
        });
        drop(st);
        self.queue_draw();
        self.notify_changed();
    }

    fn delete_image(&self, id: ImageId) {
        let mut st = self.imp().state.borrow_mut();
        let Some(idx) = st.session.content.image_index(id) else { return };
        let before = st.session.content.images[idx].clone();
        st.sel_images.remove(&id);
        st.session.dispatch(Command::Replace {
            strokes_before: vec![],
            strokes_after: vec![],
            images_before: vec![before],
            images_after: vec![],
        });
        drop(st);
        self.queue_draw();
        self.notify_changed();
    }

    /// Context menu for an image (right-click, or long-press in Select).
    fn show_image_menu(&self, x: f64, y: f64, id: ImageId) {
        let pinned = {
            let st = self.imp().state.borrow();
            match st.session.content.image_index(id) {
                Some(i) => st.session.content.images[i].pinned,
                None => return,
            }
        };
        let popover = gtk::Popover::new();
        popover.set_parent(self);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.set_has_arrow(false);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_top(4);
        vbox.set_margin_bottom(4);
        vbox.set_margin_start(4);
        vbox.set_margin_end(4);
        let pin = gtk::Button::with_label(if pinned { "Unpin from background" } else { "Pin to background" });
        let del = gtk::Button::with_label("Delete image");
        for b in [&pin, &del] {
            b.add_css_class("flat");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
        }
        del.add_css_class("destructive-action");
        vbox.append(&pin);
        vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        vbox.append(&del);
        popover.set_child(Some(&vbox));

        let view = self.clone();
        let pop = popover.clone();
        pin.connect_clicked(move |_| {
            pop.popdown();
            view.set_image_pinned(id, !pinned);
        });
        let view = self.clone();
        let pop = popover.clone();
        del.connect_clicked(move |_| {
            pop.popdown();
            view.delete_image(id);
        });
        // Unparent once closed (deferred: never inside the popover's own signal).
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }

    pub fn clear_selection(&self) {
        let mut st = self.imp().state.borrow_mut();
        st.selection.clear();
        st.sel_images.clear();
        st.sel_texts.clear();
        st.sel_drag = None;
        st.sel_resize = None;
        st.marquee = None;
        st.sel_offset = kurbo::Vec2::ZERO;
        drop(st);
        self.queue_draw();
    }

    pub fn has_selection(&self) -> bool {
        let st = self.imp().state.borrow();
        !st.selection.is_empty() || !st.sel_images.is_empty() || !st.sel_texts.is_empty()
    }

    /// Delete the current selection (ink + images) as one undoable command.
    pub fn delete_selection(&self) {
        let mut st = self.imp().state.borrow_mut();
        if st.selection.is_empty() && st.sel_images.is_empty() && st.sel_texts.is_empty() {
            return;
        }
        let ids: Vec<StrokeId> = st.selection.drain().collect();
        let img_ids: Vec<ImageId> = st.sel_images.drain().collect();
        let txt_ids: Vec<TextId> = st.sel_texts.drain().collect();
        let removed_texts: Vec<TextBox> = txt_ids
            .iter()
            .filter_map(|id| st.session.content.text_index(*id).map(|i| st.session.content.texts[i].clone()))
            .collect();
        let removed: Vec<Stroke> = ids
            .iter()
            .filter_map(|id| st.session.content.stroke_index(*id).map(|i| st.session.content.strokes[i].clone()))
            .collect();
        let removed_images: Vec<ImageItem> = img_ids
            .iter()
            .filter_map(|id| st.session.content.image_index(*id).map(|i| st.session.content.images[i].clone()))
            .collect();
        for id in &ids {
            st.node_cache.remove(id);
        }
        Self::dispatch_combined(&mut st, removed, vec![], removed_images, vec![], removed_texts, vec![]);
        drop(st);
        self.queue_draw();
        self.notify_changed();
    }

    /// Dispatch ink/image and text replacements as ONE undo step.
    fn dispatch_combined(
        st: &mut State,
        strokes_before: Vec<Stroke>,
        strokes_after: Vec<Stroke>,
        images_before: Vec<ImageItem>,
        images_after: Vec<ImageItem>,
        texts_before: Vec<TextBox>,
        texts_after: Vec<TextBox>,
    ) {
        let mut cmds = Vec::new();
        if !(strokes_before.is_empty() && strokes_after.is_empty() && images_before.is_empty() && images_after.is_empty()) {
            cmds.push(Command::Replace { strokes_before, strokes_after, images_before, images_after });
        }
        if !(texts_before.is_empty() && texts_after.is_empty()) {
            cmds.push(Command::ReplaceTexts { before: texts_before, after: texts_after });
        }
        match cmds.len() {
            0 => {}
            1 => st.session.dispatch(cmds.pop().unwrap()),
            _ => st.session.dispatch(Command::Batch(cmds)),
        }
    }

    /// Configure and activate the eraser.
    pub fn set_eraser(&self, kind: EraserKind, radius: f64) {
        self.commit_text_edit();
        let mut st = self.imp().state.borrow_mut();
        st.active = ActiveTool::Eraser;
        st.eraser_kind = kind;
        st.eraser_radius = radius;
        drop(st);
        self.queue_draw();
    }

    fn notify_changed(&self) {
        // Take the hook out of the RefCell so the callback can re-enter
        // canvas methods (content_snapshot etc.) without a borrow panic.
        let hook = self.imp().state.borrow_mut().on_change.take();
        if let Some(hook) = hook {
            hook();
            let mut st = self.imp().state.borrow_mut();
            if st.on_change.is_none() {
                st.on_change = Some(hook);
            }
        }
    }

    pub fn toggle_debug_overlay(&self) {
        let mut st = self.imp().state.borrow_mut();
        st.debug.enabled = !st.debug.enabled;
        drop(st);
        self.queue_draw();
    }

    /// Home: scroll to the page's top-left corner, keeping the zoom.
    pub fn go_home(&self) {
        self.imp().state.borrow_mut().offset = kurbo::Vec2::ZERO;
        self.queue_draw();
    }

    pub fn reset_view(&self) {
        let mut st = self.imp().state.borrow_mut();
        st.offset = kurbo::Vec2::ZERO;
        st.zoom = 1.0;
        drop(st);
        self.queue_draw();
        self.notify_zoom();
    }

    // ---- input wiring ----

    fn setup_input(&self) {
        // Stylus: the primary input path. Uncompressed history via backlog().
        let stylus = gtk::GestureStylus::new();
        let weak = self.downgrade();
        stylus.connect_down(move |g, x, y| {
            if let Some(view) = weak.upgrade() {
                view.stylus_begin(g, x, y);
            }
        });
        let weak = self.downgrade();
        stylus.connect_motion(move |g, x, y| {
            if let Some(view) = weak.upgrade() {
                view.imp().state.borrow_mut().shift_down =
                    g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
                view.stylus_motion(g, x, y);
            }
        });
        let weak = self.downgrade();
        stylus.connect_up(move |g, x, y| {
            if let Some(view) = weak.upgrade() {
                view.stylus_motion(g, x, y);
                view.commit_live();
            }
        });
        self.add_controller(stylus);

        // Mouse drawing fallback (primary button, synthetic pressure).
        let draw = gtk::GestureDrag::new();
        draw.set_button(gdk::BUTTON_PRIMARY);
        let weak = self.downgrade();
        draw.connect_drag_begin(move |g, x, y| {
            let Some(view) = weak.upgrade() else { return };
            // A stylus press also emulates a primary-button drag; the stylus
            // gesture already handles it, so ignore events carrying a tool.
            if g.current_event().and_then(|ev| ev.device_tool()).is_some() {
                return;
            }
            view.imp().state.borrow_mut().shift_down =
                g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            view.mouse_begin(x, y);
        });
        let weak = self.downgrade();
        draw.connect_drag_update(move |g, dx, dy| {
            let Some(view) = weak.upgrade() else { return };
            if g.current_event().and_then(|ev| ev.device_tool()).is_some() {
                return;
            }
            view.imp().state.borrow_mut().shift_down =
                g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            if let Some((sx, sy)) = g.start_point() {
                view.mouse_point(sx + dx, sy + dy);
            }
        });
        let weak = self.downgrade();
        draw.connect_drag_end(move |g, _, _| {
            let Some(view) = weak.upgrade() else { return };
            if g.current_event().and_then(|ev| ev.device_tool()).is_some() {
                return;
            }
            view.commit_live();
        });
        self.add_controller(draw);

        // Middle-button pan.
        let pan = gtk::GestureDrag::new();
        pan.set_button(gdk::BUTTON_MIDDLE);
        let weak = self.downgrade();
        pan.connect_drag_begin(move |g, _, _| {
            // Stylus barrel buttons emulate middle-click; they must never pan.
            if g.current_event().and_then(|ev| ev.device_tool()).is_some() {
                return;
            }
            if let Some(view) = weak.upgrade() {
                let mut st = view.imp().state.borrow_mut();
                st.pan_anchor = Some(st.offset);
            }
        });
        let weak = self.downgrade();
        pan.connect_drag_update(move |g, dx, dy| {
            let Some(view) = weak.upgrade() else { return };
            if g.current_event().and_then(|ev| ev.device_tool()).is_some() {
                return;
            }
            let mut st = view.imp().state.borrow_mut();
            if let Some(anchor) = st.pan_anchor {
                let zoom = st.zoom;
                st.offset = anchor - kurbo::Vec2::new(dx, dy) / zoom;
                Self::clamp_offset(&mut st);
                drop(st);
                view.queue_draw();
            }
        });
        pan.connect_drag_end(move |_, _, _| {});
        self.add_controller(pan);

        // Stylus barrel click (arrives as middle/secondary with a device
        // tool): toggle the eraser instead of panning or context-clicking.
        for button in [gdk::BUTTON_MIDDLE, gdk::BUTTON_SECONDARY] {
            let click = gtk::GestureClick::new();
            click.set_button(button);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            let weak = self.downgrade();
            click.connect_pressed(move |g, _, x, y| {
                let Some(view) = weak.upgrade() else { return };
                if g.current_event().and_then(|ev| ev.device_tool()).is_some() {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    view.fire_eraser_toggle();
                    return;
                }
                // Mouse right-click on an image: its context menu.
                if g.current_button() == gdk::BUTTON_SECONDARY {
                    let hit = {
                        let st = view.imp().state.borrow();
                        let (wx, wy) = Self::widget_to_world(&st, x, y);
                        Self::image_at(&st, kurbo::Point::new(wx, wy), true)
                    };
                    if let Some(id) = hit {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        view.show_image_menu(x, y, id);
                    }
                }
            });
            self.add_controller(click);
        }

        // Double-click a text box in Select mode to edit it.
        let dbl = gtk::GestureClick::new();
        dbl.set_button(gdk::BUTTON_PRIMARY);
        dbl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        dbl.connect_pressed(move |_, n, x, y| {
            if n != 2 {
                return;
            }
            let Some(view) = weak.upgrade() else { return };
            let hit = {
                let mut st = view.imp().state.borrow_mut();
                if st.active != ActiveTool::Select {
                    return;
                }
                let (wx, wy) = Self::widget_to_world(&st, x, y);
                let wp = kurbo::Point::new(wx, wy);
                let hit = Self::text_at(&st, wp)
                    .and_then(|id| st.session.content.text_index(id))
                    .map(|i| (st.session.content.texts[i].clone(), wp));
                if hit.is_some() {
                    st.sel_drag = None;
                    st.sel_offset = kurbo::Vec2::ZERO;
                }
                hit
            };
            if let Some((tb, wp)) = hit {
                view.start_edit(Some(tb), wp, Some((x, y)));
            }
        });
        self.add_controller(dbl);

        // Pen-friendly context menu: long-press an image in Select mode
        // (also reaches pinned images, so they can be unpinned).
        let long = gtk::GestureLongPress::new();
        let weak = self.downgrade();
        long.connect_pressed(move |_, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let hit = {
                let mut st = view.imp().state.borrow_mut();
                if st.active != ActiveTool::Select {
                    return;
                }
                let (wx, wy) = Self::widget_to_world(&st, x, y);
                let hit = Self::image_at(&st, kurbo::Point::new(wx, wy), true);
                if hit.is_some() {
                    // Cancel the drag the press started; the menu takes over.
                    st.sel_drag = None;
                    st.sel_resize = None;
                    st.sel_offset = kurbo::Vec2::ZERO;
                }
                hit
            };
            if let Some(id) = hit {
                view.show_image_menu(x, y, id);
                view.queue_draw();
            }
        });
        self.add_controller(long);


        // Scroll = pan; Ctrl+scroll = zoom around the pointer.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let weak = self.downgrade();
        scroll.connect_scroll(move |c, dx, dy| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let ctrl = c
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl {
                view.zoom_by(1.1_f64.powf(-dy));
            } else {
                let mut st = view.imp().state.borrow_mut();
                let step = 40.0 / st.zoom;
                st.offset += kurbo::Vec2::new(dx * step, dy * step);
                Self::clamp_offset(&mut st);
                drop(st);
                view.queue_draw();
            }
            glib::Propagation::Stop
        });
        self.add_controller(scroll);

        // Track the pointer so zoom can anchor on it.
        let motion = gtk::EventControllerMotion::new();
        let weak = self.downgrade();
        motion.connect_motion(move |_, x, y| {
            if let Some(view) = weak.upgrade() {
                view.imp().state.borrow_mut().pointer = (x, y);
            }
        });
        self.add_controller(motion);
    }

    /// The canvas is pinned at a top-left home: the viewport never scrolls
    /// above or left of the page origin, so the margin and rule lines always
    /// have a fixed home regardless of pan/zoom.
    fn clamp_offset(st: &mut State) {
        if st.offset.x < 0.0 {
            st.offset.x = 0.0;
        }
        if st.offset.y < 0.0 {
            st.offset.y = 0.0;
        }
    }

    fn zoom_by(&self, factor: f64) {
        let mut st = self.imp().state.borrow_mut();
        let old = st.zoom;
        let new = (old * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if (new - old).abs() < f64::EPSILON {
            return;
        }
        // Keep the world point under the pointer fixed on screen.
        let (px, py) = st.pointer;
        let before = kurbo::Vec2::new(px / old, py / old);
        let after = kurbo::Vec2::new(px / new, py / new);
        st.offset += before - after;
        st.zoom = new;
        Self::clamp_offset(&mut st);
        drop(st);
        self.queue_draw();
        self.notify_zoom();
    }

    // ---- stroke input ----

    fn widget_to_world(st: &State, x: f64, y: f64) -> (f64, f64) {
        (st.offset.x + x / st.zoom, st.offset.y + y / st.zoom)
    }

    fn stylus_begin(&self, g: &gtk::GestureStylus, x: f64, y: f64) {
        if self.text_press(x, y) {
            return;
        }
        self.grab_focus();
        let state = g.current_event_state();
        // XP-Pen barrel buttons arrive as middle/secondary button masks (or an
        // eraser-type tool). Holding one at pen-down erases for that stroke.
        let barrel = state.intersects(
            gdk::ModifierType::BUTTON2_MASK | gdk::ModifierType::BUTTON3_MASK,
        );
        let mut st = self.imp().state.borrow_mut();
        st.shift_down = state.contains(gdk::ModifierType::SHIFT_MASK);
        let mut force_erase = barrel;
        if let Some(t) = g.device_tool() {
            st.debug.tool_name = format!("{:?}", t.tool_type());
            if t.tool_type() == gdk::DeviceToolType::Eraser {
                force_erase = true;
            }
        }
        st.debug.buttons = format!(
            "b1:{} b2:{} b3:{}",
            state.contains(gdk::ModifierType::BUTTON1_MASK) as u8,
            state.contains(gdk::ModifierType::BUTTON2_MASK) as u8,
            state.contains(gdk::ModifierType::BUTTON3_MASK) as u8,
        );
        Self::begin_locked(&mut st, x, y, false);
        if force_erase && st.active.as_draw_tool().is_some() {
            // Override the drawing gesture with a temporary eraser drag; the
            // selected tool (and its toolbar underline) stay unchanged.
            st.live = None;
            st.erasing = true;
            st.live_erased.clear();
            st.erase_path.clear();
        }
        drop(st);
        self.push_stylus_point(g, x, y);
    }

    /// Start the gesture appropriate for the active tool at widget (x, y).
    fn begin_locked(st: &mut State, x: f64, y: f64, is_mouse: bool) {
        let (wx, wy) = Self::widget_to_world(st, x, y);
        let wp = kurbo::Point::new(wx, wy);
        st.sel_drag = None;
        match st.active {
            ActiveTool::Pen | ActiveTool::Pencil | ActiveTool::Highlighter => {
                let tool = st.active.as_draw_tool().unwrap();
                st.erasing = false;
                st.live = Some(LiveStroke {
                    tool,
                    color: st.cur_color,
                    width: st.cur_width,
                    points: Vec::with_capacity(256),
                    started: Instant::now(),
                    t0_ms: now_unix_ms(),
                    simulate_pressure: is_mouse,
                });
            }
            ActiveTool::Eraser => {
                st.erasing = true;
                st.live = None;
                st.live_erased.clear();
                st.erase_path.clear();
            }
            ActiveTool::Lasso => {
                if let Some((anchor, start)) = Self::handle_hit(st, wp) {
                    st.sel_resize = Some(ResizeDrag { anchor, start, factor: 1.0 });
                } else if Self::selection_bounds(st).is_some_and(|b| b.contains(wp)) {
                    // Drag inside the selection moves it.
                    st.sel_drag = Some(wp);
                    st.sel_offset = kurbo::Vec2::ZERO;
                } else {
                    st.lassoing = true;
                    st.lasso_path.clear();
                    st.lasso_path.push(wp);
                }
            }
            ActiveTool::Select => {
                let additive = st.shift_down;
                if !additive {
                    if let Some((anchor, start)) = Self::handle_hit(st, wp) {
                        st.sel_resize = Some(ResizeDrag { anchor, start, factor: 1.0 });
                        return;
                    }
                    if Self::selection_bounds(st).is_some_and(|b| b.contains(wp)) {
                        st.sel_drag = Some(wp);
                        st.sel_offset = kurbo::Vec2::ZERO;
                        return;
                    }
                }
                // Click an object to select it (Shift toggles it in/out of
                // the selection); a plain click also allows dragging at once.
                let radius = 6.0 / st.zoom;
                Self::ensure_bounds(st);
                let stroke_hit = st
                    .session
                    .content
                    .strokes
                    .iter()
                    .zip(st.bounds.iter())
                    .rev()
                    .filter(|(_, b)| b.is_some_and(|b| b.inflate(radius, radius).contains(wp)))
                    .find(|(s, _)| omascratch_ink::stroke_hit(&s.points, s.width, wp, radius))
                    .map(|(s, _)| s.id);
                let text_hit = if stroke_hit.is_none() { Self::text_at(st, wp) } else { None };
                let image_hit = if stroke_hit.is_none() && text_hit.is_none() {
                    Self::image_at(st, wp, false)
                } else {
                    None
                };
                if additive {
                    if let Some(id) = stroke_hit {
                        if !st.selection.remove(&id) {
                            st.selection.insert(id);
                        }
                    } else if let Some(id) = text_hit {
                        if !st.sel_texts.remove(&id) {
                            st.sel_texts.insert(id);
                        }
                    } else if let Some(id) = image_hit {
                        if !st.sel_images.remove(&id) {
                            st.sel_images.insert(id);
                        }
                    } else {
                        // Shift-drag on empty canvas: add a rectangle.
                        st.marquee = Some((wp, wp));
                    }
                    return;
                }
                st.selection.clear();
                st.sel_images.clear();
                st.sel_texts.clear();
                if let Some(id) = stroke_hit {
                    st.selection.insert(id);
                    st.sel_drag = Some(wp);
                    st.sel_offset = kurbo::Vec2::ZERO;
                } else if let Some(id) = text_hit {
                    st.sel_texts.insert(id);
                    st.sel_drag = Some(wp);
                    st.sel_offset = kurbo::Vec2::ZERO;
                } else if let Some(img) = image_hit {
                    st.sel_images.insert(img);
                    st.sel_drag = Some(wp);
                    st.sel_offset = kurbo::Vec2::ZERO;
                } else {
                    // Empty canvas: start a rubber-band selection rectangle.
                    st.marquee = Some((wp, wp));
                }
            }
            ActiveTool::Shape => {
                st.shape_drag = Some((wp, wp));
            }
            ActiveTool::Pan => {
                st.panning = Some((x, y, st.offset));
            }
            // Text clicks are handled before gestures start (text_press).
            ActiveTool::Text => {}
        }
    }

    /// Union bounds of the selected strokes and images (no drag offset).
    fn selection_bounds(st: &State) -> Option<kurbo::Rect> {
        let mut out: Option<kurbo::Rect> = None;
        for s in &st.session.content.strokes {
            if st.selection.contains(&s.id) {
                if let Some(b) = s.bounds() {
                    out = Some(out.map_or(b, |o| o.union(b)));
                }
            }
        }
        for i in &st.session.content.images {
            if st.sel_images.contains(&i.id) {
                let b = i.rect();
                out = Some(out.map_or(b, |o| o.union(b)));
            }
        }
        for t in &st.session.content.texts {
            if st.sel_texts.contains(&t.id) {
                let b = t.rect();
                out = Some(out.map_or(b, |o| o.union(b)));
            }
        }
        out
    }

    /// Line pitch for text on this page (rule spacing multiples).
    fn text_cell(st: &State, font: f64) -> f64 {
        textmod::cell_height(st.background.spacing, font)
    }

    /// Semantic → screen color for text on the current page.
    fn text_resolver(st: &State) -> impl Fn(SemanticColor) -> gdk::RGBA {
        let (inverted, palette) = (st.inverted, st.palette);
        move |c| resolve_color_inv(c, Tool::Pen, inverted, &palette)
    }

    /// Topmost text box under a world point (excluding the one being edited).
    fn text_at(st: &State, wp: kurbo::Point) -> Option<TextId> {
        let editing = st.editing.as_ref().map(|e| e.working.id);
        st.session
            .content
            .texts
            .iter()
            .rev()
            .find(|t| Some(t.id) != editing && t.rect().contains(wp))
            .map(|t| t.id)
    }

    /// Decoded texture for an asset, loaded lazily from the note's asset dir.
    fn texture_for(st: &mut State, asset: &str) -> Option<gdk::Texture> {
        if let Some(t) = st.textures.get(asset) {
            return t.clone();
        }
        let tex = st
            .asset_dir
            .as_ref()
            .and_then(|d| gdk::Texture::from_filename(d.join(asset)).ok());
        st.textures.insert(asset.to_string(), tex.clone());
        tex
    }

    /// Corner handle under `wp`: returns (anchor = opposite corner, corner).
    fn handle_hit(st: &State, wp: kurbo::Point) -> Option<(kurbo::Point, kurbo::Point)> {
        let b = Self::selection_bounds(st)?;
        let r = 10.0 / st.zoom;
        let corners = [
            (kurbo::Point::new(b.x0, b.y0), kurbo::Point::new(b.x1, b.y1)),
            (kurbo::Point::new(b.x1, b.y0), kurbo::Point::new(b.x0, b.y1)),
            (kurbo::Point::new(b.x1, b.y1), kurbo::Point::new(b.x0, b.y0)),
            (kurbo::Point::new(b.x0, b.y1), kurbo::Point::new(b.x1, b.y0)),
        ];
        corners
            .into_iter()
            .find(|(c, _)| (*c - wp).hypot() <= r)
            .map(|(c, anchor)| (anchor, c))
    }

    /// Proportional scale of a point about an anchor.
    fn scale_about(p: kurbo::Point, anchor: kurbo::Point, f: f64) -> kurbo::Point {
        anchor + (p - anchor) * f
    }

    fn stylus_motion(&self, g: &gtk::GestureStylus, x: f64, y: f64) {
        {
            let st = self.imp().state.borrow();
            let gesture_active = st.live.is_some()
                || st.erasing
                || st.lassoing
                || st.sel_drag.is_some()
                || st.sel_resize.is_some()
                || st.marquee.is_some()
                || st.shape_drag.is_some()
                || st.panning.is_some();
            if !gesture_active {
                return;
            }
        }
        // Drain the uncompressed event history first (GTK compresses plain
        // motion events; the backlog preserves every stylus sample).
        if let Some(backlog) = g.backlog() {
            let mut st = self.imp().state.borrow_mut();
            for coord in &backlog {
                let axes = coord.axes();
                let bx = axes[axis_index(gdk::AxisUse::X)];
                let by = axes[axis_index(gdk::AxisUse::Y)];
                let pressure = axes[axis_index(gdk::AxisUse::Pressure)];
                let tilt_x = axes[axis_index(gdk::AxisUse::Xtilt)];
                let tilt_y = axes[axis_index(gdk::AxisUse::Ytilt)];
                Self::push_point_locked(&mut st, bx, by, pressure, tilt_x, tilt_y);
            }
        }
        self.push_stylus_point(g, x, y);
    }

    fn push_stylus_point(&self, g: &gtk::GestureStylus, x: f64, y: f64) {
        let pressure = g.axis(gdk::AxisUse::Pressure).unwrap_or(0.5);
        let tilt_x = g.axis(gdk::AxisUse::Xtilt).unwrap_or(0.0);
        let tilt_y = g.axis(gdk::AxisUse::Ytilt).unwrap_or(0.0);
        let mut st = self.imp().state.borrow_mut();
        Self::push_point_locked(&mut st, x, y, pressure, tilt_x, tilt_y);
        drop(st);
        self.queue_draw();
    }

    /// Rebuild the bounds cache if content changed since it was built; also
    /// prunes render nodes of strokes that no longer exist.
    fn ensure_bounds(st: &mut State) {
        let rev = st.session.revision();
        if st.bounds_rev == rev && st.bounds.len() == st.session.content.strokes.len() {
            return;
        }
        st.bounds = st.session.content.strokes.iter().map(|s| s.bounds()).collect();
        let present: HashSet<StrokeId> = st.session.content.strokes.iter().map(|s| s.id).collect();
        st.node_cache.retain(|id, _| present.contains(id));
        st.text_layouts.clear();
        st.bounds_rev = rev;
    }

    fn erase_at(st: &mut State, wx: f64, wy: f64) {
        let radius = st.eraser_radius;
        let p = kurbo::Point::new(wx, wy);
        if st.eraser_kind == EraserKind::Area {
            // Thin the path: a new point only matters once it has moved a
            // fraction of the radius.
            let far_enough = st
                .erase_path
                .last()
                .map(|last| (*last - p).hypot() >= radius * 0.35)
                .unwrap_or(true);
            if far_enough {
                st.erase_path.push(p);
            }
        }
        Self::ensure_bounds(st);
        let mut hits = Vec::new();
        for (i, s) in st.session.content.strokes.iter().enumerate() {
            // Cheap reject: point not within the stroke's (width-padded)
            // bounds grown by the eraser radius.
            match st.bounds.get(i).copied().flatten() {
                Some(b) if b.inflate(radius, radius).contains(p) => {}
                _ => continue,
            }
            if st.live_erased.contains(&s.id) {
                continue;
            }
            if omascratch_ink::stroke_hit(&s.points, s.width, p, radius) {
                hits.push(s.id);
            }
        }
        for id in hits {
            st.live_erased.insert(id);
        }
    }

    fn push_point_locked(st: &mut State, x: f64, y: f64, pressure: f64, tilt_x: f64, tilt_y: f64) {
        let (wx, wy) = Self::widget_to_world(st, x, y);
        let wp = kurbo::Point::new(wx, wy);
        st.pointer = (x, y);
        st.debug.sample_times.push_back(Instant::now());
        if let Some((sx, sy, start_offset)) = st.panning {
            st.offset = start_offset - kurbo::Vec2::new(x - sx, y - sy) / st.zoom;
            Self::clamp_offset(st);
            return;
        }
        if st.erasing {
            Self::erase_at(st, wx, wy);
            return;
        }
        if st.lassoing {
            let far = st.lasso_path.last().map(|l| (*l - wp).hypot() >= 2.0 / st.zoom).unwrap_or(true);
            if far {
                st.lasso_path.push(wp);
            }
            return;
        }
        if let Some((_, cur)) = st.marquee.as_mut() {
            *cur = wp;
            return;
        }
        if let Some(mut r) = st.sel_resize {
            // Project the pointer onto the anchor→corner diagonal.
            let d0 = r.start - r.anchor;
            let len2 = d0.hypot2().max(1e-9);
            r.factor = ((wp - r.anchor).dot(d0) / len2).max(0.05);
            st.sel_resize = Some(r);
            return;
        }
        if let Some(last) = st.sel_drag {
            st.sel_offset += wp - last;
            st.sel_drag = Some(wp);
            return;
        }
        if let Some((start, cur)) = st.shape_drag.as_mut() {
            *cur = if st.shift_down { constrain_shape(st.shape_kind, *start, wp) } else { wp };
            return;
        }
        // A zero pressure sample on a device that reports pressure is a
        // proximity artifact; clamp into a drawable range instead of a gap.
        let p = if pressure <= 0.0 { 0.05 } else { pressure.min(1.0) };
        st.debug.last_pressure = pressure;
        st.debug.min_pressure = st.debug.min_pressure.min(pressure);
        st.debug.max_pressure = st.debug.max_pressure.max(pressure);
        st.debug.last_tilt = (tilt_x, tilt_y);
        if let Some(live) = st.live.as_mut() {
            let dt = live.started.elapsed().as_millis().min(u16::MAX as u128) as u16;
            live.points.push(InkPoint {
                x: wx,
                y: wy,
                pressure: p as f32,
                tilt_x: tilt_x as f32,
                tilt_y: tilt_y as f32,
                dt_ms: dt,
            });
        }
    }

    fn mouse_begin(&self, x: f64, y: f64) {
        if self.text_press(x, y) {
            return;
        }
        self.grab_focus();
        let mut st = self.imp().state.borrow_mut();
        st.debug.tool_name = "Mouse".into();
        Self::begin_locked(&mut st, x, y, true);
        Self::push_point_locked(&mut st, x, y, 0.5, 0.0, 0.0);
        drop(st);
        self.queue_draw();
    }

    fn mouse_point(&self, x: f64, y: f64) {
        let mut st = self.imp().state.borrow_mut();
        Self::push_point_locked(&mut st, x, y, 0.5, 0.0, 0.0);
        drop(st);
        self.queue_draw();
    }

    fn commit_live(&self) {
        let mut st = self.imp().state.borrow_mut();

        if st.panning.take().is_some() {
            drop(st);
            self.queue_draw();
            return;
        }

        // Lasso finished: everything fully inside the loop becomes selected.
        if st.lassoing {
            st.lassoing = false;
            let poly = std::mem::take(&mut st.lasso_path);
            st.selection = st
                .session
                .content
                .strokes
                .iter()
                .filter(|s| omascratch_ink::stroke_inside_polygon(&s.points, &poly))
                .map(|s| s.id)
                .collect();
            // Unpinned images whose four corners are inside the loop.
            st.sel_images = st
                .session
                .content
                .images
                .iter()
                .filter(|i| !i.pinned)
                .filter(|i| {
                    let r = i.rect();
                    [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
                        .iter()
                        .all(|(x, y)| omascratch_ink::point_in_polygon(kurbo::Point::new(*x, *y), &poly))
                })
                .map(|i| i.id)
                .collect();
            st.sel_texts = st
                .session
                .content
                .texts
                .iter()
                .filter(|t| {
                    let r = t.rect();
                    [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
                        .iter()
                        .all(|(x, y)| omascratch_ink::point_in_polygon(kurbo::Point::new(*x, *y), &poly))
                })
                .map(|t| t.id)
                .collect();
            drop(st);
            self.queue_draw();
            return;
        }

        // Rubber-band finished: select every object the rectangle touches.
        if let Some((a, b)) = st.marquee.take() {
            let rect = kurbo::Rect::from_points(a, b);
            if rect.width() * st.zoom >= 3.0 || rect.height() * st.zoom >= 3.0 {
                let hits: Vec<StrokeId> = st
                    .session
                    .content
                    .strokes
                    .iter()
                    .filter(|s| {
                        let r = rect.inflate(s.width / 2.0, s.width / 2.0);
                        s.points.iter().any(|p| r.contains(kurbo::Point::new(p.x, p.y)))
                    })
                    .map(|s| s.id)
                    .collect();
                let img_hits: Vec<ImageId> = st
                    .session
                    .content
                    .images
                    .iter()
                    .filter(|i| !i.pinned && !i.rect().intersect(rect).is_zero_area())
                    .map(|i| i.id)
                    .collect();
                let txt_hits: Vec<TextId> = st
                    .session
                    .content
                    .texts
                    .iter()
                    .filter(|t| !t.rect().intersect(rect).is_zero_area())
                    .map(|t| t.id)
                    .collect();
                st.selection.extend(hits);
                st.sel_images.extend(img_hits);
                st.sel_texts.extend(txt_hits);
            }
            drop(st);
            self.queue_draw();
            return;
        }

        // Resize finished: one exact Replace for strokes + images.
        if let Some(r) = st.sel_resize.take() {
            if (r.factor - 1.0).abs() >= 0.001 {
                let (before_s, after_s): (Vec<Stroke>, Vec<Stroke>) = st
                    .session
                    .content
                    .strokes
                    .iter()
                    .filter(|s| st.selection.contains(&s.id))
                    .map(|s| {
                        let mut a = s.clone();
                        for p in &mut a.points {
                            let q = Self::scale_about(kurbo::Point::new(p.x, p.y), r.anchor, r.factor);
                            p.x = q.x;
                            p.y = q.y;
                        }
                        a.width = (a.width * r.factor).max(0.3);
                        (s.clone(), a)
                    })
                    .unzip();
                let (before_i, after_i): (Vec<ImageItem>, Vec<ImageItem>) = st
                    .session
                    .content
                    .images
                    .iter()
                    .filter(|i| st.sel_images.contains(&i.id))
                    .map(|i| {
                        let mut a = i.clone();
                        let tl = Self::scale_about(kurbo::Point::new(i.x, i.y), r.anchor, r.factor);
                        a.x = tl.x;
                        a.y = tl.y;
                        a.w = i.w * r.factor;
                        a.h = i.h * r.factor;
                        (i.clone(), a)
                    })
                    .unzip();
                let (before_t, after_t): (Vec<TextBox>, Vec<TextBox>) = st
                    .session
                    .content
                    .texts
                    .iter()
                    .filter(|t| st.sel_texts.contains(&t.id))
                    .map(|t| {
                        let mut a = t.clone();
                        let tl = Self::scale_about(kurbo::Point::new(t.x, t.y), r.anchor, r.factor);
                        a.x = tl.x;
                        a.y = tl.y;
                        a.w = t.w * r.factor;
                        a.h = t.h * r.factor;
                        a.font_size = (t.font_size * r.factor).max(4.0);
                        (t.clone(), a)
                    })
                    .unzip();
                for s in &before_s {
                    st.node_cache.remove(&s.id);
                }
                Self::dispatch_combined(&mut st, before_s, after_s, before_i, after_i, before_t, after_t);
                drop(st);
                self.queue_draw();
                self.notify_changed();
                return;
            }
            drop(st);
            self.queue_draw();
            return;
        }

        // Selection move finished: commit the accumulated offset.
        if st.sel_drag.take().is_some() {
            let offset = std::mem::replace(&mut st.sel_offset, kurbo::Vec2::ZERO);
            if offset.hypot() >= 0.01
                && (!st.selection.is_empty() || !st.sel_images.is_empty() || !st.sel_texts.is_empty())
            {
                let (before_s, after_s): (Vec<Stroke>, Vec<Stroke>) = st
                    .session
                    .content
                    .strokes
                    .iter()
                    .filter(|s| st.selection.contains(&s.id))
                    .map(|s| {
                        let mut a = s.clone();
                        for p in &mut a.points {
                            p.x += offset.x;
                            p.y += offset.y;
                        }
                        (s.clone(), a)
                    })
                    .unzip();
                let (before_i, after_i): (Vec<ImageItem>, Vec<ImageItem>) = st
                    .session
                    .content
                    .images
                    .iter()
                    .filter(|i| st.sel_images.contains(&i.id))
                    .map(|i| {
                        let mut a = i.clone();
                        a.x += offset.x;
                        a.y += offset.y;
                        (i.clone(), a)
                    })
                    .unzip();
                let (before_t, after_t): (Vec<TextBox>, Vec<TextBox>) = st
                    .session
                    .content
                    .texts
                    .iter()
                    .filter(|t| st.sel_texts.contains(&t.id))
                    .map(|t| {
                        let mut a = t.clone();
                        a.x += offset.x;
                        a.y += offset.y;
                        (t.clone(), a)
                    })
                    .unzip();
                for s in &before_s {
                    st.node_cache.remove(&s.id);
                }
                Self::dispatch_combined(&mut st, before_s, after_s, before_i, after_i, before_t, after_t);
                drop(st);
                self.queue_draw();
                self.notify_changed();
                return;
            }
            drop(st);
            self.queue_draw();
            return;
        }

        // Shape finished: commit as ink strokes (one undo step).
        if let Some((start, end)) = st.shape_drag.take() {
            if (end - start).hypot() >= 3.0 {
                let polylines = omascratch_ink::shape_polylines(st.shape_kind, start, end);
                let t0 = now_unix_ms();
                let strokes: Vec<Stroke> = polylines
                    .into_iter()
                    .filter(|l| l.len() >= 2)
                    .map(|l| Stroke {
                        id: StrokeId::new(),
                        tool: Tool::Shape,
                        color: st.cur_color,
                        width: st.cur_width,
                        t0_ms: t0,
                        points: l
                            .into_iter()
                            .enumerate()
                            .map(|(i, (x, y))| InkPoint {
                                x,
                                y,
                                pressure: 0.6,
                                tilt_x: 0.0,
                                tilt_y: 0.0,
                                dt_ms: (i as u16).saturating_mul(4),
                            })
                            .collect(),
                    })
                    .collect();
                if !strokes.is_empty() {
                    st.session.dispatch(Command::AddStrokes(strokes));
                    drop(st);
                    self.queue_draw();
                    self.notify_changed();
                    return;
                }
            }
            drop(st);
            self.queue_draw();
            return;
        }

        // Eraser drag: commit everything the drag touched as one undoable
        // EraseStrokes command (area eraser adds split fragments back).
        if st.erasing {
            st.erasing = false;
            let ids: Vec<StrokeId> = st.live_erased.drain().collect();
            let removed: Vec<Stroke> = ids
                .iter()
                .filter_map(|id| {
                    st.session.content.stroke_index(*id).map(|i| st.session.content.strokes[i].clone())
                })
                .collect();
            let mut replacements: Vec<Stroke> = Vec::new();
            if st.eraser_kind == EraserKind::Area && !st.erase_path.is_empty() {
                for orig in &removed {
                    let fragments = omascratch_ink::erase_samples(
                        &orig.points,
                        &st.erase_path,
                        st.eraser_radius + orig.width / 2.0,
                    );
                    for pts in fragments {
                        replacements.push(Stroke {
                            id: StrokeId::new(),
                            tool: orig.tool,
                            color: orig.color,
                            width: orig.width,
                            t0_ms: orig.t0_ms,
                            points: pts,
                        });
                    }
                }
            }
            st.erase_path.clear();
            if !removed.is_empty() {
                for id in &ids {
                    st.node_cache.remove(id);
                }
                st.session.dispatch(Command::EraseStrokes { removed, replacements });
                drop(st);
                self.queue_draw();
                self.notify_changed();
            }
            return;
        }

        let Some(live) = st.live.take() else { return };
        if live.points.len() >= 2 {
            let stroke = Stroke {
                id: StrokeId::new(),
                tool: live.tool,
                color: live.color,
                width: live.width,
                t0_ms: live.t0_ms,
                points: live.points,
            };
            st.session.dispatch(Command::AddStroke(stroke));
            drop(st);
            self.queue_draw();
            self.notify_changed();
            return;
        }
        drop(st);
        self.queue_draw();
    }

    // ---- rendering ----

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let width = self.width() as f32;
        let height = self.height() as f32;
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let mut st = self.imp().state.borrow_mut();

        // Page background (M5 swaps these constants for the Omarchy theme).
        let bg = if st.inverted { st.palette.bg_inv } else { st.palette.bg };
        snapshot.append_color(&bg, &graphene::Rect::new(0.0, 0.0, width, height));

        // World transform: screen = (world - offset) * zoom.
        snapshot.save();
        snapshot.scale(st.zoom as f32, st.zoom as f32);
        snapshot.translate(&graphene::Point::new(-st.offset.x as f32, -st.offset.y as f32));

        let visible = kurbo::Rect::new(
            st.offset.x,
            st.offset.y,
            st.offset.x + width as f64 / st.zoom,
            st.offset.y + height as f64 / st.zoom,
        );

        // Rule / grid lines + margin, drawn across the visible world rect.
        if st.background.kind != BackgroundKind::None || st.background.margin {
            let rule = if st.inverted { st.palette.rule_inv } else { st.palette.rule };
            let lw = (1.0 / st.zoom) as f32;
            let pb = gsk::PathBuilder::new();
            let spacing = st.background.spacing.max(8.0);
            if st.background.kind != BackgroundKind::None {
                let first = (visible.y0 / spacing).floor() as i64;
                let last = (visible.y1 / spacing).ceil() as i64;
                // Ruled paper has a double-height header line: the first rule
                // sits at 2x spacing. Grid paper stays uniform.
                let min_k = if st.background.kind == BackgroundKind::Rules { 2 } else { i64::MIN };
                for k in first.max(min_k)..=last {
                    let y = k as f64 * spacing;
                    pb.move_to(visible.x0 as f32, y as f32);
                    pb.line_to(visible.x1 as f32, y as f32);
                }
            }
            if st.background.kind == BackgroundKind::Grid {
                let first = (visible.x0 / spacing).floor() as i64;
                let last = (visible.x1 / spacing).ceil() as i64;
                for k in first..=last {
                    let x = k as f64 * spacing;
                    pb.move_to(x as f32, visible.y0 as f32);
                    pb.line_to(x as f32, visible.y1 as f32);
                }
            }
            snapshot.append_stroke(&pb.to_path(), &gsk::Stroke::new(lw), &rule);
            if st.background.margin {
                // Indented like paper: the margin line sits in from the left.
                const MARGIN_X: f32 = 90.0;
                let margin = if st.inverted { st.palette.margin_inv } else { st.palette.margin };
                let mb = gsk::PathBuilder::new();
                mb.move_to(MARGIN_X, visible.y0 as f32);
                mb.line_to(MARGIN_X, visible.y1 as f32);
                snapshot.append_stroke(&mb.to_path(), &gsk::Stroke::new(lw * 1.5), &margin);
            }
        }

        // Images sit under all ink: pinned first, then unpinned. Selected
        // images follow a live move/resize preview.
        let images: Vec<ImageItem> = {
            let mut v = st.session.content.images.clone();
            v.sort_by_key(|i| !i.pinned);
            v
        };
        for img in &images {
            let mut r = img.rect();
            if st.sel_images.contains(&img.id) {
                if st.sel_drag.is_some() {
                    r = r + st.sel_offset;
                } else if let Some(rz) = st.sel_resize {
                    let tl = Self::scale_about(kurbo::Point::new(r.x0, r.y0), rz.anchor, rz.factor);
                    r = kurbo::Rect::new(tl.x, tl.y, tl.x + img.w * rz.factor, tl.y + img.h * rz.factor);
                }
            }
            if r.intersect(visible).is_zero_area() {
                continue;
            }
            let grect = graphene::Rect::new(r.x0 as f32, r.y0 as f32, r.width() as f32, r.height() as f32);
            match Self::texture_for(&mut st, &img.asset) {
                Some(tex) => snapshot.append_texture(&tex, &grect),
                None => {
                    // Missing asset (e.g. not synced yet): a quiet placeholder.
                    snapshot.append_color(&gdk::RGBA::new(0.5, 0.5, 0.55, 0.18), &grect);
                }
            }
        }

        // Text boxes: above images, below ink. The one being edited is drawn
        // by the overlay editor instead. Selected boxes follow move/resize.
        Self::ensure_bounds(&mut st);
        let ctx = self.pango_context();
        let editing_id = st.editing.as_ref().map(|e| e.working.id);
        let texts: Vec<TextBox> = st.session.content.texts.clone();
        for tb in &texts {
            if Some(tb.id) == editing_id || tb.rect().intersect(visible.inflate(tb.w, tb.h)).is_zero_area() {
                continue;
            }
            if !st.text_layouts.contains_key(&tb.id) {
                let tl = textmod::layout_text(&ctx, tb, Self::text_cell(&st, tb.font_size), &Self::text_resolver(&st));
                st.text_layouts.insert(tb.id, tl);
            }
            let color = resolve_color_inv(tb.color, Tool::Pen, st.inverted, &st.palette);
            let selected = st.sel_texts.contains(&tb.id);
            snapshot.save();
            if selected && st.sel_drag.is_some() {
                snapshot.translate(&graphene::Point::new(st.sel_offset.x as f32, st.sel_offset.y as f32));
            } else if let (true, Some(rz)) = (selected, st.sel_resize) {
                snapshot.translate(&graphene::Point::new(rz.anchor.x as f32, rz.anchor.y as f32));
                snapshot.scale(rz.factor as f32, rz.factor as f32);
                snapshot.translate(&graphene::Point::new(-rz.anchor.x as f32, -rz.anchor.y as f32));
            }
            if let Some(tl) = st.text_layouts.get(&tb.id) {
                textmod::draw_text(snapshot, &ctx, tl, tb.x, tb.y, tb.font_size, &color);
            }
            snapshot.restore();
        }
        // The box being edited: the overlay editor draws the text, the
        // canvas draws its list prefixes exactly as on the page.
        // Positions come from the page layout of the live text (never ask
        // the TextView for geometry mid-snapshot: that breaks its layout).
        if let (Some(sess), Some(ed)) = (st.editing.as_ref(), self.editor()) {
            let mut w = sess.working.clone();
            w.paras = ed.to_paras();
            let tl = textmod::layout_text(&ctx, &w, Self::text_cell(&st, w.font_size), &Self::text_resolver(&st));
            let color = resolve_color_inv(w.color, Tool::Pen, st.inverted, &st.palette);
            for p in &tl.paras {
                textmod::draw_prefix(snapshot, &ctx, &p.prefix, w.x, w.y + p.baseline, w.font_size, &color);
            }
        }

        // Committed strokes: cached node per stroke, culled by bounds.
        let live_simulate = st.live.as_ref().map(|l| l.simulate_pressure);
        Self::ensure_bounds(&mut st);
        let strokes: Vec<(StrokeId, Option<kurbo::Rect>)> = st
            .session
            .content
            .strokes
            .iter()
            .zip(st.bounds.iter())
            .map(|(s, b)| (s.id, *b))
            .collect();
        for (id, bounds) in strokes {
            let Some(bounds) = bounds else { continue };
            if bounds.intersect(visible).is_zero_area() {
                continue;
            }
            // Strokes the current eraser drag affects: stroke eraser hides
            // them; area eraser previews the surviving fragments live.
            if st.live_erased.contains(&id) {
                if st.eraser_kind == EraserKind::Area && st.erasing && !st.erase_path.is_empty() {
                    if let Some(i) = st.session.content.stroke_index(id) {
                        let s = &st.session.content.strokes[i];
                        let frags = omascratch_ink::erase_samples(
                            &s.points,
                            &st.erase_path,
                            st.eraser_radius + s.width / 2.0,
                        );
                        let color = resolve_color_inv(s.color, s.tool, st.inverted, &st.palette);
                        let (tool, width) = (s.tool, s.width);
                        for pts in frags {
                            if tool == Tool::Shape {
                                snapshot.append_stroke(&polyline_to_gsk(&pts), &shape_stroke(width), &color);
                            } else {
                                let path = omascratch_ink::stroke_bezpath(&pts, tool, width, false);
                                snapshot.append_fill(&bezpath_to_gsk(&path), gsk::FillRule::Winding, &color);
                            }
                        }
                    }
                }
                continue;
            }
            if !st.node_cache.contains_key(&id) {
                let stroke = st.session.content.strokes[st.session.content.stroke_index(id).unwrap()].clone();
                let color = resolve_color_inv(stroke.color, stroke.tool, st.inverted, &st.palette);
                let sub = gtk::Snapshot::new();
                if stroke.tool == Tool::Shape {
                    // Shapes: clean constant-width stroked path.
                    sub.append_stroke(
                        &polyline_to_gsk(&stroke.points),
                        &shape_stroke(stroke.width),
                        &color,
                    );
                } else {
                    let path = omascratch_ink::stroke_bezpath(
                        &stroke.points,
                        stroke.tool,
                        stroke.width,
                        false,
                    );
                    sub.append_fill(&bezpath_to_gsk(&path), gsk::FillRule::Winding, &color);
                }
                if let Some(node) = sub.to_node() {
                    st.node_cache.insert(id, node);
                }
            }
            // A selection being dragged/resized renders transformed, after this loop.
            if (st.sel_drag.is_some() || st.sel_resize.is_some()) && st.selection.contains(&id) {
                continue;
            }
            if let Some(node) = st.node_cache.get(&id) {
                snapshot.append_node(node);
            }
        }

        // Dragged selection: draw its strokes shifted by the live offset.
        if st.sel_drag.is_some() && !st.selection.is_empty() {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(st.sel_offset.x as f32, st.sel_offset.y as f32));
            let ids: Vec<StrokeId> = st.selection.iter().copied().collect();
            for id in ids {
                if let Some(node) = st.node_cache.get(&id) {
                    snapshot.append_node(node);
                }
            }
            snapshot.restore();
        }

        // Resized selection: strokes scaled about the anchor (width included).
        if let Some(rz) = st.sel_resize {
            if !st.selection.is_empty() {
                snapshot.save();
                snapshot.translate(&graphene::Point::new(rz.anchor.x as f32, rz.anchor.y as f32));
                snapshot.scale(rz.factor as f32, rz.factor as f32);
                snapshot.translate(&graphene::Point::new(-rz.anchor.x as f32, -rz.anchor.y as f32));
                let ids: Vec<StrokeId> = st.selection.iter().copied().collect();
                for id in ids {
                    if let Some(node) = st.node_cache.get(&id) {
                        snapshot.append_node(node);
                    }
                }
                snapshot.restore();
            }
        }

        // Selection bounding box (dashed, constant on-screen width) + handles.
        if (!st.selection.is_empty() || !st.sel_images.is_empty() || !st.sel_texts.is_empty()) && !st.lassoing {
            if let Some(mut b) = Self::selection_bounds(&st) {
                if st.sel_drag.is_some() {
                    b = b + st.sel_offset;
                } else if let Some(rz) = st.sel_resize {
                    let p0 = Self::scale_about(kurbo::Point::new(b.x0, b.y0), rz.anchor, rz.factor);
                    let p1 = Self::scale_about(kurbo::Point::new(b.x1, b.y1), rz.anchor, rz.factor);
                    b = kurbo::Rect::from_points(p0, p1);
                }
                let pb = gsk::PathBuilder::new();
                pb.add_rect(&graphene::Rect::new(
                    b.x0 as f32,
                    b.y0 as f32,
                    b.width() as f32,
                    b.height() as f32,
                ));
                let stroke = gsk::Stroke::new((1.5 / st.zoom) as f32);
                stroke.set_dash(&[(6.0 / st.zoom) as f32, (4.0 / st.zoom) as f32]);
                snapshot.append_stroke(&pb.to_path(), &stroke, &st.palette.accent);

                // Square corner handles, constant on-screen size.
                let hs = (8.0 / st.zoom) as f32;
                let page = if st.inverted { st.palette.bg_inv } else { st.palette.bg };
                for (cx, cy) in [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)] {
                    let outer = graphene::Rect::new(cx as f32 - hs / 2.0, cy as f32 - hs / 2.0, hs, hs);
                    snapshot.append_color(&st.palette.accent, &outer);
                    let inset = hs * 0.25;
                    let inner = graphene::Rect::new(
                        outer.x() + inset,
                        outer.y() + inset,
                        hs - 2.0 * inset,
                        hs - 2.0 * inset,
                    );
                    snapshot.append_color(&page, &inner);
                }
            }
        }

        // Rubber-band rectangle preview.
        if let Some((a, b)) = st.marquee {
            let r = kurbo::Rect::from_points(a, b);
            let g = graphene::Rect::new(r.x0 as f32, r.y0 as f32, r.width() as f32, r.height() as f32);
            let acc = st.palette.accent;
            snapshot.append_color(&gdk::RGBA::new(acc.red(), acc.green(), acc.blue(), 0.10), &g);
            let pb = gsk::PathBuilder::new();
            pb.add_rect(&g);
            let stroke = gsk::Stroke::new((1.2 / st.zoom) as f32);
            stroke.set_dash(&[(5.0 / st.zoom) as f32, (4.0 / st.zoom) as f32]);
            snapshot.append_stroke(&pb.to_path(), &stroke, &acc);
        }

        // Lasso path preview.
        if st.lassoing && st.lasso_path.len() >= 2 {
            let pb = gsk::PathBuilder::new();
            pb.move_to(st.lasso_path[0].x as f32, st.lasso_path[0].y as f32);
            for p in &st.lasso_path[1..] {
                pb.line_to(p.x as f32, p.y as f32);
            }
            let stroke = gsk::Stroke::new((1.5 / st.zoom) as f32);
            stroke.set_dash(&[(5.0 / st.zoom) as f32, (4.0 / st.zoom) as f32]);
            snapshot.append_stroke(&pb.to_path(), &stroke, &st.palette.accent);
        }

        // Shape preview while dragging.
        if let Some((start, end)) = st.shape_drag {
            if (end - start).hypot() >= 1.0 {
                let color = resolve_color_inv(st.cur_color, Tool::Shape, st.inverted, &st.palette);
                for line in omascratch_ink::shape_polylines(st.shape_kind, start, end) {
                    if line.len() < 2 {
                        continue;
                    }
                    let pb = gsk::PathBuilder::new();
                    pb.move_to(line[0].0 as f32, line[0].1 as f32);
                    for (x, y) in &line[1..] {
                        pb.line_to(*x as f32, *y as f32);
                    }
                    snapshot.append_stroke(&pb.to_path(), &shape_stroke(st.cur_width), &color);
                }
            }
        }

        // Live stroke: recomputed each frame.
        if let Some(live) = st.live.as_ref() {
            let path = omascratch_ink::stroke_path(
                &live.points,
                live.tool,
                live.width,
                live_simulate.unwrap_or(false),
                false,
            );
            if !path.is_empty() {
                let color = resolve_color_inv(live.color, live.tool, st.inverted, &st.palette);
                snapshot.append_fill(&bezpath_to_gsk(&path), gsk::FillRule::Winding, &color);
            }
        }

        // Eraser cursor ring, drawn in world space (we are inside the world
        // transform). Width is divided by zoom so it stays ~1.5px on screen.
        if st.active == ActiveTool::Eraser {
            let (px, py) = st.pointer;
            let wx = st.offset.x + px / st.zoom;
            let wy = st.offset.y + py / st.zoom;
            let ring = gsk::PathBuilder::new();
            ring.add_circle(&graphene::Point::new(wx as f32, wy as f32), st.eraser_radius as f32);
            snapshot.append_stroke(
                &ring.to_path(),
                &gsk::Stroke::new((1.5 / st.zoom) as f32),
                &gdk::RGBA::new(0.7, 0.7, 0.8, 0.8),
            );
        }

        snapshot.restore();

        // Debug overlay (widget coordinates).
        if st.debug.enabled {
            let cutoff = Instant::now() - std::time::Duration::from_secs(1);
            while st.debug.sample_times.front().is_some_and(|t| *t < cutoff) {
                st.debug.sample_times.pop_front();
            }
            let text = format!(
                "tool: {} [{}]  |  pressure {:.3} (min {:.3} max {:.3})\n\
                 tilt: ({:+.2}, {:+.2})  |  {} samples/s\n\
                 strokes: {}  |  zoom {:.0}%  |  offset ({:.0}, {:.0})\n\
                 undo: {}  redo: {}",
                if st.debug.tool_name.is_empty() { "-" } else { &st.debug.tool_name },
                if st.debug.buttons.is_empty() { "-" } else { &st.debug.buttons },
                st.debug.last_pressure,
                st.debug.min_pressure,
                st.debug.max_pressure,
                st.debug.last_tilt.0,
                st.debug.last_tilt.1,
                st.debug.sample_times.len(),
                st.session.content.strokes.len(),
                st.zoom * 100.0,
                st.offset.x,
                st.offset.y,
                st.session.can_undo(),
                st.session.can_redo(),
            );
            drop(st);
            let layout = self.create_pango_layout(Some(&text));
            let (tw, th) = layout.pixel_size();
            snapshot.append_color(
                &DEBUG_BG,
                &graphene::Rect::new(6.0, 6.0, tw as f32 + 16.0, th as f32 + 12.0),
            );
            snapshot.save();
            snapshot.translate(&graphene::Point::new(14.0, 12.0));
            snapshot.append_layout(&layout, &DEBUG_TEXT);
            snapshot.restore();
        }
    }
}
