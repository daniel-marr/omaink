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

### M4 draw toolset + M5 Omarchy theming — live Hyprland acceptance (2026-10-01/02, user-driven iteration)

All verified interactively by the user on the live session through many build-test rounds:

- OneNote fluid toolbar: pen-preset chips (drawn glyphs with live color tips), click-again flyout (stroke preview, −/dots/+ thickness, Recent Colors, arranged color grid, More Colors dialog, Remove Pen), Add Pen menu, eraser chip with Stroke/Small/Medium/Large flyout; pen set, eraser config, recent colors and shape style persist (`~/.local/state/omascratch/toolbar.json`).
- Area erasers split strokes into fragments (live preview, exact undo); highlighters have flat chisel ends (no start blob).
- Select (click/drag), Lasso (enclose/move/delete), Hand pan tool; selection copy/cut/paste (Ctrl+C/X/V + toolbar buttons), paste offset + preselected.
- Shapes: line/arrow/rect/ellipse as a dedicated `shape` stroke type (constant-width paths; preview == committed), Shift constrains (square/circle/45°), own thickness+color flyout.
- Page backgrounds per note (rules/grid spacing presets + indented margin line at x=90), stored in the note file; canvas invert with neutral-ink luminance adaptation (black↔white track page polarity; colored inks unchanged).
- M5 theming: `~/.local/state/omarchy/current/theme/colors.toml` → chrome CSS + libadwaita variable layer (dialogs/popovers/accent/destructive) + canvas palette + pen-chip tips; parent-dir watcher with debounce + persisted last-good; backdrop contrast pinned (no unfocused fading). Theme crate parses all required/derived keys (unit fixtures incl. partial + garbage files).
- Viewport pinned to a top-left page origin (pan/zoom clamped); zoom indicator with click-to-100%; note switch resets to 100% + home.
- Notebook/folder/note management unified: ⋯ menus, AlertDialog rename/new (popover prompts were killed by menu grab teardown — converted to dialogs), confirm-then-trash deletes; notebook rename relocates the open note's save path; notebook delete recovers to another note without writing into trash. Sub-folders disabled by product choice (UI only).
- Stylus barrel button: click toggles eraser (and back to previous tool); stylus never triggers middle-drag pan. Pending user confirmation of button delivery on the XP-Pen (debug overlay shows b1/b2/b3 masks).
- Desktop: Omarchy's default-opacity window rule (0.96 inactive) identified as the unfocused "dulling"; opt-out rule with `override` added to the USER's `~/.config/hypr/hyprland.lua` (not part of this repo; documented here), validated via `hyprctl reload` + `configerrors`.

### Performance — reproduced benchmark (2026-10-03, release build, this machine)

Harness: `crates/omascratch-store/examples/gen_stress.rs` builds an isolated profile (1 note × 5,000 handwriting-like strokes + 300 notes in 10 folders, 39 MB); `OMASCRATCH_PERF=1 ./target/release/omascratch` (with `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_CACHE_HOME` pointed at the profile) runs a scripted pan, eraser sweep, lasso and select hit-test, logs `[perf]` lines and quits. CPU time of our code only (snapshot construction, hit tests) — GPU rasterization is not measured.

| Measure | Before | After |
|---|---|---|
| Sidebar `rows()` | ~900 ms, 3× at startup | 254 ms cold (header-only decode) · 0.4–0.7 ms warm (mtime/size cache in `~/.cache/omascratch/note-meta.json`) |
| Eraser hit-test | 2.17 ms/sample | 0.006 ms/sample (per-stroke bounds prefilter) |
| Pan frame (snapshot) | 1.66 ms avg, 2.95 max | 0.33 ms avg, 0.98 max (bounds cached per revision, not per frame) |
| Big-note open (read+decode) | 285 ms | 264–285 ms (unchanged — JSON point encoding; follow-up) |

Correctness parity in the same runs: eraser sweep hits (10) and lasso selection (114) identical before/after. Also fixed: sync conflict copies were never listed in the sidebar (filenames don't parse as ids); rows are now keyed by path and conflict copies show with a "(sync conflict)" suffix.

### Image file drag-and-drop — investigated and removed (2026-10-03)

- Diagnostic window-level drop logging showed the user's file manager (Strata, native GTK4) reaches the window with formats `GdkFileList … text/uri-list … application/vnd.portal.filetransfer` but offers **only the MOVE action**. The canvas target deliberately refused MOVE (a completed move lets the source delete the original file), so drops were rejected.
- Drags from Chrome (running under XWayland) never reached the window at all.
- Decision (user): drag-and-drop removed for now; insert-from-file via the toolbar button remains. Revisit if a safe path exists (e.g. COPY-capable sources, or accepting MOVE only after confirming the source keeps the file).

## Historical

None.

## Failed

None recorded.

## Not run

Stylus/tablet input (XP-Pen Artist 15.6 Pro axes unverified in-app), ink rendering, persistence, theme adaptation and hot reload, launcher (`.desktop`) start from the app menu (desktop entry not yet installed to a searched path), GUI tests, package build, installation, upgrade, removal, multi-monitor/scaling checks, and any non-development-machine Omarchy acceptance.
