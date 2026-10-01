use adw::prelude::*;
use gtk4 as gtk;
use gtk4::{gdk, glib};
use libadwaita as adw;

use crate::canvas::CanvasView;
use crate::storage::Storage;
use crate::APP_ID;

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

/// M1 shell: header with undo/redo + the ink canvas. The full layout
/// (sidebar, docked draw toolbar, breadcrumb) arrives in M3.
fn build_window(app: &adw::Application) {
    let canvas = CanvasView::default();

    // Open the most recent note (or create the first one) and wire autosave.
    let storage = Storage::open_startup_note();
    canvas.set_content(storage.doc.borrow().content.clone());
    storage.attach_autosave(&canvas);

    let undo_btn = gtk::Button::from_icon_name("edit-undo-symbolic");
    undo_btn.set_tooltip_text(Some("Undo (Ctrl+Z)"));
    let c = canvas.clone();
    undo_btn.connect_clicked(move |_| c.undo());

    let redo_btn = gtk::Button::from_icon_name("edit-redo-symbolic");
    redo_btn.set_tooltip_text(Some("Redo (Ctrl+Shift+Z)"));
    let c = canvas.clone();
    redo_btn.connect_clicked(move |_| c.redo());

    let header = adw::HeaderBar::new();
    header.pack_start(&undo_btn);
    header.pack_start(&redo_btn);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&canvas));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OmaScratch")
        .default_width(1100)
        .default_height(720)
        .content(&toolbar_view)
        .build();

    // App-level shortcuts only; never compositor/global bindings.
    let keys = gtk::EventControllerKey::new();
    let c = canvas.clone();
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
            gdk::Key::o | gdk::Key::O if !ctrl => {
                c.toggle_debug_overlay();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);

    // Final save before the window goes away; on failure keep the window
    // open and tell the user instead of silently losing ink.
    let c = canvas.clone();
    window.connect_close_request(move |win| {
        match storage.save_final(&c) {
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
        }
    });

    window.present();
}
