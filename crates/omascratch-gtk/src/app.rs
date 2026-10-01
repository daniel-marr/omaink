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
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| theme::install());
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

    // On-canvas, editable note title (top-left), like OneNote / Obsidian.
    // Commit only when editing *finishes* (the "editing" property goes false);
    // reacting to every `changed` caused title feedback/duplication.
    let title = gtk::EditableLabel::new(&storage.current_title());
    title.add_css_class("page-title");
    title.set_halign(gtk::Align::Start);
    title.set_valign(gtk::Align::Start);
    title.set_margin_start(24);
    title.set_margin_top(16);
    title.set_max_width_chars(48);
    {
        let storage = storage.clone();
        let canvas_w = canvas.downgrade();
        let sidebar = sidebar.clone();
        title.connect_notify_local(Some("editing"), move |l, _| {
            if l.property::<bool>("editing") {
                return; // editing just started
            }
            let t = l.text().to_string();
            if t.trim().is_empty() {
                l.set_text(&storage.current_title());
                return;
            }
            if t == storage.current_title() {
                return;
            }
            if let Some(canvas) = canvas_w.upgrade() {
                storage.set_title(&t, &canvas);
                sidebar.refresh();
            }
        });
    }

    // Full-width top toolbar (placeholder for the M4 draw tools). The canvas
    // title sits below it, down in the page.
    let toolbar = gtk::CenterBox::new();
    toolbar.add_css_class("main-toolbar");
    toolbar.set_hexpand(true);

    let fs_btn = gtk::Button::from_icon_name("view-fullscreen-symbolic");
    fs_btn.add_css_class("flat");
    fs_btn.set_tooltip_text(Some("Fullscreen canvas (F11)"));
    let toolbar_end = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar_end.set_margin_end(8);
    toolbar_end.append(&fs_btn);
    toolbar.set_end_widget(Some(&toolbar_end));

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&canvas));
    overlay.add_overlay(&title);

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
        let title = title.clone();
        sidebar.set_on_open_note(move |path| {
            if let Some(canvas) = canvas_w.upgrade() {
                let t = storage.switch_to(path, &canvas);
                title.set_text(&t);
            }
        });
    }

    // Renaming a note in the sidebar: if it's the open note, route through
    // storage (so autosave won't clobber it) and update the on-canvas title.
    {
        let storage = storage.clone();
        let canvas_w = canvas.downgrade();
        let library = library.clone();
        let title = title.clone();
        sidebar.set_on_rename_note(move |path, name| {
            if path == storage.current_path() {
                if let Some(canvas) = canvas_w.upgrade() {
                    storage.set_title(name, &canvas);
                }
                title.set_text(name);
            } else {
                library.rename_note_on_disk(path, name);
            }
        });
    }

    install_shortcuts(&window, &canvas, &split, toggle_fullscreen);
    install_close_handler(&window, &storage, &canvas);
    window.present();
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
