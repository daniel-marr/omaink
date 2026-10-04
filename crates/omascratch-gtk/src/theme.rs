//! Omarchy theme manager (M5): loads the active theme's `colors.toml`,
//! applies it to the app CSS, the Adwaita color scheme and the canvas
//! palette, and hot-reloads when the user switches Omarchy themes.
//!
//! Omarchy replaces the whole theme *directory* on switch (`rm -rf` + `mv`),
//! so the watcher monitors the parent `~/.local/state/omarchy/current/` and
//! tolerates a brief window where the theme is missing (last-good fallback,
//! persisted so even a fresh start during a broken switch looks right).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::{gio, glib, prelude::*};
use libadwaita as adw;

use omascratch_core::Rgba;
use omascratch_store as store;
use omascratch_theme::Palette;

use crate::canvas::{CanvasPalette, CanvasView};
use crate::toolbar::Toolbar;

const LAST_GOOD: &str = "last-theme.toml";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn omarchy_current_dir() -> PathBuf {
    home().join(".local/state/omarchy/current")
}

fn colors_paths() -> [PathBuf; 2] {
    [
        home().join(".local/state").join(omascratch_theme::OMARCHY_COLORS_RELPATH),
        home().join(".config").join(omascratch_theme::OMARCHY_LEGACY_RELPATH),
    ]
}

fn gdk(c: Rgba) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::new(c.r, c.g, c.b, c.a)
}

fn with_alpha(c: Rgba, a: f32) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::new(c.r, c.g, c.b, a)
}

thread_local! {
    /// Settings → Appearance → Page color.
    static PAGE_COLOR: std::cell::Cell<store::PageColor> = const { std::cell::Cell::new(store::PageColor::Theme) };
}

/// Set the page color mode; call `Manager::refresh_canvas` to apply it.
pub fn set_page_color(mode: store::PageColor) {
    PAGE_COLOR.with(|c| c.set(mode));
}

/// The theme's canvas palette, with the normal/inverted pages swapped when
/// the page color setting asks for the opposite of the theme's polarity.
fn canvas_palette(p: &Palette) -> CanvasPalette {
    let pal = theme_canvas_palette(p);
    let want_dark = match PAGE_COLOR.with(|c| c.get()) {
        store::PageColor::Theme => p.dark,
        store::PageColor::Light => false,
        store::PageColor::Dark => true,
    };
    if want_dark == p.dark {
        return pal;
    }
    CanvasPalette {
        bg: pal.bg_inv,
        bg_inv: pal.bg,
        ink: pal.ink_inv,
        ink_inv: pal.ink,
        accent: pal.accent,
        rule: pal.rule_inv,
        rule_inv: pal.rule,
        margin: pal.margin_inv,
        margin_inv: pal.margin,
    }
}

fn theme_canvas_palette(p: &Palette) -> CanvasPalette {
    // A light page is always pure white (#ffffff): the inverted page on dark
    // themes, and the normal page on light themes. A light theme's inverted
    // page stays dark (its brightest ink color).
    let white = gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
    let (bg, bg_inv) = if p.dark {
        (gdk(p.background), white)
    } else {
        (white, gdk(p.bright_foreground))
    };
    CanvasPalette {
        bg,
        bg_inv,
        ink: gdk(p.bright_foreground),
        ink_inv: gdk(p.background),
        accent: gdk(p.accent),
        rule: with_alpha(p.muted, 0.8),
        rule_inv: with_alpha(p.dark_foreground, 0.65),
        margin: with_alpha(p.red, 0.85),
        margin_inv: with_alpha(p.red, 0.85),
    }
}

/// Apply the current Omarchy theme to the app chrome before any main window
/// exists (the first-run welcome). The main window's `Manager` takes over
/// (and hot-reloads) once it starts.
pub fn preload() {
    let provider = gtk::CssProvider::new();
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    let palette = Manager::load_palette();
    provider.load_from_string(&palette.app_css());
    adw::StyleManager::default().set_color_scheme(if palette.dark {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    });
}

