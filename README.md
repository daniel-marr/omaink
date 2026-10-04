# OmaInk

<img alt="Built for Omarchy: App" height="20" src="https://raw.githubusercontent.com/tcballard/omarchy-badges/75975e5b5bf75e7ede3764bcd2950046f7abfe2c/badges/v1/omarchy-app.svg">

Ink-first notes for Omarchy: notebooks, folders and notes on a growing page, built around fluid stylus input (pressure and tilt via Wayland tablet-v2), styled from the active Omarchy theme, with a synced-folder-safe file format.

OmaInk is an independent community project, not affiliated with or endorsed by Omarchy. The OMA logo is a trademark of the Omarchy Foundation.

> **Alpha.** OmaInk is early software for testing and feedback. Expect rough edges and changes; keep backups of notes you care about. Please report problems via [GitHub Issues](../../issues).

Status: alpha (0.1.0-alpha.1). Daily-driven on one machine; not yet tested elsewhere.
Intended Omarchy target: 4 / Hyprland (Wayland).
Tested Omarchy versions: 4.0.4 only, on a single development machine with an XP-Pen Artist 15.6 Pro (see [VERIFICATION.md](VERIFICATION.md)).

## Features

- **Ink**: pressure-sensitive pen, pencil and highlighter presets with colours and widths; stroke and area erasers; pen side button = hold for eraser; the pen's eraser end erases whole strokes.
- **Page**: a OneNote-style page that grows as you add content (no scrollbars), ruled/grid backgrounds with a margin line, invert/page colour, zoom, and an **Insert space** tool.
- **Objects**: select, lasso, move, resize, copy/cut/paste; shapes (line, arrow, rectangle, ellipse); images (insert or paste, pin to background); text boxes with bold/italic/underline/highlight, colours, sizes and bullet/numbered/checklists.
- **Organisation**: notebooks → folders → notes in an Obsidian-style sidebar, title search (Ctrl+F), multi-select (Ctrl/Shift+click) to move or delete notes.
- **Omarchy**: follows the active Omarchy theme live, including when you switch themes.
- **Settings**: notebooks folder, pen & ink (pressure response, smoothing, side button, mouse/touch drawing), new page defaults, page colour, what opens on launch.
- **Storage**: one plain file per note in a folder you choose (default `~/Documents/OmaInk`), safe to sync with Dropbox, Google Drive or Syncthing; autosave and crash-safe writes.

## Known limitations

- Only tested with one pen display (XP-Pen Artist 15.6 Pro) on the kernel driver. Vendor tablet apps (XP-Pen, Huion) remap the pen's side buttons into clicks or keys, so OmaInk can't see them as pen buttons yet.
- No export or printing yet; no built-in cloud sync (point a sync client at the notebooks folder instead).
- Very large notes (thousands of strokes) take a fraction of a second to open.
- Search covers note titles in the current notebook only.

## Build and run

Requires Rust 1.92+, GTK 4.18+ and libadwaita 1.6+ (on Omarchy/Arch: `sudo pacman -S --needed rust gtk4 libadwaita`).

```sh
git clone https://github.com/daniel-marr/omaink.git
cd omaink
cargo test --workspace
cargo run -p omaink-gtk --release
```

## Install and rollback

No system package exists yet. For a per-user install (no root) that adds OmaInk to the app launcher:

```sh
packaging/install-local.sh              # release build → ~/.local/bin/omaink + launcher + icon
packaging/install-local.sh --uninstall  # remove exactly those three files
```

It installs `~/.local/bin/omaink`, `~/.local/share/applications/co.think3.OmaInk.desktop` and `~/.local/share/icons/hicolor/scalable/apps/co.think3.OmaInk.svg`, then refreshes the desktop and icon caches. `~/.local/bin` must be on the session PATH (it is on Omarchy).

The app itself writes only to XDG app directories (`~/.config/omaink`, `~/.local/state/omaink`, `~/.cache/omaink`) and the chosen notebooks folder (default `~/Documents/OmaInk`). Uninstalling never touches those; remove them by hand to fully revert. Notebooks are plain files you own. A system package (PKGBUILD) is a later milestone.

## Layout

- `crates/omaink-core` — domain model, commands, undo (UI-free, IO-free)
- `crates/omaink-ink` — stroke engine: samples, smoothing, outlines, hit-testing
- `crates/omaink-store` — on-disk format, atomic writes, notebook scanning
- `crates/omaink-theme` — Omarchy `colors.toml` → palette → GTK CSS (pure)
- `crates/omaink-gtk` — GTK4/libadwaita shell, binary `omaink`
- `packaging/` — desktop entry, icon (`co.think3.OmaInk`), logo source and `install-local.sh`

## Evidence

See [verification](VERIFICATION.md), [architecture](ARCHITECTURE.md) and [credits](CREDITS.md). This community badge does not imply official acceptance.

## Licence and credits

GPL-3.0-or-later — see [LICENSE](LICENSE). Created by Daniel Marr · [think3.co](https://think3.co). Third-party credits in [CREDITS.md](CREDITS.md).
