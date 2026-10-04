use adw::prelude::*;
use gtk4 as gtk;
use gtk4::{gdk, glib};
use libadwaita as adw;
use std::rc::Rc;

use crate::canvas::CanvasView;
use crate::library::Library;
use crate::sidebar::Sidebar;
use crate::storage::Storage;
use crate::theme;
use crate::APP_ID;

pub fn run() -> glib::ExitCode {
    let mut builder = adw::Application::builder().application_id(APP_ID);
    if crate::perf::enabled() {
        // Benchmark runs must not hand off to an already-running instance.
        builder = builder.flags(gtk4::gio::ApplicationFlags::NON_UNIQUE);
    }
    // Renamed from OmaScratch: carry over settings and state once.
    for dir in omaink_store::migrate_legacy_dirs() {
        eprintln!("omaink: migrated settings from OmaScratch into {}", dir.display());
    }
    let app = builder.build();
    app.connect_activate(|app| {
        // First launch: ask where notebooks live before building the window.
        if !omaink_store::Settings::exists(&omaink_store::config_dir()) {
            crate::theme::preload();
            let app2 = app.clone();
            crate::settings::present_welcome(app, std::rc::Rc::new(move |_| build_window(&app2)));
        } else {
            build_window(app);
        }
    });
    app.run()
}

