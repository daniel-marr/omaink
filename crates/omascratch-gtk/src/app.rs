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
    let app = builder.build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &adw::Application) {
    let canvas = CanvasView::default();

    let library = Library::open();
    library.ensure_notebook();
    let first_note = first_note_path(&library);
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
        // Stylus barrel click toggles the eraser (and back).
        let tb = draw_toolbar.clone();
        canvas.set_on_eraser_toggle(move || tb.toggle_eraser());
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
    fs_btn.set_tooltip_text(Some("Fullscreen canvas (F11)"));
    // Zoom indicator: shows the live zoom, click to return to 100%.
    let zoom_btn = gtk::Button::with_label("100%");
    zoom_btn.add_css_class("flat");
    zoom_btn.add_css_class("zoom-indicator");
    zoom_btn.set_tooltip_text(Some("Zoom — click for 100% (Ctrl+0 resets view)"));
    {
        let c = canvas.clone();
        zoom_btn.connect_clicked(move |_| c.zoom_to_100());
    }
    {
        let zb = zoom_btn.clone();
        canvas.set_on_zoom(move |z| zb.set_label(&format!("{:.0}%", z * 100.0)));
    }

    let toolbar_end = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar_end.set_margin_end(8);
    toolbar_end.append(&zoom_btn);
    toolbar_end.append(&fs_btn);
    toolbar.set_end_widget(Some(&toolbar_end));

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&canvas));
    canvas.attach_editor_host(&overlay);

    let content_view = adw::ToolbarView::new();
    content_view.set_top_bar_style(adw::ToolbarStyle::Flat);
    content_view.add_top_bar(&toolbar);
    content_view.set_content(Some(&overlay));

    // Split view: sidebar | content. Narrower than before.
    let split = adw::OverlaySplitView::new();
    split.set_sidebar(Some(&sidebar.widget));
    split.set_content(Some(&content_view));
    split.set_min_sidebar_width(170.0);
    split.set_max_sidebar_width(240.0);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OmaScratch")
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
                    canvas.set_asset_dir(omascratch_store::assets_dir(&new_path));
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
            gdk::Key::o | gdk::Key::O if !ctrl => {
                c.toggle_debug_overlay();
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
        Ok(()) => glib::Propagation::Proceed,
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
