//! Pure theme logic: parse an Omarchy `colors.toml` into a `Palette`, and
//! generate the app's GTK CSS from it. File watching and GTK application
//! live in the shell (`theme.rs`), not here.

use omascratch_core::Rgba;
use serde::Deserialize;

/// Omarchy 4.x active-theme colors file, relative to `~/.local/state/`.
pub const OMARCHY_COLORS_RELPATH: &str = "omarchy/current/theme/colors.toml";
/// Pre-4.x location, relative to `~/.config/`.
pub const OMARCHY_LEGACY_RELPATH: &str = "omarchy/current/theme/colors.toml";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub accent: Rgba,
    pub selection: Rgba,
    pub muted: Rgba,
    pub background: Rgba,
    pub dark_background: Rgba,
    pub darker_background: Rgba,
    pub lighter_background: Rgba,
    pub foreground: Rgba,
    pub dark_foreground: Rgba,
    pub light_foreground: Rgba,
    pub bright_foreground: Rgba,
    pub red: Rgba,
}

#[derive(Deserialize)]
struct RawColors {
    mode: Option<String>,
    accent: Option<String>,
    selection: Option<String>,
    muted: Option<String>,
    background: Option<String>,
    dark_background: Option<String>,
    darker_background: Option<String>,
    lighter_background: Option<String>,
    foreground: Option<String>,
    dark_foreground: Option<String>,
    light_foreground: Option<String>,
    bright_foreground: Option<String>,
    red: Option<String>,
}

pub fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim().strip_prefix('#')?;
    let (r, g, b, a) = match s.len() {
        6 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            ((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff, 255)
        }
        8 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            ((v >> 24) & 0xff, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff)
        }
        _ => return None,
    };
    Some(Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    })
}

pub fn to_hex(c: Rgba) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8
    )
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

impl Palette {
    /// Tokyo Night, matching the colors the app shipped with before theming.
    pub fn fallback() -> Palette {
        let hx = |s| parse_hex(s).unwrap();
        Palette {
            dark: true,
            accent: hx("#7aa2f7"),
            selection: hx("#292e42"),
            muted: hx("#414868"),
            background: hx("#1a1b26"),
            dark_background: hx("#13141c"),
            darker_background: hx("#0e0e14"),
            lighter_background: hx("#24283b"),
            foreground: hx("#a9b1d6"),
            dark_foreground: hx("#565f89"),
            light_foreground: hx("#b4bee6"),
            bright_foreground: hx("#c0caf5"),
            red: hx("#f7768e"),
        }
    }

    /// Parse a `colors.toml`. `background`, `foreground` and `accent` are
    /// required; everything else derives from them when missing, so partial
    /// user themes still work.
    pub fn parse(text: &str) -> Option<Palette> {
        let raw: RawColors = toml::from_str(text).ok()?;
        let h = |o: &Option<String>| o.as_deref().and_then(parse_hex);
        let background = h(&raw.background)?;
        let foreground = h(&raw.foreground)?;
        let accent = h(&raw.accent)?;
        let dark = raw.mode.as_deref().map(|m| m != "light").unwrap_or(true);
        let black = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        let white = Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
        let toward = if dark { white } else { black };
        let away = if dark { black } else { white };
        Some(Palette {
            dark,
            accent,
            selection: h(&raw.selection).unwrap_or(mix(background, accent, 0.22)),
            muted: h(&raw.muted).unwrap_or(mix(background, foreground, 0.35)),
            background,
            dark_background: h(&raw.dark_background).unwrap_or(mix(background, away, 0.25)),
            darker_background: h(&raw.darker_background).unwrap_or(mix(background, away, 0.45)),
            lighter_background: h(&raw.lighter_background).unwrap_or(mix(background, toward, 0.08)),
            foreground,
            dark_foreground: h(&raw.dark_foreground).unwrap_or(mix(foreground, background, 0.45)),
            light_foreground: h(&raw.light_foreground).unwrap_or(mix(foreground, toward, 0.2)),
            bright_foreground: h(&raw.bright_foreground).unwrap_or(mix(foreground, toward, 0.35)),
            red: h(&raw.red).unwrap_or(parse_hex("#f7768e").unwrap()),
        })
    }

