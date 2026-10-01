# Verification

Input identity: working tree at first scaffold commit (see git history; EVIDENCE.sha256 snapshot deferred until a release candidate).
Upstream revision: none (original project, no upstream).
Toolchain/platform: rustc 1.98.1 (Arch Linux 1:1.98.1-1), gtk4 1:4.22.4-1, libadwaita 1:1.9.3-1, Omarchy 4.0.4-1 / Hyprland (Wayland), x86_64.

## Reproduced now

Run 2026-10-01 on the development machine (Omarchy 4.0.4, live Hyprland session):

- `cargo build --workspace` — exit 0.
- `cargo test --workspace` — exit 0 (2 passed: `omascratch-core` id ordering, `omascratch-ink` serde roundtrip).
- `desktop-file-validate packaging/co.think3.OmaScratch.desktop` — exit 0, no warnings.
- `./target/debug/omascratch` launched in the live Hyprland session; `hyprctl clients` showed the window `mapped: 1`, `class: co.think3.OmaScratch`, `initialClass: co.think3.OmaScratch`, `xwayland: 0`. Window closed cleanly.
  - Note: GDK logged a Vulkan `VK_ERROR_INCOMPATIBLE_DRIVER` warning and fell back to the GL renderer on this machine. Track renderer choice when ink performance is measured.

### M1 ink slice — live Hyprland acceptance (2026-10-01, user-performed)

Hardware: XP-Pen Artist 15.6 Pro (kernel `uclogic`, Wayland tablet-v2), Omarchy 4.0.4 live session, debug overlay (`O`) observed by the user:

- Pressure axis sweeps ~0.0→1.0 and stroke width follows it — confirmed.
- Tilt axes report non-zero values when the pen is angled — confirmed.
- Sample rate mid-stroke: ~210 samples/s (backlog draining active).
- Subjective latency/feel: "looks and feels great"; no hooks/tails reported.
- `cargo test --workspace` — exit 0 (10 passed: 7 core, 3 ink).
- Stylus notes: this pen has no eraser end. A barrel button pans while hovering (compositor/driver mapping) but is inert while the tip is down — in-stroke button chords are app work, tracked for M4.

### M2 persistence — live Hyprland acceptance (2026-10-01)

Omarchy 4.0.4 live session, user drawing with the XP-Pen, verified by inspecting the on-disk `.omanote` (zstd+JSON):

- First-run creation: launching with no data created `~/.config/omascratch/settings.toml`, `~/Documents/OmaScratch/My Notebook/notebook.json`, and a first `.omanote` (schema 1, empty `elements`). Settings and notebook metadata are human-readable TOML/JSON as designed.
- Clean-close save: drew 5 strokes, closed the window; final-save handler wrote all 5 strokes (9.3 KB note).
- Reopen round-trip: strokes reloaded onto the canvas (user-confirmed).
- Crash recovery cycle 1: drew more (19 elements on disk via debounced autosave), `kill -9` with no clean shutdown; reopen recovered all 19, no stale `.omatmp-*` debris.
- Crash recovery cycle 2 (model-driven): 62 elements on disk, `kill -9`, disk still showed exactly 62 afterward, no stale temps, reopened cleanly.
- `cargo test --workspace` — exit 0 (20 passed; store suite covers interrupted-write simulation, corrupt/newer-schema refusal, unknown-element round-trip, conflicted-copy detection, two-machine file-copy merge).

### M3 organization UI — live Hyprland acceptance (2026-10-01, user-verified)

Obsidian-style sidebar on the live session; verified interactively by the user:

- Notebook → Folders (nestable) → Notes tree; single-click opens a note, single-click toggles folder collapse.
- Create/rename/delete of notebooks, folders and notes; rename works inline (double-click), via the ⋯ menu, and on the canvas title — two-way synced (sidebar ↔ canvas). Renaming the open note routes through storage (synchronous title write) so autosave cannot revert it.
- Drag-and-drop: note into folder, note reorder, folder reorder/nest — all via fractional order keys (only the moved item's file is rewritten).
- Collapsed-folder state persists across restarts (`~/.local/state/omascratch/view.json`).
- Chrome: no window title bar; full-width top toolbar (reserved for M4 draw tools) matching the sidebar; on-canvas editable page title; fullscreen-canvas toggle (button + F11); F9 toggles sidebar.
- A use-after-free crash (rebuilding the list inside a row widget's own signal) was found and fixed by deferring rebuilds to an idle tick; confirmed stable afterward.
- `cargo test --workspace` — exit 0 (32 passed: core incl. 5 fractional-order tests, ink, store incl. library CRUD + view-state + conflicted-copy + interrupted-write).

## Historical

None.

## Failed

None recorded.

## Not run

Stylus/tablet input (XP-Pen Artist 15.6 Pro axes unverified in-app), ink rendering, persistence, theme adaptation and hot reload, launcher (`.desktop`) start from the app menu (desktop entry not yet installed to a searched path), GUI tests, package build, installation, upgrade, removal, multi-monitor/scaling checks, and any non-development-machine Omarchy acceptance.
