//! Per-note page background: rule/grid lines and a paper-style margin line.
//! Stored in the note file; colors resolve against the active palette so the
//! background adapts to light/dark.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundKind {
    None,
    Rules,
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageBackground {
    pub kind: BackgroundKind,
    /// Line spacing in world units.
    pub spacing: f64,
    /// Paper-style vertical margin line.
    pub margin: bool,
}

impl Default for PageBackground {
    fn default() -> Self {
        Self { kind: BackgroundKind::None, spacing: 32.0, margin: false }
    }
}
