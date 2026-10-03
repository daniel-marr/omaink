//! Typed text boxes on the canvas. Deliberately small rich text: paragraphs
//! (body / bullet / numbered / checkbox) made of styled spans (bold, italic,
//! underline, highlight). One font size per box; color is semantic so text
//! follows the page polarity like ink.

use serde::{Deserialize, Serialize};

use crate::id::TextId;
use crate::SemanticColor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ParaKind {
    Body,
    Bullet,
    Number,
    Check { checked: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Span {
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub highlight: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paragraph {
    pub kind: ParaKind,
    pub spans: Vec<Span>,
}

impl Paragraph {
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    pub id: TextId,
    pub x: f64,
    pub y: f64,
    /// Wrap width in world units.
    pub w: f64,
    /// Last measured height (world units), for hit-testing and culling.
    pub h: f64,
    /// Font size in world units (pixels at 100% zoom).
    pub font_size: f64,
    pub color: SemanticColor,
    pub paras: Vec<Paragraph>,
}

impl TextBox {
    pub fn rect(&self) -> kurbo::Rect {
        kurbo::Rect::new(self.x, self.y, self.x + self.w, self.y + self.h.max(self.font_size))
    }

    /// True when the box holds no visible text (empty boxes are discarded).
    pub fn is_blank(&self) -> bool {
        self.paras.iter().all(|p| p.plain_text().trim().is_empty())
    }

    /// Plain-text rendering (lists as "• ", "1. ", "[ ] " / "[x] ").
    pub fn plain_text(&self) -> String {
        let mut out = Vec::new();
        let mut n = 0;
        for p in &self.paras {
            let prefix = match p.kind {
                ParaKind::Body => {
                    n = 0;
                    String::new()
                }
                ParaKind::Bullet => {
                    n = 0;
                    "• ".to_string()
                }
                ParaKind::Number => {
                    n += 1;
                    format!("{n}. ")
                }
                ParaKind::Check { checked } => {
                    n = 0;
                    if checked { "[x] ".to_string() } else { "[ ] ".to_string() }
                }
            };
            out.push(format!("{prefix}{}", p.plain_text()));
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(kind: ParaKind, text: &str) -> Paragraph {
        Paragraph { kind, spans: vec![Span { text: text.into(), ..Default::default() }] }
    }

    #[test]
    fn plain_text_numbers_restart_after_other_kinds() {
        let tb = TextBox {
            id: TextId::new(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 20.0,
            font_size: 16.0,
            color: SemanticColor::Foreground,
            paras: vec![
                para(ParaKind::Number, "a"),
                para(ParaKind::Number, "b"),
                para(ParaKind::Body, "x"),
                para(ParaKind::Number, "c"),
                para(ParaKind::Check { checked: true }, "done"),
                para(ParaKind::Bullet, "dot"),
            ],
        };
        assert_eq!(tb.plain_text(), "1. a\n2. b\nx\n1. c\n[x] done\n• dot");
        assert!(!tb.is_blank());
    }

    #[test]
    fn serde_round_trip_and_compact_spans() {
        let p = Paragraph {
            kind: ParaKind::Check { checked: false },
            spans: vec![Span { text: "hi".into(), bold: true, ..Default::default() }],
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("italic"), "false flags are omitted: {json}");
        let back: Paragraph = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
