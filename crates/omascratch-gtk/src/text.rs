//! Text boxes in the GTK shell: Pango layout + rendering on the canvas, and
//! the in-place rich-text editor (a GtkTextView overlaid on the canvas).
//!
//! Model ↔ editor mapping: list markers are literal, non-editable text at the
//! start of a line ("• ", "1. ", "☐ ", "☑ ") so the editor needs no custom
//! widgets; styles are text tags (bold, italic, underline, highlight).

use std::cell::{Cell, RefCell};

use gtk4 as gtk;
use gtk4::{gdk, glib, graphene, gsk, pango, prelude::*};

use omascratch_core::{ParaKind, Paragraph, Span, TextBox};

pub const DEFAULT_FONT_SIZE: f64 = 18.0;
pub const DEFAULT_WIDTH: f64 = 420.0;
/// Hanging indent for list paragraphs, in font sizes.
const LIST_INDENT_EM: f64 = 1.6;
/// Extra space after each paragraph, in font sizes.
const PARA_GAP_EM: f64 = 0.25;

pub const MARK_BULLET: &str = "• ";
pub const MARK_CHECK: &str = "☐ ";
pub const MARK_CHECKED: &str = "☑ ";

pub fn highlight_rgba() -> gdk::RGBA {
    gdk::RGBA::new(0.98, 0.86, 0.33, 0.55)
}

// ---------------------------------------------------------------- layout --

pub enum Prefix {
    None,
    Bullet,
    Number(u32),
    Check(bool),
}

pub struct ParaLayout {
    /// Paragraph top, relative to the box origin.
    pub y: f64,
    pub indent: f64,
    pub prefix: Prefix,
    pub layout: pango::Layout,
    /// First-line height (for vertically centering markers).
    pub line_h: f64,
}

pub struct TextLayout {
    pub paras: Vec<ParaLayout>,
    pub height: f64,
}

impl TextLayout {
    /// Checkbox hit rects relative to the box origin: (paragraph index, rect).
    pub fn check_rects(&self, font_size: f64) -> Vec<(usize, kurbo::Rect)> {
        let s = font_size * 0.8;
        self.paras
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p.prefix {
                Prefix::Check(_) => {
                    let y = p.y + (p.line_h - s) / 2.0;
                    Some((i, kurbo::Rect::new(0.0, y, s, y + s)))
                }
                _ => None,
            })
            .collect()
    }
}

fn font_for(ctx: &pango::Context, size: f64) -> pango::FontDescription {
    let mut fd = ctx.font_description().unwrap_or_default();
    fd.set_absolute_size(size * pango::SCALE as f64);
    fd
}

/// Lay out a text box in world units (font size is absolute world units).
pub fn layout_text(ctx: &pango::Context, tb: &TextBox) -> TextLayout {
    let fd = font_for(ctx, tb.font_size);
    let mut paras = Vec::with_capacity(tb.paras.len());
    let mut y = 0.0;
    let mut number = 0u32;
    for p in &tb.paras {
        let (prefix, indent) = match p.kind {
            ParaKind::Body => {
                number = 0;
                (Prefix::None, 0.0)
            }
            ParaKind::Bullet => {
                number = 0;
                (Prefix::Bullet, tb.font_size * LIST_INDENT_EM)
            }
            ParaKind::Number => {
                number += 1;
                (Prefix::Number(number), tb.font_size * LIST_INDENT_EM)
            }
            ParaKind::Check { checked } => {
                number = 0;
                (Prefix::Check(checked), tb.font_size * LIST_INDENT_EM)
            }
        };
        let layout = pango::Layout::new(ctx);
        layout.set_font_description(Some(&fd));
        layout.set_wrap(pango::WrapMode::WordChar);
        layout.set_width(((tb.w - indent).max(tb.font_size) * pango::SCALE as f64) as i32);

        let mut text = String::new();
        let attrs = pango::AttrList::new();
        for span in &p.spans {
            let start = text.len() as u32;
            text.push_str(&span.text);
            let end = text.len() as u32;
            if start == end {
                continue;
            }
            if span.bold {
                let mut a = pango::AttrInt::new_weight(pango::Weight::Bold);
                a.set_start_index(start);
                a.set_end_index(end);
                attrs.insert(a);
            }
            if span.italic {
                let mut a = pango::AttrInt::new_style(pango::Style::Italic);
                a.set_start_index(start);
                a.set_end_index(end);
                attrs.insert(a);
            }
            if span.underline {
                let mut a = pango::AttrInt::new_underline(pango::Underline::Single);
                a.set_start_index(start);
                a.set_end_index(end);
                attrs.insert(a);
            }
            if span.highlight {
                let h = highlight_rgba();
                let mut a = pango::AttrColor::new_background(
                    (h.red() * 65535.0) as u16,
                    (h.green() * 65535.0) as u16,
                    (h.blue() * 65535.0) as u16,
                );
                a.set_start_index(start);
                a.set_end_index(end);
                attrs.insert(a);
                let mut al = pango::AttrInt::new_background_alpha((h.alpha() * 65535.0) as u16);
                al.set_start_index(start);
                al.set_end_index(end);
                attrs.insert(al);
            }
        }
        layout.set_text(&text);
        layout.set_attributes(Some(&attrs));

        let (_, lh) = layout.size();
        let height = (lh as f64 / pango::SCALE as f64).max(tb.font_size * 1.2);
        let line_h = layout
            .line_readonly(0)
            .map(|l| {
                let (_, logical) = l.extents();
                logical.height() as f64 / pango::SCALE as f64
            })
            .unwrap_or(tb.font_size * 1.2)
            .max(tb.font_size);
        paras.push(ParaLayout { y, indent, prefix, layout, line_h });
        y += height + tb.font_size * PARA_GAP_EM;
    }
    let height = (y - tb.font_size * PARA_GAP_EM).max(tb.font_size * 1.2);
    TextLayout { paras, height }
}

