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

## Historical

None.

## Failed

None recorded.

## Not run

Stylus/tablet input (XP-Pen Artist 15.6 Pro axes unverified in-app), ink rendering, persistence, theme adaptation and hot reload, launcher (`.desktop`) start from the app menu (desktop entry not yet installed to a searched path), GUI tests, package build, installation, upgrade, removal, multi-monitor/scaling checks, and any non-development-machine Omarchy acceptance.
