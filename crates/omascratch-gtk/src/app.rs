use adw::prelude::*;
use gtk4::glib;
use libadwaita as adw;

use crate::APP_ID;

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

/// Smallest working window (M0). The real layout (sidebar, docked draw
/// toolbar, canvas) replaces this content in later milestones.
fn build_window(app: &adw::Application) {
    let header = adw::HeaderBar::new();
    let content = adw::StatusPage::builder()
        .title("OmaScratch")
        .description("Ink-first notes for Omarchy — development scaffold")
        .icon_name(APP_ID)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OmaScratch")
        .default_width(1100)
        .default_height(720)
        .content(&toolbar_view)
        .build();

    window.present();
}
