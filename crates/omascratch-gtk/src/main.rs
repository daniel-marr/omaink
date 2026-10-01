//! Startup boundary: app identity, dependency construction, window creation.
//! No document rules or persistence logic belongs here.

mod app;
mod canvas;
mod library;
mod sidebar;
mod storage;
mod theme;

/// Must equal the Wayland app_id, the .desktop basename and the icon basename.
pub const APP_ID: &str = "co.think3.OmaScratch";

fn main() -> gtk4::glib::ExitCode {
    app::run()
}