/// Draw a laid-out text box at (x, y) in the current (world) transform.
pub fn draw_text(
    snapshot: &gtk::Snapshot,
    ctx: &pango::Context,
    tl: &TextLayout,
    x: f64,
    y: f64,
    font_size: f64,
    color: &gdk::RGBA,
) {
    for p in &tl.paras {
        let py = y + p.y;
        match p.prefix {
            Prefix::None => {}
            Prefix::Bullet => {
                let r = font_size * 0.16;
                let cy = py + p.line_h / 2.0;
                let pb = gsk::PathBuilder::new();
                pb.add_circle(&graphene::Point::new((x + font_size * 0.55) as f32, cy as f32), r as f32);
                snapshot.append_fill(&pb.to_path(), gsk::FillRule::Winding, color);
            }
            Prefix::Number(n) => {
                let l = pango::Layout::new(ctx);
                l.set_font_description(Some(&font_for(ctx, font_size)));
                l.set_text(&format!("{n}."));
                snapshot.save();
                snapshot.translate(&graphene::Point::new(x as f32, py as f32));
                snapshot.append_layout(&l, color);
                snapshot.restore();
            }
            Prefix::Check(checked) => {
                let s = font_size * 0.8;
                let by = py + (p.line_h - s) / 2.0;
                let pb = gsk::PathBuilder::new();
                pb.add_rect(&graphene::Rect::new(x as f32, by as f32, s as f32, s as f32));
                let stroke = gsk::Stroke::new((font_size * 0.08).max(1.0) as f32);
                snapshot.append_stroke(&pb.to_path(), &stroke, color);
                if checked {
                    let ck = gsk::PathBuilder::new();
                    ck.move_to((x + s * 0.2) as f32, (by + s * 0.52) as f32);
                    ck.line_to((x + s * 0.42) as f32, (by + s * 0.75) as f32);
                    ck.line_to((x + s * 0.82) as f32, (by + s * 0.25) as f32);
                    let st = gsk::Stroke::new((font_size * 0.1).max(1.2) as f32);
                    st.set_line_cap(gsk::LineCap::Round);
                    st.set_line_join(gsk::LineJoin::Round);
                    snapshot.append_stroke(&ck.to_path(), &st, color);
                }
            }
        }
        snapshot.save();
        snapshot.translate(&graphene::Point::new((x + p.indent) as f32, py as f32));
        snapshot.append_layout(&p.layout, color);
        snapshot.restore();
    }
}

// ---------------------------------------------------------------- editor --

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    Bold,
    Italic,
    Underline,
    Highlight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Bullet,
    Number,
    Check,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StyleState {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub highlight: bool,
}

