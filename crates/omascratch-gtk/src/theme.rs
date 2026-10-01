//! M3 theme shim: installs the app's structural CSS with a fixed Tokyo
//! Night-ish palette (matching the canvas and the user's reference). M5
//! replaces the hardcoded palette with the live Omarchy `colors.toml` adapter
//! plus hot reload; the CSS structure here is meant to carry over.

use gtk4 as gtk;

const CSS: &str = r#"
/* Compact Obsidian-like density on a dark navy palette. */
.sidebar-pane {
    background-color: #16161e;
}
.sidebar-header .heading {
    font-size: 0.85rem;
    color: #565f89;
}
/* Tight Obsidian-like rows with no vertical gaps (so indent guides are
   continuous between consecutive rows). */
.navigation-sidebar {
    background-color: #16161e;
    padding: 4px 8px;
}
.navigation-sidebar row {
    min-height: 24px;
    border-radius: 4px;
    margin: 0;
    padding: 0;
}
.navigation-sidebar row:selected {
    background-color: #283457;
    color: #c0caf5;
}
.row-title {
    font-size: 0.875rem;
    font-weight: 400;
}
.folder-title {
    font-weight: 500;
    color: #9aa5ce;
}
.folder-chevron {
    color: #8891b3;
}
/* Smaller add-note / add-folder buttons in the sidebar header. */
.header-icon {
    min-height: 24px;
    min-width: 24px;
    padding: 2px;
}
.header-icon image {
    -gtk-icon-size: 15px;
}
/* 1px vertical indent guide, centered in its INDENT-wide cell. */
.indent-line {
    background-color: #2a2e42;
}
.row-menu {
    min-height: 20px;
    min-width: 20px;
    padding: 0 2px;
    opacity: 0.0;
}
.navigation-sidebar row:hover .row-menu,
.navigation-sidebar row:selected .row-menu {
    opacity: 0.7;
}
.notebook-switcher {
    color: #c0caf5;
    font-size: 0.9rem;
}
.rename-entry {
    min-height: 22px;
    padding: 0 4px;
    border-radius: 4px;
}

/* Full-width top toolbar (future draw tools). */
.main-toolbar {
    background-color: #16161e;
    border-bottom: 1px solid #2a2e42;
    min-height: 40px;
}

/* On-canvas page title (OneNote/Obsidian style). */
.page-title {
    font-size: 1.6rem;
    font-weight: 700;
    color: #c0caf5;
}
.page-title:not(:focus-within) {
    background: transparent;
}
.page-title text {
    background: transparent;
}
"#;

pub fn install() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