    /// The app chrome CSS, templated from this palette.
    pub fn app_css(&self) -> String {
        let bg = to_hex(self.background);
        let dark_bg = to_hex(self.dark_background);
        let sel = to_hex(self.selection);
        let muted = to_hex(self.muted);
        let fg = to_hex(self.foreground);
        let dark_fg = to_hex(self.dark_foreground);
        let light_fg = to_hex(self.light_foreground);
        let accent = to_hex(self.accent);
        let lighter_bg = to_hex(self.lighter_background);
        let red = to_hex(self.red);
        format!(
            r#"
/* Libadwaita variable layer: themes every Adwaita surface the app uses —
   alert dialogs, popovers, buttons — from the Omarchy palette. */
:root {{
    --window-bg-color: {bg};
    --window-fg-color: {fg};
    --view-bg-color: {bg};
    --view-fg-color: {fg};
    --dialog-bg-color: {lighter_bg};
    --dialog-fg-color: {fg};
    --popover-bg-color: {lighter_bg};
    --popover-fg-color: {fg};
    --headerbar-bg-color: {dark_bg};
    --headerbar-fg-color: {fg};
    --sidebar-bg-color: {dark_bg};
    --sidebar-fg-color: {fg};
    --card-bg-color: {lighter_bg};
    --card-fg-color: {fg};
    --accent-bg-color: {accent};
    --accent-fg-color: {bg};
    --accent-color: {accent};
    --destructive-bg-color: {red};
    --destructive-fg-color: {bg};
    --destructive-color: {red};
}}

window {{
    background-color: {bg};
    color: {fg};
}}
.sidebar-pane {{
    background-color: {dark_bg};
}}
.sidebar-header .heading {{
    font-size: 0.85rem;
    color: {dark_fg};
}}
.navigation-sidebar {{
    background-color: {dark_bg};
    padding: 4px 8px;
}}
.navigation-sidebar row {{
    min-height: 24px;
    border-radius: 4px;
    margin: 0;
    padding: 0;
}}
.navigation-sidebar row:selected {{
    background-color: {sel};
    color: {fg};
}}
.row-title {{
    font-size: 0.875rem;
    font-weight: 400;
}}
.folder-title {{
    font-weight: 500;
    color: {light_fg};
}}
.folder-chevron {{
    color: {dark_fg};
}}
.header-icon {{
    min-height: 24px;
    min-width: 24px;
    padding: 2px;
}}
.header-icon image {{
    -gtk-icon-size: 15px;
}}
.indent-line {{
    background-color: {muted};
}}
.notebook-switcher {{
    color: {fg};
    font-size: 0.9rem;
}}
.rename-entry {{
    min-height: 22px;
    padding: 0 4px;
    border-radius: 4px;
}}
.main-toolbar {{
    background-color: {dark_bg};
    border-bottom: 1px solid {sel};
    min-height: 58px;
}}
.draw-toolbar button {{
    min-height: 28px;
    min-width: 28px;
    padding: 2px;
}}
/* Near-square hover/press backgrounds on every toolbar button
   (pens, tools, menu buttons, zoom, fullscreen). */
.main-toolbar button {{
    border-radius: 2px;
}}
.pen-chip {{
    padding: 1px 2px;
    margin: 0;
}}
.pen-chip.pen-active {{
    background: none;
    border-bottom: 3px solid {accent};
    border-radius: 0;
}}
.pen-chip.pen-armed {{
    opacity: 0.55;
}}
.pen-chip.drop-before {{
    box-shadow: inset 3px 0 0 {accent};
}}
.pen-chip.drop-after {{
    box-shadow: inset -3px 0 0 {accent};
}}
.mode-active {{
    background: none;
    border-bottom: 3px solid {accent};
    border-radius: 0;
}}
.zoom-indicator {{
    font-size: 0.85rem;
    min-width: 52px;
    color: {dark_fg};
}}
.swatch-btn {{
    min-height: 0;
    min-width: 0;
    padding: 2px;
}}
/* The chosen option in a flyout (color, list type, style). */
.option-selected {{
    box-shadow: inset 0 0 0 2px {accent};
    border-radius: 5px;
}}

/* Keep full contrast when the window is unfocused: notes stay readable on a
   second monitor during calls. Overrides libadwaita's backdrop dimming. */
window:backdrop,
window:backdrop * {{
    color: {fg};
}}
window:backdrop .dim-label,
window:backdrop .heading,
window:backdrop .folder-chevron,
window:backdrop .sidebar-header .heading {{
    color: {dark_fg};
}}
window:backdrop .folder-title {{
    color: {light_fg};
}}
window:backdrop .navigation-sidebar row:selected {{
    background-color: {sel};
}}
window:backdrop .row-menu {{
    opacity: 0;
}}
window:backdrop .navigation-sidebar row:hover .row-menu,
window:backdrop .navigation-sidebar row:selected .row-menu {{
    opacity: 0.7;
}}
"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_omarchy_colors_file() {
        let text = r##"
mode = "dark"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"
red = "#f7768e"
"##;
        let p = Palette::parse(text).unwrap();
        assert!(p.dark);
        assert_eq!(to_hex(p.accent), "#7aa2f7");
        assert_eq!(to_hex(p.bright_foreground), "#c0caf5");
        assert!(p.app_css().contains("#13141c"));
    }

    #[test]
    fn partial_theme_derives_missing_colors() {
        let text = r##"
mode = "light"
background = "#f2f0e9"
foreground = "#100f0f"
accent = "#205ea6"
"##;
        let p = Palette::parse(text).unwrap();
        assert!(!p.dark);
        // Derived values exist and differ sensibly from the base.
        assert_ne!(to_hex(p.dark_background), to_hex(p.background));
        assert_ne!(to_hex(p.bright_foreground), to_hex(p.background));
    }

    #[test]
    fn garbage_is_rejected_not_panicked() {
        assert!(Palette::parse("not toml at all [").is_none());
        assert!(Palette::parse("mode = \"dark\"").is_none(), "missing required keys");
        assert!(parse_hex("#zzz").is_none());
    }
}
