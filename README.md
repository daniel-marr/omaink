# OmaInk

<img alt="Built for Omarchy: App" height="20" src="https://raw.githubusercontent.com/tcballard/omarchy-badges/75975e5b5bf75e7ede3764bcd2950046f7abfe2c/badges/v1/omarchy-app.svg">

Ink-first notes for Omarchy: notebooks, folders and notes on an infinite canvas, built around fluid stylus input (pressure/tilt via Wayland tablet-v2), theme-adaptive to the active Omarchy theme, with a synced-folder-safe file format.

Status: in development; ink, persistence, organization, full OneNote-style draw toolset and live Omarchy theming working on Hyprland.
Intended Omarchy target: 4 / Hyprland; establish a supported range during implementation.
Tested Omarchy versions: 4.0.4 (development machine only; see VERIFICATION.md). Live desktop acceptance: window mapping and stylus ink verified on the dev machine; everything else not yet.

## Build and run

Requires Rust (1.92+), GTK4 (4.14+) and libadwaita (1.6+) development libraries. On Omarchy/Arch: `gtk4` and `libadwaita` packages.

```sh
cargo build --workspace
cargo test --workspace
./target/debug/omaink
```

## Install and rollback

No package exists yet; nothing is installed system-wide. Running the development binary writes only to XDG app directories (`~/.config/omaink`, `~/.local/state/omaink`, `~/.cache/omaink`) and the chosen notebooks folder (default `~/Documents/OmaInk`). Remove those paths to fully revert; notebooks are plain files you own. Packaging (PKGBUILD) is a later milestone.

## Layout

- `crates/omaink-core` — domain model, commands, undo (UI-free, IO-free)
- `crates/omaink-ink` — stroke engine: samples, smoothing, outlines, hit-testing
- `crates/omaink-store` — on-disk format, atomic writes, notebook scanning
- `crates/omaink-theme` — Omarchy `colors.toml` → palette → GTK CSS (pure)
- `crates/omaink-gtk` — GTK4/libadwaita shell, binary `omaink`
- `packaging/` — desktop entry + icon (`co.think3.OmaInk`)

## Evidence

See [verification](VERIFICATION.md), [architecture](ARCHITECTURE.md) and [credits](CREDITS.md). This community badge does not imply official acceptance.

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
