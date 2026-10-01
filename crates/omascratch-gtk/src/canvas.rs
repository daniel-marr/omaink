//! CanvasView: the infinite-canvas ink widget.
//!
//! UI-boundary rules: this widget owns only transient state (live stroke
//! buffer, viewport, node cache). Authoritative content lives in the
//! `NoteSession` (core). Input handlers only buffer samples and queue a
//! redraw; geometry runs once per frame in `snapshot()`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::time::{Instant, SystemTime};

use gtk4 as gtk;
use gtk4::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};
use kurbo::PathEl;

use omascratch_core::{Command, InkPoint, NoteSession, SemanticColor, Stroke, StrokeId, Tool};

// M1 fixed palette (Tokyo Night-ish). Replaced by the Omarchy theme adapter in M5.
const BG: gdk::RGBA = gdk::RGBA::new(0.102, 0.106, 0.149, 1.0);
const INK: gdk::RGBA = gdk::RGBA::new(0.753, 0.792, 0.961, 1.0);
const DEBUG_TEXT: gdk::RGBA = gdk::RGBA::new(1.0, 0.62, 0.39, 1.0);
const DEBUG_BG: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.55);

const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 16.0;

struct LiveStroke {
    tool: Tool,
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
    sample_times: VecDeque<Instant>,
}

