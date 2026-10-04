# Credits

**OmaInk** is created by **Daniel Marr · [think3.co](https://think3.co)**.
Application licence: **GPL-3.0-or-later** (see [LICENSE](LICENSE)).

## Artwork

- **App icon / logo** (`packaging/logo/OmaInk.svg`, installed as
  `co.think3.OmaInk.svg`): by Daniel Marr, incorporating the OMA logo.
  The OMA logo is a trademark of the Omarchy Foundation. OmaInk is an
  independent community project, not affiliated with or endorsed by Omarchy.
- **Toolbar and sidebar glyphs**: drawn in code (cairo). The pan (hand), copy,
  paste (clipboard) and help (circle-help) glyphs are traced from
  [Lucide](https://lucide.dev) icons — ISC License, Copyright (c) Lucide
  Contributors. All other glyphs (pens, highlighter, eraser, lasso, insert
  space, settings gear, text, shapes, page) are original to this project.
- **Community App badge** in the README:
  [tcballard/omarchy-badges](https://github.com/tcballard/omarchy-badges)
  (linked remotely, not bundled). It is an identity label, not official
  approval. The Omarchy name and logo belong to their owners.

## Code and algorithms

- Stroke outlines for highlighters use the **perfect-freehand** algorithm by
  Steve Ruiz (MIT) via the Rust port [`freedraw`](https://crates.io/crates/freedraw)
  (MIT). Pen and pencil strokes use OmaInk's own renderer.
- [Rnote](https://github.com/flxzt/rnote) (GPL-3.0-or-later) was studied as a
  reference for GTK4 stylus handling. **No Rnote code is included.**
- Built on [gtk4-rs](https://gtk-rs.org) and libadwaita-rs (MIT) against the
  system GTK 4 and libadwaita libraries (LGPL-2.1-or-later, dynamically
  linked).
- All other Rust dependencies (see `Cargo.lock`) are under permissive
  licences (MIT, Apache-2.0, BSD-3-Clause, Unicode-3.0, Unlicense). Binary
  releases must ship their notices; this release is source-only.
