# Architecture worksheet

Status: boundaries decided and scaffolded; implementation begins with the ink vertical slice (M1). Decisions below are design-level until marked implemented.

## Stack and boundaries

**Stack: Rust workspace + GTK4 (gtk4-rs 0.11) + libadwaita (0.9).** Chosen because fluid stylus input is the product's core requirement and GTK4 is a proven Wayland inking path (tablet-v2 → `GtkGestureStylus` with pressure/tilt and uncompressed event history; Rnote demonstrates it in production). Core crates are UI-free so a future mobile shell can reuse them behind FFI. License GPL-3.0-or-later (keeps the option of porting GPL code from Rnote, with attribution in CREDITS.md).

| Responsibility | Lives in | Notes |
| --- | --- | --- |
| Startup | `crates/omascratch-gtk/src/{main,app}.rs` | App identity (`co.think3.OmaScratch`), DI wiring, window creation. No document rules. |
| Application/session | `omascratch-core::session` (M2) | `NoteSession`: open note, command dispatch, undo stacks, dirty tracking, autosave *policy*. Talks to storage via `trait NoteStore`. |
| Domain/core | `omascratch-core` + `omascratch-ink` | Notebook/Folder/Note model, elements, commands, undo; stroke geometry, smoothing, hit-testing, spatial index. No gtk/cairo/std::fs. |
| UI | `omascratch-gtk` | CanvasView widget, panels, docked draw toolbar, GSK rendering, transient drag/selection state. |
| Desktop/storage adapters | `omascratch-store`, `omascratch-theme`, `omascratch-gtk/src/theme_watch.rs` (M5) | Persistence, Omarchy palette input + hot reload, portals. |

Dependency direction: gtk shell → {core, ink, store, theme}; store → {core, ink}; ink → core; theme → core. Never the reverse.

## State and resource owners

- **Authoritative note content** is owned by the `NoteSession` (one open note at a time). The canvas widget holds only transient state (live stroke buffer, drag/lasso state, viewport offset/zoom).
- **Saves**: per-note dirty flag + save generation counter owned by the session. Store I/O runs on `gio::spawn_blocking` workers; a completion carrying a stale generation (note since replaced) is discarded, never applied.
- **Autosave policy**: debounce 2 s after last command, hard cap 20 s, plus on note switch, window close and application shutdown.
- **Theme watcher** (M5) is owned by the application object; it monitors `~/.local/state/omarchy/current/` (parent dir — Omarchy replaces the theme directory inode on switch) and keeps a last-good palette when the input is briefly absent or malformed.
- Replacing the open note: cancel the live stroke, flush or queue a final save for the outgoing note (generation-tagged), then swap the session's document. Old worker completions are rejected by generation.

## Lifecycle

- **Focus loss**: an in-progress stroke is committed as-is (pen leaving proximity is equivalent); drags/lasso in progress are cancelled. Editing continues in background (an editor, not a game — no pause).
- **Modal entry** (file dialogs via portal): canvas input controllers are inert while a modal is up; a queued shortcut must not fire into the canvas after the dialog closes.
- **Close/shutdown order**: cancel input → flush autosave (final synchronous-on-worker save, generation-checked) → join store workers → drop theme watcher → destroy window. Error on final save surfaces a dialog before the window is destroyed, never silent loss.
- **Multiple instances**: GtkApplication D-Bus uniqueness; a second launch activates the existing window.

## Undo, recovery and file conflicts (editor spec)

- Command pattern with per-note undo/redo stacks; the area eraser emits `EraseStrokes { removed, replacements }` so undo is exact.
- On-disk: one note = one zstd-wrapped JSON file (`notes/<uuid7>.omanote`, schema-versioned), atomic write (same-dir temp + fsync + rename + dir fsync). Assets are content-addressed immutable blobs.
- Sync conflicts: the notebook tree is designed for third-party folder sync; no central index file; fractional `order_key` inside each item's own file; sync tools' "conflicted copy" siblings are detected by name pattern and surfaced as notes with a conflict badge.
- Migrations keep a recoverable copy of the original before rewriting a newer schema.
- Crash recovery v1 = autosave window only (≤ debounce); an op-journal in XDG state (never in the synced tree) is a planned later milestone.

## Vertical slice

Open app → draw a pressure-varying stroke with the stylus → close → reopen → the stroke is present and identical. (M1 delivers the drawing; M2 completes the slice with persistence.)

## Verification

See VERIFICATION.md. Core/ink/store/theme logic is unit-tested headless; windowing, stylus and theme behavior are only ever claimed from live Hyprland runs on this machine (Omarchy 4.0.4, XP-Pen Artist 15.6 Pro).