pub struct TextEditor {
    pub view: gtk::TextView,
    pub buffer: gtk::TextBuffer,
    bold: gtk::TextTag,
    italic: gtk::TextTag,
    underline: gtk::TextTag,
    highlight: gtk::TextTag,
    marker: gtk::TextTag,
    list: gtk::TextTag,
    /// Explicit style for the next typed text (toggled with no selection).
    pending: Cell<Option<StyleState>>,
    /// True while we mutate the buffer ourselves (suppress style inheritance).
    internal: Cell<bool>,
    css: gtk::CssProvider,
    on_state: RefCell<Option<Box<dyn Fn(StyleState)>>>,
}

impl TextEditor {
    pub fn new() -> std::rc::Rc<TextEditor> {
        let buffer = gtk::TextBuffer::new(None);
        buffer.set_enable_undo(true);
        let view = gtk::TextView::with_buffer(&buffer);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_accepts_tab(false);
        view.add_css_class("oma-text-editor");
        view.set_halign(gtk::Align::Start);
        view.set_valign(gtk::Align::Start);
        view.set_visible(false);

        let table = buffer.tag_table();
        let bold = gtk::TextTag::builder().name("bold").weight(700).build();
        let italic = gtk::TextTag::builder().name("italic").style(pango::Style::Italic).build();
        let underline = gtk::TextTag::builder().name("underline").underline(pango::Underline::Single).build();
        let highlight = gtk::TextTag::builder().name("highlight").background_rgba(&highlight_rgba()).build();
        let marker = gtk::TextTag::builder().name("marker").editable(false).build();
        let list = gtk::TextTag::builder().name("list").build();
        for t in [&bold, &italic, &underline, &highlight, &marker, &list] {
            table.add(t);
        }

        let css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }

        let ed = std::rc::Rc::new(TextEditor {
            view,
            buffer,
            bold,
            italic,
            underline,
            highlight,
            marker,
            list,
            pending: Cell::new(None),
            internal: Cell::new(false),
            css,
            on_state: RefCell::new(None),
        });

