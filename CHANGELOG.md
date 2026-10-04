# Changelog

## 0.1.0-alpha.1 — first public alpha (unreleased)

First public test build. Source only; build with Rust against the system
GTK 4.18+ and libadwaita 1.6+ (see README). Tested on Omarchy 4.0.4 only.

- Pressure-sensitive pen, pencil and highlighter presets; stroke and area
  erasers; pen side button holds the eraser; pen eraser end erases strokes.
- Growing OneNote-style page with ruled/grid backgrounds, margin line,
  page colour/invert, zoom and an Insert space tool.
- Select, lasso, move, resize, copy/cut/paste; shapes; images (insert, paste,
  pin to background); rich text boxes with lists and checkboxes.
- Notebooks → folders → notes sidebar with title search and multi-select.
- Live Omarchy theming; Settings window (storage folder, pen & ink, new page
  defaults, page colour, launch behaviour).
- One file per note in a user-chosen folder, crash-safe writes, safe to sync
  with Dropbox/Google Drive/Syncthing.
- Per-user install script with uninstall (`packaging/install-local.sh`).
