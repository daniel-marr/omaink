//! Pure theme logic: parse an Omarchy `colors.toml` into a `Palette`, resolve
//! `SemanticColor`s, and generate the GTK CSS override string. File watching
//! lives in the GTK shell (`theme_watch.rs`), not here.

/// Omarchy 4.x active-theme colors file, relative to the state dir.
pub const OMARCHY_COLORS_RELPATH: &str = "omarchy/current/theme/colors.toml";