fn build_window(app: &adw::Application) {
    let canvas = CanvasView::default();
    apply_settings(&canvas, &omaink_store::Settings::load_or_default(&omaink_store::config_dir()));

    let library = Library::open();
    library.ensure_notebook();
    let first_note = startup_note(&library);
    omaink_store::save_last_note(&omaink_store::state_dir(), &first_note);
    let storage = Storage::open(first_note);
    storage.load_into_canvas(&canvas);
    storage.attach_autosave(&canvas);

    let sidebar = Sidebar::new(library.clone());
    sidebar.set_selected(&storage.current_path());
    sidebar.refresh();

    // Full-width top toolbar with the draw tools. The canvas title sits below
    // it, down in the page.
    let toolbar = gtk::CenterBox::new();
    toolbar.add_css_class("main-toolbar");
    toolbar.set_hexpand(true);
    let theme_mgr = theme::Manager::start(&canvas);
    let draw_toolbar = crate::toolbar::build(&canvas);
    theme_mgr.attach_toolbar(&draw_toolbar);
    {
        // Stylus barrel button: hold for the eraser, or click to toggle it
        // (Settings → Pen & ink → Pen side button).
        let tb = draw_toolbar.clone();
        canvas.set_on_eraser_toggle(move || tb.toggle_eraser());
        let tb = draw_toolbar.clone();
        canvas.set_on_eraser_hold(move |held| tb.hold_eraser(held));
    }
    {
        // Pasted images: bytes saved beside the open note; afterwards the
        // toolbar switches to Select so the image can be moved right away.
        let st = storage.clone();
        canvas.set_asset_writer(move |bytes, ext| st.write_asset(bytes, ext));
        let tb = draw_toolbar.clone();
        canvas.set_on_request_select(move || tb.select_tool());
    }
    toolbar.set_start_widget(Some(&draw_toolbar.widget));
    {
        // Persist background changes into the open note.
        let storage = storage.clone();
        let canvas_w = canvas.downgrade();
        draw_toolbar.set_on_background(move |bg| {
            if let Some(canvas) = canvas_w.upgrade() {
                storage.set_background(bg, &canvas);
            }
        });
    }

    let fs_btn = gtk::Button::from_icon_name("view-fullscreen-symbolic");
    fs_btn.add_css_class("flat");
    fs_btn.add_css_class("compact-btn");
    fs_btn.set_tooltip_text(Some("Fullscreen canvas (F11)"));
    // Zoom indicator: shows the live zoom, click to return to 100%.
    let zoom_btn = gtk::Button::with_label("100%");
    zoom_btn.add_css_class("flat");
    zoom_btn.add_css_class("zoom-indicator");
    zoom_btn.set_tooltip_text(Some("Zoom — click for 100% (Ctrl+0 resets view)"));
    let zoom_out_btn = gtk::Button::with_label("−");
    zoom_out_btn.add_css_class("flat");
    zoom_out_btn.add_css_class("compact-btn");
    zoom_out_btn.set_tooltip_text(Some("Zoom out (Ctrl+−)"));
    let zoom_in_btn = gtk::Button::with_label("+");
    zoom_in_btn.add_css_class("flat");
    zoom_in_btn.add_css_class("compact-btn");
    zoom_in_btn.set_tooltip_text(Some("Zoom in (Ctrl+=)"));
    {
        let c = canvas.clone();
        zoom_out_btn.connect_clicked(move |_| c.zoom_step(false));
        let c = canvas.clone();
        zoom_in_btn.connect_clicked(move |_| c.zoom_step(true));
    }
    {
        let c = canvas.clone();
        zoom_btn.connect_clicked(move |_| c.zoom_to_100());
    }
    {
        let zb = zoom_btn.clone();
        canvas.set_on_zoom(move |z| zb.set_label(&format!("{:.0}%", z * 100.0)));
    }

    let toolbar_end = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    toolbar_end.set_margin_end(4);
    let zoom_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    zoom_group.append(&zoom_out_btn);
    zoom_group.append(&zoom_btn);
    zoom_group.append(&zoom_in_btn);
    toolbar_end.append(&zoom_group);
    toolbar_end.append(&fs_btn);
    toolbar.set_end_widget(Some(&toolbar_end));

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&canvas));
    canvas.attach_editor_host(&overlay);
    // Pen diagnostics (pressure/tilt/sample rate): opt-in at launch only.
    if std::env::var_os("OMAINK_DEBUG_OVERLAY").is_some() {
        canvas.toggle_debug_overlay();
    }

    let content_view = adw::ToolbarView::new();
    content_view.set_top_bar_style(adw::ToolbarStyle::Flat);
    content_view.add_top_bar(&toolbar);
    // The sidebar's NOTES header is exactly as tall as the draw toolbar, so
    // the search box below it starts level with the top of the page.
    let top_rows = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
    top_rows.add_widget(&toolbar);
    top_rows.add_widget(&sidebar.header);
    content_view.set_content(Some(&overlay));

    // Split view: sidebar | content. Narrower than before.
    let split = adw::OverlaySplitView::new();
    split.set_sidebar(Some(&sidebar.widget));
    split.set_content(Some(&content_view));
    split.set_min_sidebar_width(170.0);
    split.set_max_sidebar_width(240.0);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OmaInk")
        .default_width(1200)
        .default_height(780)
        .content(&split)
        .build();

    // Fullscreen = canvas only (also collapses the sidebar).
    let toggle_fullscreen = {
        let win = window.clone();
        let split = split.clone();
        move || {
            if win.is_fullscreen() {
                win.unfullscreen();
                split.set_show_sidebar(true);
            } else {
                win.fullscreen();
                split.set_show_sidebar(false);
            }
        }
    };
    {
        let tf = toggle_fullscreen.clone();
        fs_btn.connect_clicked(move |_| tf());
    }

    // Opening a note from the sidebar: switch storage + canvas + title.
    {
        let storage = storage.clone();
        let canvas_w = canvas.downgrade();
        sidebar.set_on_open_note(move |path| {
            if let Some(canvas) = canvas_w.upgrade() {
                let _ = storage.switch_to(path, &canvas);
                omaink_store::save_last_note(&omaink_store::state_dir(), path);
            }
        });
    }

    // Renaming a note in the sidebar: if it's the open note, route through
    // storage (so autosave won't clobber it) and update the on-canvas title.
    {
        let storage = storage.clone();
        let canvas_w = canvas.downgrade();
        let library = library.clone();
        sidebar.set_on_rename_note(move |path, name| {
            if path == storage.current_path() {
                if let Some(canvas) = canvas_w.upgrade() {
                    storage.set_title(name, &canvas);
                }
            } else {
                library.rename_note_on_disk(path, name);
            }
        });
    }

    // Notebook renamed: if the open note lived inside, follow its new path.
    {
        let storage = storage.clone();
        let canvas_w = canvas.downgrade();
        sidebar.set_on_notebook_renamed(move |old_dir, new_dir| {
            let cur = storage.current_path();
            if let Ok(rest) = cur.strip_prefix(old_dir) {
                let new_path = new_dir.join(rest);
                if let Some(canvas) = canvas_w.upgrade() {
                    canvas.set_asset_dir(omaink_store::assets_dir(&new_path));
                }
                storage.relocate(new_path);
            }
        });
    }
    // Notebook trashed: if the open note was inside, open another note
    // (without saving into the trashed tree).
    {
        let storage = storage.clone();
        let canvas_w = canvas.downgrade();
        let library = library.clone();
        sidebar.set_on_notebook_deleted(move |old_dir| {
            if storage.current_path().starts_with(old_dir) {
                if let Some(canvas) = canvas_w.upgrade() {
                    library.ensure_notebook();
                    let next = first_note_path(&library);
                    storage.abandon_and_open(&next, &canvas);
                }
            }
        });
    }

    install_shortcuts(&window, &canvas, &split, toggle_fullscreen);
    // Ctrl+F → sidebar title search (reveals the sidebar if hidden).
    {
        let find = gtk::EventControllerKey::new();
        find.set_propagation_phase(gtk::PropagationPhase::Capture);
        let sb = sidebar.clone();
        let split = split.clone();
        find.connect_key_pressed(move |_, key, _, m| {
            if matches!(key, gdk::Key::f | gdk::Key::F) && m.contains(gdk::ModifierType::CONTROL_MASK) {
                split.set_show_sidebar(true);
                sb.focus_search();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(find);
    }
    // Settings (gear). Changing the notebooks folder saves the open note,
    // then rebuilds the window against the new folder.
    {
        let win = window.downgrade();
        let app = app.clone();
        let storage = storage.clone();
        let canvas = canvas.clone();
        let root = library.root.clone();
        let theme_mgr = theme_mgr.clone();
        sidebar.set_on_settings(move || {
            let Some(w) = win.upgrade() else { return };
            let win = win.clone();
            let app = app.clone();
            let storage = storage.clone();
            let canvas = canvas.clone();
            crate::settings::present_settings(
                w.upcast_ref(),
                root.clone(),
                std::rc::Rc::new({
                    let canvas = canvas.clone();
                    move |_new_root| {
                    let Some(w) = win.upgrade() else { return };
                    if let Err(e) = storage.save_final(&canvas) {
                        let d = adw::AlertDialog::new(
                            Some("Could not save your note"),
                            Some(&format!("{e}\n\nThe notebooks folder was changed; it takes effect after the next restart.")),
                        );
                        d.add_response("ok", "OK");
                        d.present(Some(&w));
                        return;
                    }
                    omaink_store::save_last_note(&omaink_store::state_dir(), &storage.current_path());
                    w.destroy();
                    build_window(&app);
                }}),
                std::rc::Rc::new({
                    let canvas = canvas.clone();
                    let theme_mgr = theme_mgr.clone();
                    move |s: &omaink_store::Settings| {
                        apply_settings(&canvas, s);
                        theme_mgr.refresh_canvas();
                    }
                }),
            );
        });
    }
    install_close_handler(&window, &storage, &canvas);
    // Keep the theme manager (CSS provider + file watcher) alive with the window.
    window.connect_map(move |_| {
        let _ = &theme_mgr;
    });
    window.present();
    if crate::perf::enabled() {
        canvas.run_perf_bench();
    }
}

/// Apply everything in Settings except the notebooks folder (live).
fn apply_settings(canvas: &CanvasView, s: &omaink_store::Settings) {
    theme::set_page_color(s.appearance.page_color);
    canvas.apply_ink_settings(s.ink);
    omaink_store::set_new_note_background(s.page.background);
    canvas.set_default_text_size(s.page.text_size);
}

/// The note to open at launch (Settings → General → On launch).
fn startup_note(library: &Rc<Library>) -> std::path::PathBuf {
    use omaink_store as store;
    let settings = store::Settings::load_or_default(&store::config_dir());
    let last = store::load_last_note(&store::state_dir()).filter(|p| p.starts_with(&library.root));
    // The last note's notebook: <root>/<notebook>/notes/<id>.omanote.
    if let Some(name) = last
        .as_ref()
        .and_then(|p| p.parent()?.parent()?.file_name())
        .map(|n| n.to_string_lossy().to_string())
    {
        library.select_notebook(&name);
    }
    match settings.general.on_launch {
        store::OnLaunch::LastNote => {
            if let Some(p) = last {
                return p;
            }
        }
        store::OnLaunch::NewNote => {
            // Reuse a still-blank "Untitled" note rather than piling them up.
            if let Some(p) = last.filter(|p| {
                store::read_note(p).is_ok_and(|d| {
                    d.title == "Untitled"
                        && d.content.strokes.is_empty()
                        && d.content.images.is_empty()
                        && d.content.texts.is_empty()
                })
            }) {
                return p;
            }
            if let Some((_, path)) = library.new_note(None) {
                return path;
            }
        }
    }
    first_note_path(library)
}

fn first_note_path(library: &Rc<Library>) -> std::path::PathBuf {
    use crate::library::Row;
    for row in library.rows() {
        if let Row::Note { path, .. } = row {
            return path;
        }
    }
    if let Some((_, path)) = library.new_note(None) {
        return path;
    }
    library.root.join("untitled.omanote")
}

fn install_shortcuts(
    window: &adw::ApplicationWindow,
    canvas: &CanvasView,
    split: &adw::OverlaySplitView,
    toggle_fullscreen: impl Fn() + 'static,
) {
    let keys = gtk::EventControllerKey::new();
    let c = canvas.clone();
    let split = split.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        match key {
            gdk::Key::z | gdk::Key::Z if ctrl && shift => {
                c.redo();
                glib::Propagation::Stop
            }
            gdk::Key::z if ctrl => {
                c.undo();
                glib::Propagation::Stop
            }
            gdk::Key::y if ctrl => {
                c.redo();
                glib::Propagation::Stop
            }
            gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add if ctrl => {
                c.zoom_step(true);
                glib::Propagation::Stop
            }
            gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract if ctrl => {
                c.zoom_step(false);
                glib::Propagation::Stop
            }
            gdk::Key::_0 if ctrl => {
                c.reset_view();
                glib::Propagation::Stop
            }
            gdk::Key::F9 => {
                split.set_show_sidebar(!split.shows_sidebar());
                glib::Propagation::Stop
            }
            gdk::Key::F11 => {
                toggle_fullscreen();
                glib::Propagation::Stop
            }
            gdk::Key::c if ctrl && c.has_selection() => {
                c.copy_selection();
                glib::Propagation::Stop
            }
            gdk::Key::x if ctrl && c.has_selection() => {
                c.cut_selection();
                glib::Propagation::Stop
            }
            gdk::Key::v if ctrl => {
                c.paste();
                glib::Propagation::Stop
            }
            gdk::Key::Delete | gdk::Key::BackSpace if c.has_selection() => {
                c.delete_selection();
                glib::Propagation::Stop
            }
            gdk::Key::Escape if c.has_selection() => {
                c.clear_selection();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);
}

fn install_close_handler(
    window: &adw::ApplicationWindow,
    storage: &Rc<Storage>,
    canvas: &CanvasView,
) {
    let storage = storage.clone();
    let c = canvas.clone();
    window.connect_close_request(move |win| match storage.save_final(&c) {
        Ok(()) => {
            omaink_store::save_last_note(&omaink_store::state_dir(), &storage.current_path());
            glib::Propagation::Proceed
        }
        Err(e) => {
            let dialog = adw::AlertDialog::new(
                Some("Could not save your note"),
                Some(&format!("{e}\n\nClose anyway and lose the latest changes?")),
            );
            dialog.add_response("cancel", "Keep editing");
            dialog.add_response("discard", "Close anyway");
            dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            let win2 = win.clone();
            dialog.connect_response(None, move |_, resp| {
                if resp == "discard" {
                    win2.destroy();
                }
            });
            dialog.present(Some(win));
            glib::Propagation::Stop
        }
    });
}