        // Newly typed text takes the pending style, else the style of the
        // character before it (word-processor behavior).
        {
            let weak = std::rc::Rc::downgrade(&ed);
            ed.buffer.connect_local("insert-text", true, move |args| {
                let ed = weak.upgrade()?;
                if ed.internal.get() {
                    return None;
                }
                let end = args[1].get::<gtk::TextIter>().ok()?;
                let text = args[2].get::<String>().ok()?;
                let n = text.chars().count() as i32;
                let mut start = end;
                start.backward_chars(n);
                let style = ed.pending.get().unwrap_or_else(|| {
                    let mut before = start;
                    if before.backward_char() && !before.has_tag(&ed.marker) {
                        ed.style_at(&before)
                    } else {
                        StyleState::default()
                    }
                });
                ed.apply_style(&start, &end, style);
                None
            });
        }
        // Report the style at the cursor (toolbar B/I/U/H state).
        {
            let weak = std::rc::Rc::downgrade(&ed);
            ed.buffer.connect_mark_set(move |_, _, mark| {
                if let Some(ed) = weak.upgrade() {
                    if mark.name().as_deref() == Some("insert") {
                        ed.pending.set(None);
                        ed.emit_state();
                    }
                }
            });
        }
        // List-aware Return and Backspace.
        {
            let weak = std::rc::Rc::downgrade(&ed);
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(move |_, key, _, mods| {
                let Some(ed) = weak.upgrade() else { return glib::Propagation::Proceed };
                let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
                match key {
                    gdk::Key::Return | gdk::Key::KP_Enter if !shift => {
                        if ed.list_return() {
                            return glib::Propagation::Stop;
                        }
                    }
                    gdk::Key::BackSpace if !ctrl => {
                        if ed.list_backspace() {
                            return glib::Propagation::Stop;
                        }
                    }
                    gdk::Key::b if ctrl => {
                        ed.toggle(Fmt::Bold);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::i if ctrl => {
                        ed.toggle(Fmt::Italic);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::u if ctrl => {
                        ed.toggle(Fmt::Underline);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::H | gdk::Key::h if ctrl && shift => {
                        ed.toggle(Fmt::Highlight);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::_1 if ctrl => {
                        ed.cycle_check();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            });
            ed.view.add_controller(keys);
        }
        ed
    }

    pub fn set_on_state(&self, f: impl Fn(StyleState) + 'static) {
        *self.on_state.borrow_mut() = Some(Box::new(f));
    }

    fn emit_state(&self) {
        let st = self.current_style();
        if let Some(cb) = self.on_state.borrow().as_ref() {
            cb(st);
        }
    }

    /// Style the toolbar should show: pending override, else the selection
    /// start / character before the cursor.
    pub fn current_style(&self) -> StyleState {
        if let Some(p) = self.pending.get() {
            return p;
        }
        if let Some((s, _)) = self.buffer.selection_bounds() {
            return self.style_at(&s);
        }
        let mut it = self.buffer.iter_at_offset(self.buffer.cursor_position());
        if it.backward_char() && !it.has_tag(&self.marker) {
            self.style_at(&it)
        } else {
            StyleState::default()
        }
    }

    fn style_at(&self, it: &gtk::TextIter) -> StyleState {
        StyleState {
            bold: it.has_tag(&self.bold),
            italic: it.has_tag(&self.italic),
            underline: it.has_tag(&self.underline),
            highlight: it.has_tag(&self.highlight),
        }
    }

    fn apply_style(&self, start: &gtk::TextIter, end: &gtk::TextIter, st: StyleState) {
        for (tag, on) in [
            (&self.bold, st.bold),
            (&self.italic, st.italic),
            (&self.underline, st.underline),
            (&self.highlight, st.highlight),
        ] {
            if on {
                self.buffer.apply_tag(tag, start, end);
            } else {
                self.buffer.remove_tag(tag, start, end);
            }
        }
    }

    fn tag_for(&self, f: Fmt) -> &gtk::TextTag {
        match f {
            Fmt::Bold => &self.bold,
            Fmt::Italic => &self.italic,
            Fmt::Underline => &self.underline,
            Fmt::Highlight => &self.highlight,
        }
    }

    /// Toggle a style on the selection, or for the next typed text.
    pub fn toggle(&self, f: Fmt) {
        if let Some((s, e)) = self.buffer.selection_bounds() {
            let tag = self.tag_for(f);
            // On if any selected char lacks it, else off.
            let mut it = s;
            let mut all = true;
            while it < e {
                if !it.has_tag(&self.marker) && !it.has_tag(tag) {
                    all = false;
                    break;
                }
                it.forward_char();
            }
            if all {
                self.buffer.remove_tag(tag, &s, &e);
            } else {
                self.buffer.apply_tag(tag, &s, &e);
            }
        } else {
            let mut st = self.current_style();
            match f {
                Fmt::Bold => st.bold = !st.bold,
                Fmt::Italic => st.italic = !st.italic,
                Fmt::Underline => st.underline = !st.underline,
                Fmt::Highlight => st.highlight = !st.highlight,
            }
            self.pending.set(Some(st));
        }
        self.emit_state();
    }

    // -- model <-> buffer --

    pub fn load(&self, tb: &TextBox) {
        self.internal.set(true);
        self.buffer.set_text("");
        let mut number = 0u32;
        for (i, p) in tb.paras.iter().enumerate() {
            if i > 0 {
                let mut end = self.buffer.end_iter();
                self.buffer.insert(&mut end, "\n");
            }
            let marker = match p.kind {
                ParaKind::Body => {
                    number = 0;
                    None
                }
                ParaKind::Bullet => {
                    number = 0;
                    Some(MARK_BULLET.to_string())
                }
                ParaKind::Number => {
                    number += 1;
                    Some(format!("{number}. "))
                }
                ParaKind::Check { checked } => {
                    number = 0;
                    Some(if checked { MARK_CHECKED } else { MARK_CHECK }.to_string())
                }
            };
            if let Some(m) = marker {
                let mut end = self.buffer.end_iter();
                self.buffer.insert_with_tags(&mut end, &m, &[&self.marker]);
            }
            for span in &p.spans {
                let mut tags: Vec<&gtk::TextTag> = Vec::new();
                if span.bold {
                    tags.push(&self.bold);
                }
                if span.italic {
                    tags.push(&self.italic);
                }
                if span.underline {
                    tags.push(&self.underline);
                }
                if span.highlight {
                    tags.push(&self.highlight);
                }
                let mut end = self.buffer.end_iter();
                self.buffer.insert_with_tags(&mut end, &span.text, &tags);
            }
        }
        self.retag_lists();
        self.internal.set(false);
        self.pending.set(None);
        let start = self.buffer.start_iter();
        self.buffer.place_cursor(&start);
    }

    fn line_bounds(&self, line: i32) -> Option<(gtk::TextIter, gtk::TextIter)> {
        let start = self.buffer.iter_at_line(line)?;
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        Some((start, end))
    }

    /// (kind, marker end iter) of a line's list marker, if any.
    fn line_marker(&self, line: i32) -> Option<(ParaKind, gtk::TextIter)> {
        let (start, end) = self.line_bounds(line)?;
        if !start.has_tag(&self.marker) {
            return None;
        }
        let mut mend = start;
        while mend < end && mend.has_tag(&self.marker) {
            mend.forward_char();
        }
        let text = self.buffer.text(&start, &mend, true).to_string();
        let kind = if text == MARK_BULLET {
            ParaKind::Bullet
        } else if text == MARK_CHECK {
            ParaKind::Check { checked: false }
        } else if text == MARK_CHECKED {
            ParaKind::Check { checked: true }
        } else {
            ParaKind::Number
        };
        Some((kind, mend))
    }

    pub fn to_paras(&self) -> Vec<Paragraph> {
        let mut out = Vec::new();
        for line in 0..self.buffer.line_count() {
            let Some((start, end)) = self.line_bounds(line) else { continue };
            let (kind, mut it) = match self.line_marker(line) {
                Some((k, m)) => (k, m),
                None => (ParaKind::Body, start),
            };
            let mut spans: Vec<Span> = Vec::new();
            while it < end {
                let st = self.style_at(&it);
                let ch = it.char();
                match spans.last_mut() {
                    Some(last)
                        if last.bold == st.bold
                            && last.italic == st.italic
                            && last.underline == st.underline
                            && last.highlight == st.highlight =>
                    {
                        last.text.push(ch)
                    }
                    _ => spans.push(Span {
                        text: ch.to_string(),
                        bold: st.bold,
                        italic: st.italic,
                        underline: st.underline,
                        highlight: st.highlight,
                    }),
                }
                it.forward_char();
            }
            out.push(Paragraph { kind, spans });
        }
        if out.is_empty() {
            out.push(Paragraph { kind: ParaKind::Body, spans: vec![] });
        }
        out
    }

    // -- lists --

    fn marker_text(kind: ParaKind, n: u32) -> String {
        match kind {
            ParaKind::Body => String::new(),
            ParaKind::Bullet => MARK_BULLET.to_string(),
            ParaKind::Number => format!("{n}. "),
            ParaKind::Check { checked } => if checked { MARK_CHECKED } else { MARK_CHECK }.to_string(),
        }
    }

    /// Replace (or remove, for Body) a line's marker.
    fn set_line_kind(&self, line: i32, kind: ParaKind) {
        self.internal.set(true);
        if let Some((_, mend)) = self.line_marker(line) {
            let mut s = self.buffer.iter_at_line(line).unwrap();
            let mut e = mend;
            self.buffer.delete(&mut s, &mut e);
        }
        let text = Self::marker_text(kind, 1);
        if !text.is_empty() {
            let mut s = self.buffer.iter_at_line(line).unwrap();
            self.buffer.insert_with_tags(&mut s, &text, &[&self.marker]);
        }
        self.internal.set(false);
    }

    /// Renumber numbered runs and refresh the hanging-indent tag.
    fn retag_lists(&self) {
        self.internal.set(true);
        let mut n = 0u32;
        for line in 0..self.buffer.line_count() {
            match self.line_marker(line) {
                Some((ParaKind::Number, mend)) => {
                    n += 1;
                    let want = format!("{n}. ");
                    let s = self.buffer.iter_at_line(line).unwrap();
                    if self.buffer.text(&s, &mend, true) != want {
                        let mut s2 = s;
                        let mut e2 = mend;
                        self.buffer.delete(&mut s2, &mut e2);
                        let mut s3 = self.buffer.iter_at_line(line).unwrap();
                        self.buffer.insert_with_tags(&mut s3, &want, &[&self.marker]);
                    }
                }
                _ => n = 0,
            }
        }
        let (s, e) = self.buffer.bounds();
        self.buffer.remove_tag(&self.list, &s, &e);
        for line in 0..self.buffer.line_count() {
            if self.line_marker(line).is_some() {
                if let Some((ls, le)) = self.line_bounds(line) {
                    self.buffer.apply_tag(&self.list, &ls, &le);
                }
            }
        }
        self.internal.set(false);
    }

    fn cursor_line(&self) -> i32 {
        self.buffer.iter_at_offset(self.buffer.cursor_position()).line()
    }

    fn selected_lines(&self) -> (i32, i32) {
        match self.buffer.selection_bounds() {
            Some((s, e)) => (s.line(), e.line()),
            None => {
                let l = self.cursor_line();
                (l, l)
            }
        }
    }

    /// Toolbar list buttons: apply `kind` to the selected lines, or remove it
    /// when every selected line already has it.
    pub fn toggle_list(&self, kind: ListKind) {
        let (a, b) = self.selected_lines();
        let target = match kind {
            ListKind::Bullet => ParaKind::Bullet,
            ListKind::Number => ParaKind::Number,
            ListKind::Check => ParaKind::Check { checked: false },
        };
        let same = |k: ParaKind| match (k, target) {
            (ParaKind::Check { .. }, ParaKind::Check { .. }) => true,
            (x, y) => x == y,
        };
        let all = (a..=b).all(|l| self.line_marker(l).is_some_and(|(k, _)| same(k)));
        for l in a..=b {
            self.set_line_kind(l, if all { ParaKind::Body } else { target });
        }
        self.retag_lists();
    }

    /// Ctrl+1 (OneNote to-do): none → unchecked → checked → unchecked …
    fn cycle_check(&self) {
        let l = self.cursor_line();
        let next = match self.line_marker(l) {
            Some((ParaKind::Check { checked: false }, _)) => ParaKind::Check { checked: true },
            _ => ParaKind::Check { checked: false },
        };
        self.set_line_kind(l, next);
        self.retag_lists();
    }

    /// Return inside a list: continue it, or end it on an empty item.
    fn list_return(&self) -> bool {
        if self.buffer.selection_bounds().is_some() {
            return false;
        }
        let line = self.cursor_line();
        let Some((kind, mend)) = self.line_marker(line) else { return false };
        let (_, lend) = self.line_bounds(line).unwrap();
        if self.buffer.text(&mend, &lend, false).trim().is_empty() {
            self.set_line_kind(line, ParaKind::Body);
            self.retag_lists();
            return true;
        }
        let next = match kind {
            ParaKind::Check { .. } => ParaKind::Check { checked: false },
            k => k,
        };
        self.internal.set(true);
        let mut cur = self.buffer.iter_at_offset(self.buffer.cursor_position());
        self.buffer.insert(&mut cur, "\n");
        let text = Self::marker_text(next, 1);
        self.buffer.insert_with_tags(&mut cur, &text, &[&self.marker]);
        self.buffer.place_cursor(&cur);
        self.internal.set(false);
        self.retag_lists();
        true
    }

    /// Backspace right after a marker removes the marker (not the text).
    fn list_backspace(&self) -> bool {
        if self.buffer.selection_bounds().is_some() {
            return false;
        }
        let line = self.cursor_line();
        let Some((_, mend)) = self.line_marker(line) else { return false };
        let cur = self.buffer.iter_at_offset(self.buffer.cursor_position());
        if cur.offset() != mend.offset() {
            return false;
        }
        self.set_line_kind(line, ParaKind::Body);
        self.retag_lists();
        true
    }

    // -- geometry / styling --

    /// Update the editor CSS for the current zoom and colors.
    pub fn restyle(&self, font_px: f64, color: &gdk::RGBA, accent: &gdk::RGBA) {
        let c = |v: f32| (v * 255.0).round() as u8;
        let fg = format!("#{:02x}{:02x}{:02x}", c(color.red()), c(color.green()), c(color.blue()));
        let ac = format!("rgba({},{},{},0.30)", c(accent.red()), c(accent.green()), c(accent.blue()));
        let border = format!("rgba({},{},{},0.55)", c(accent.red()), c(accent.green()), c(accent.blue()));
        self.css.load_from_string(&format!(
            "textview.oma-text-editor, textview.oma-text-editor text {{ background: transparent; color: {fg}; caret-color: {fg}; font-size: {font_px:.2}px; }}
             textview.oma-text-editor {{ outline: 1px dashed {border}; outline-offset: 2px; }}
             textview.oma-text-editor text selection {{ background-color: {ac}; color: {fg}; }}"
        ));
        let indent = (font_px * LIST_INDENT_EM) as i32;
        self.list.set_left_margin(indent);
        self.list.set_indent(-indent);
        self.view.set_pixels_below_lines((font_px * PARA_GAP_EM) as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omascratch_core::{SemanticColor, TextId};

    fn tb(paras: Vec<Paragraph>) -> TextBox {
        TextBox {
            id: TextId::new(),
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 20.0,
            font_size: 18.0,
            color: SemanticColor::Foreground,
            paras,
        }
    }

    fn span(t: &str, bold: bool, highlight: bool) -> Span {
        Span { text: t.into(), bold, highlight, ..Default::default() }
    }

    #[test]
    fn editor_round_trips_styles_and_lists() {
        gtk::test_synced(|| {
            if gtk::init().is_err() {
                eprintln!("no display; skipping");
                return;
            }
            let ed = TextEditor::new();
            let original = tb(vec![
                Paragraph { kind: ParaKind::Body, spans: vec![span("Plain ", false, false), span("bold", true, false)] },
                Paragraph { kind: ParaKind::Number, spans: vec![span("one", false, false)] },
                Paragraph { kind: ParaKind::Number, spans: vec![span("two", false, true)] },
                Paragraph { kind: ParaKind::Check { checked: true }, spans: vec![span("done", false, false)] },
                Paragraph { kind: ParaKind::Bullet, spans: vec![span("dot", false, false)] },
            ]);
            ed.load(&original);
            assert_eq!(ed.to_paras(), original.paras, "load -> to_paras is lossless");
            // Numbered markers are renumbered literally in the buffer.
            let (s, e) = ed.buffer.bounds();
            let text = ed.buffer.text(&s, &e, true).to_string();
            assert!(text.contains("1. one") && text.contains("2. two"), "{text}");
        });
    }

    #[test]
    fn list_toggle_return_and_exit() {
        gtk::test_synced(|| {
            if gtk::init().is_err() {
                return;
            }
            let ed = TextEditor::new();
            ed.load(&tb(vec![Paragraph { kind: ParaKind::Body, spans: vec![span("first", false, false)] }]));
            // Cursor at end of line 0, make it a numbered list.
            let end = ed.buffer.end_iter();
            ed.buffer.place_cursor(&end);
            ed.toggle_list(ListKind::Number);
            assert_eq!(ed.to_paras()[0].kind, ParaKind::Number);
            // Return continues the list…
            let end = ed.buffer.end_iter();
            ed.buffer.place_cursor(&end);
            assert!(ed.list_return());
            let mut end = ed.buffer.end_iter();
            ed.buffer.insert(&mut end, "second");
            let paras = ed.to_paras();
            assert_eq!(paras.len(), 2);
            assert_eq!(paras[1].kind, ParaKind::Number);
            assert_eq!(paras[1].plain_text(), "second");
            // …and Return on an empty item ends it.
            let end = ed.buffer.end_iter();
            ed.buffer.place_cursor(&end);
            assert!(ed.list_return());
            let end = ed.buffer.end_iter();
            ed.buffer.place_cursor(&end);
            assert!(ed.list_return(), "empty list item: return converts it to body");
            let paras = ed.to_paras();
            assert_eq!(paras.last().unwrap().kind, ParaKind::Body);
            // Toggling the same list kind again removes it.
            let s = ed.buffer.start_iter();
            ed.buffer.place_cursor(&s);
            ed.toggle_list(ListKind::Number);
            assert_eq!(ed.to_paras()[0].kind, ParaKind::Body);
        });
    }

    #[test]
    fn typed_text_inherits_or_takes_pending_style() {
        gtk::test_synced(|| {
            if gtk::init().is_err() {
                return;
            }
            let ed = TextEditor::new();
            ed.load(&tb(vec![Paragraph { kind: ParaKind::Body, spans: vec![span("B", true, false)] }]));
            let end = ed.buffer.end_iter();
            ed.buffer.place_cursor(&end);
            // Typing after bold text continues bold.
            let mut it = ed.buffer.end_iter();
            ed.buffer.insert(&mut it, "old");
            assert_eq!(ed.to_paras()[0].spans, vec![span("Bold", true, false)]);
            // Toggle bold off with no selection: next text is plain.
            ed.toggle(Fmt::Bold);
            let mut it = ed.buffer.end_iter();
            ed.buffer.insert(&mut it, " plain");
            assert_eq!(
                ed.to_paras()[0].spans,
                vec![span("Bold", true, false), span(" plain", false, false)]
            );
        });
    }
}
