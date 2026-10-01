//! Semantic colors: ink stores *meaning*, the renderer resolves against the
//! active Omarchy palette, so a light/dark theme flip keeps ink readable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticColor {
    /// Follows the theme foreground (default pen color).
    Foreground,
    /// Follows the theme accent.
    Accent,
    /// A fixed user-chosen color, stored as-is.
    Fixed(Rgba),
}

impl Default for SemanticColor {
    fn default() -> Self {
        Self::Foreground
    }
}