pub struct State {
    session: NoteSession,
    /// World coordinate at the widget's top-left corner.
    offset: kurbo::Vec2,
    zoom: f64,
    live: Option<LiveStroke>,
    /// Committed-stroke render nodes, rebuilt lazily per stroke.
    node_cache: HashMap<StrokeId, gsk::RenderNode>,
    tool: Tool,
    stroke_width: f64,
    pan_anchor: Option<kurbo::Vec2>,
    pointer: (f64, f64),
    debug: DebugStats,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: NoteSession::default(),
            offset: kurbo::Vec2::ZERO,
            zoom: 1.0,
            live: None,
            node_cache: HashMap::new(),
            tool: Tool::Pen,
            stroke_width: 3.5,
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
            self.obj().draw(snapshot);
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

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl CanvasView {
    // ---- public API used by the shell ----

    pub fn undo(&self) {
        let changed = self.imp().state.borrow_mut().session.undo();
        if changed {
            self.queue_draw();
        }
    }

    pub fn redo(&self) {
        let changed = self.imp().state.borrow_mut().session.redo();
        if changed {
            self.queue_draw();
        }
    }

    pub fn toggle_debug_overlay(&self) {
        let mut st = self.imp().state.borrow_mut();
        st.debug.enabled = !st.debug.enabled;
        drop(st);
        self.queue_draw();
    }

    pub fn reset_view(&self) {
        let mut st = self.imp().state.borrow_mut();
        st.offset = kurbo::Vec2::ZERO;
        st.zoom = 1.0;
        drop(st);
        self.queue_draw();
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
            view.mouse_begin(x, y);
        });
        let weak = self.downgrade();
        draw.connect_drag_update(move |g, dx, dy| {
            let Some(view) = weak.upgrade() else { return };
            if g.current_event().and_then(|ev| ev.device_tool()).is_some() {
                return;
            }
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
        pan.connect_drag_begin(move |_, _, _| {
            if let Some(view) = weak.upgrade() {
                let mut st = view.imp().state.borrow_mut();
                st.pan_anchor = Some(st.offset);
            }
        });
        let weak = self.downgrade();
        pan.connect_drag_update(move |_, dx, dy| {
            let Some(view) = weak.upgrade() else { return };
            let mut st = view.imp().state.borrow_mut();
            if let Some(anchor) = st.pan_anchor {
                let zoom = st.zoom;
                st.offset = anchor - kurbo::Vec2::new(dx, dy) / zoom;
                drop(st);
                view.queue_draw();
            }
        });
        self.add_controller(pan);

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
        drop(st);
        self.queue_draw();
    }

    // ---- stroke input ----

    fn widget_to_world(st: &State, x: f64, y: f64) -> (f64, f64) {
        (st.offset.x + x / st.zoom, st.offset.y + y / st.zoom)
    }

    fn stylus_begin(&self, g: &gtk::GestureStylus, x: f64, y: f64) {
        self.grab_focus();
        let mut st = self.imp().state.borrow_mut();
        let tool = st.tool;
        let width = st.stroke_width;
        if let Some(t) = g.device_tool() {
            st.debug.tool_name = format!("{:?}", t.tool_type());
        }
        st.live = Some(LiveStroke {
            tool,
            width,
            points: Vec::with_capacity(256),
            started: Instant::now(),
            t0_ms: now_unix_ms(),
            simulate_pressure: false,
        });
        drop(st);
        self.push_stylus_point(g, x, y);
    }

    fn stylus_motion(&self, g: &gtk::GestureStylus, x: f64, y: f64) {
        if self.imp().state.borrow().live.is_none() {
            return;
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

    fn push_point_locked(st: &mut State, x: f64, y: f64, pressure: f64, tilt_x: f64, tilt_y: f64) {
        let (wx, wy) = Self::widget_to_world(st, x, y);
        // A zero pressure sample on a device that reports pressure is a
        // proximity artifact; clamp into a drawable range instead of a gap.
        let p = if pressure <= 0.0 { 0.05 } else { pressure.min(1.0) };
        st.debug.last_pressure = pressure;
        st.debug.min_pressure = st.debug.min_pressure.min(pressure);
        st.debug.max_pressure = st.debug.max_pressure.max(pressure);
        st.debug.last_tilt = (tilt_x, tilt_y);
        st.debug.sample_times.push_back(Instant::now());
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
        self.grab_focus();
        let mut st = self.imp().state.borrow_mut();
        let tool = st.tool;
        let width = st.stroke_width;
        st.debug.tool_name = "Mouse".into();
        st.live = Some(LiveStroke {
            tool,
            width,
            points: Vec::with_capacity(128),
            started: Instant::now(),
            t0_ms: now_unix_ms(),
            simulate_pressure: true,
        });
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
        let Some(live) = st.live.take() else { return };
        if live.points.len() >= 2 {
            let stroke = Stroke {
                id: StrokeId::new(),
                tool: live.tool,
                color: SemanticColor::Foreground,
                width: live.width,
                t0_ms: live.t0_ms,
                points: live.points,
            };
            st.session.dispatch(Command::AddStroke(stroke));
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

        // Background (M5 replaces this with the themed page + rule lines).
        snapshot.append_color(&BG, &graphene::Rect::new(0.0, 0.0, width, height));

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

        // Committed strokes: cached node per stroke, culled by bounds.
        let live_simulate = st.live.as_ref().map(|l| l.simulate_pressure);
        let strokes: Vec<(StrokeId, Option<kurbo::Rect>)> =
            st.session.content.strokes.iter().map(|s| (s.id, s.bounds())).collect();
        let present: std::collections::HashSet<StrokeId> = strokes.iter().map(|(id, _)| *id).collect();
        st.node_cache.retain(|id, _| present.contains(id));
        for (id, bounds) in strokes {
            let Some(bounds) = bounds else { continue };
            if bounds.intersect(visible).is_zero_area() {
                continue;
            }
            if !st.node_cache.contains_key(&id) {
                let stroke = st.session.content.strokes[st.session.content.stroke_index(id).unwrap()].clone();
                let path = omascratch_ink::stroke_bezpath(
                    &stroke.points,
                    stroke.tool,
                    stroke.width,
                    false,
                );
                let sub = gtk::Snapshot::new();
                sub.append_fill(&bezpath_to_gsk(&path), gsk::FillRule::Winding, &INK);
                if let Some(node) = sub.to_node() {
                    st.node_cache.insert(id, node);
                }
            }
            if let Some(node) = st.node_cache.get(&id) {
                snapshot.append_node(node);
            }
        }

        // Live stroke: recomputed each frame.
        if let Some(live) = st.live.as_ref() {
            let outline = omascratch_ink::outline_points(
                &live.points,
                live.tool,
                live.width,
                live_simulate.unwrap_or(false),
                false,
            );
            let path = omascratch_ink::outline_to_bezpath(&outline);
            if !path.is_empty() {
                snapshot.append_fill(&bezpath_to_gsk(&path), gsk::FillRule::Winding, &INK);
            }
        }

        snapshot.restore();

        // Debug overlay (widget coordinates).
        if st.debug.enabled {
            let cutoff = Instant::now() - std::time::Duration::from_secs(1);
            while st.debug.sample_times.front().is_some_and(|t| *t < cutoff) {
                st.debug.sample_times.pop_front();
            }
            let text = format!(
                "tool: {}  |  pressure {:.3} (min {:.3} max {:.3})\n\
                 tilt: ({:+.2}, {:+.2})  |  {} samples/s\n\
                 strokes: {}  |  zoom {:.0}%  |  offset ({:.0}, {:.0})\n\
                 undo: {}  redo: {}",
                if st.debug.tool_name.is_empty() { "-" } else { &st.debug.tool_name },
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