pub struct Manager {
    provider: gtk::CssProvider,
    canvas: glib::WeakRef<CanvasView>,
    toolbar: RefCell<Option<Toolbar>>,
    current: RefCell<Palette>,
    _monitor: RefCell<Option<gio::FileMonitor>>,
    debounce: RefCell<Option<glib::SourceId>>,
}

impl Manager {
    /// Create, apply the current (or last-good, or fallback) theme, and start
    /// watching for Omarchy theme switches.
    pub fn start(canvas: &CanvasView) -> Rc<Manager> {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let mgr = Rc::new(Manager {
            provider,
            canvas: glib::WeakRef::new(),
            toolbar: RefCell::new(None),
            current: RefCell::new(Palette::fallback()),
            _monitor: RefCell::new(None),
            debounce: RefCell::new(None),
        });
        mgr.canvas.set(Some(canvas));

        let palette = Self::load_palette();
        mgr.apply(&palette);

        // Watch the PARENT dir: the theme dir itself is replaced on switch.
        let dir = omarchy_current_dir();
        if dir.exists() {
            let file = gio::File::for_path(&dir);
            if let Ok(monitor) = file.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
                let m = Rc::downgrade(&mgr);
                monitor.connect_changed(move |_, _, _, _| {
                    if let Some(mgr) = m.upgrade() {
                        mgr.schedule_reload();
                    }
                });
                *mgr._monitor.borrow_mut() = Some(monitor);
            }
        }
        mgr
    }

    /// Re-apply the canvas palette (after the page color setting changes).
    pub fn refresh_canvas(&self) {
        if let Some(canvas) = self.canvas.upgrade() {
            canvas.set_palette(canvas_palette(&self.current.borrow()));
        }
        if let Some(toolbar) = self.toolbar.borrow().as_ref() {
            toolbar.refresh_theme();
        }
    }

    /// The toolbar re-renders its pen glyphs on theme change.
    pub fn attach_toolbar(&self, toolbar: &Toolbar) {
        *self.toolbar.borrow_mut() = Some(toolbar.clone());
    }

    fn schedule_reload(self: &Rc<Self>) {
        if let Some(id) = self.debounce.borrow_mut().take() {
            id.remove();
        }
        let m = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            if let Some(mgr) = m.upgrade() {
                *mgr.debounce.borrow_mut() = None;
                mgr.reload();
            }
        });
        *self.debounce.borrow_mut() = Some(id);
    }

    fn reload(self: &Rc<Self>) {
        // Mid-switch the file may be briefly missing or malformed: keep the
        // current palette and let the next event retry.
        for path in colors_paths() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Some(p) = Palette::parse(&text) {
                    if p != *self.current.borrow() {
                        self.apply(&p);
                        Self::persist_last_good(&text);
                        tracing::info!("omarchy theme reloaded from {}", path.display());
                    }
                    return;
                }
            }
        }
        tracing::warn!("omarchy theme changed but colors.toml unreadable; keeping last-good palette");
    }

    fn load_palette() -> Palette {
        for path in colors_paths() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Some(p) = Palette::parse(&text) {
                    Self::persist_last_good(&text);
                    return p;
                }
            }
        }
        // Live theme unreadable: try the persisted last-good copy.
        if let Ok(text) = std::fs::read_to_string(store::state_dir().join(LAST_GOOD)) {
            if let Some(p) = Palette::parse(&text) {
                return p;
            }
        }
        Palette::fallback()
    }

    fn persist_last_good(text: &str) {
        let dir = store::state_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = store::atomic::atomic_write(&dir.join(LAST_GOOD), text.as_bytes());
    }

    fn apply(&self, palette: &Palette) {
        *self.current.borrow_mut() = *palette;
        self.provider.load_from_string(&palette.app_css());
        let scheme = if palette.dark {
            adw::ColorScheme::ForceDark
        } else {
            adw::ColorScheme::ForceLight
        };
        adw::StyleManager::default().set_color_scheme(scheme);
        if let Some(canvas) = self.canvas.upgrade() {
            canvas.set_palette(canvas_palette(palette));
        }
        if let Some(toolbar) = self.toolbar.borrow().as_ref() {
            toolbar.refresh_theme();
        }
    }
}
