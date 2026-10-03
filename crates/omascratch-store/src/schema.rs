//! On-disk serde structs, kept separate from the domain model with explicit
//! `from_domain`/`into_domain` mapping so schema evolution never contorts
//! the domain types.
//!
//! Elements are a tagged enum (`"type": "stroke" | ...`) so images, shapes
//! and text boxes can be added later without a schema bump; an unknown
//! element type in a *same-schema* file is preserved as opaque JSON rather
//! than dropped (forward-compatible round-trip).

use serde::{Deserialize, Serialize};

use omascratch_core::{FolderId, InkPoint, NoteContent, NoteId, PageBackground, Rgba, SemanticColor, Stroke, StrokeId, Tool};

pub const NOTE_SCHEMA: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct NoteFileV1 {
    pub schema: u32,
    pub id: NoteId,
    pub title: String,
    pub folder: Option<FolderId>,
    pub order_key: String,
    pub created_ms: u64,
    pub modified_ms: u64,
    #[serde(default)]
    pub background: PageBackground,
    pub elements: Vec<DiskElement>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiskElement {
    Stroke(DiskStroke),
    /// Images reference an immutable asset file in `<note-id>.assets/`.
    Image(omascratch_core::ImageItem),
    /// Typed text box (paragraphs of styled spans).
    Text(omascratch_core::TextBox),
    /// Element kinds this build doesn't know. Kept verbatim and written back.
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiskStroke {
    pub id: StrokeId,
    pub tool: Tool,
    pub color: DiskColor,
    pub width: f64,
    pub t0_ms: u64,
    /// Compact samples: [x, y, pressure, tilt_x, tilt_y, dt_ms].
    pub pts: Vec<(f64, f64, f32, f32, f32, u16)>,
}

/// Semantic color on disk: "fg", "accent", or "#rrggbbaa".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiskColor {
    Named(String),
}

impl From<SemanticColor> for DiskColor {
    fn from(c: SemanticColor) -> Self {
        DiskColor::Named(match c {
            SemanticColor::Foreground => "fg".to_string(),
            SemanticColor::Accent => "accent".to_string(),
            SemanticColor::Fixed(Rgba { r, g, b, a }) => format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                (r.clamp(0.0, 1.0) * 255.0).round() as u8,
                (g.clamp(0.0, 1.0) * 255.0).round() as u8,
                (b.clamp(0.0, 1.0) * 255.0).round() as u8,
                (a.clamp(0.0, 1.0) * 255.0).round() as u8,
            ),
        })
    }
}

impl From<DiskColor> for SemanticColor {
    fn from(c: DiskColor) -> Self {
        let DiskColor::Named(s) = c;
        match s.as_str() {
            "fg" => SemanticColor::Foreground,
            "accent" => SemanticColor::Accent,
            hex => parse_hex(hex).unwrap_or(SemanticColor::Foreground),
        }
    }
}

fn parse_hex(s: &str) -> Option<SemanticColor> {
    let s = s.strip_prefix('#')?;
    if s.len() != 8 {
        return None;
    }
    let b = u32::from_str_radix(s, 16).ok()?;
    Some(SemanticColor::Fixed(Rgba {
        r: ((b >> 24) & 0xff) as f32 / 255.0,
        g: ((b >> 16) & 0xff) as f32 / 255.0,
        b: ((b >> 8) & 0xff) as f32 / 255.0,
        a: (b & 0xff) as f32 / 255.0,
    }))
}

impl From<&Stroke> for DiskStroke {
    fn from(s: &Stroke) -> Self {
        DiskStroke {
            id: s.id,
            tool: s.tool,
            color: s.color.into(),
            width: s.width,
            t0_ms: s.t0_ms,
            pts: s
                .points
                .iter()
                .map(|p| (p.x, p.y, p.pressure, p.tilt_x, p.tilt_y, p.dt_ms))
                .collect(),
        }
    }
}

impl From<DiskStroke> for Stroke {
    fn from(d: DiskStroke) -> Self {
        Stroke {
            id: d.id,
            tool: d.tool,
            color: d.color.into(),
            width: d.width,
            t0_ms: d.t0_ms,
            points: d
                .pts
                .into_iter()
                .map(|(x, y, pressure, tilt_x, tilt_y, dt_ms)| InkPoint {
                    x,
                    y,
                    pressure,
                    tilt_x,
                    tilt_y,
                    dt_ms,
                })
                .collect(),
        }
    }
}

/// Metadata + content of a note, as the app works with it.
#[derive(Debug, Clone)]
pub struct NoteDoc {
    pub id: NoteId,
    pub title: String,
    pub folder: Option<FolderId>,
    pub order_key: String,
    pub created_ms: u64,
    pub modified_ms: u64,
    pub background: PageBackground,
    pub content: NoteContent,
    /// Unknown elements carried through untouched.
    pub opaque_elements: Vec<serde_json::Value>,
}

impl NoteDoc {
    pub fn to_disk(&self, modified_ms: u64) -> NoteFileV1 {
        let mut elements: Vec<DiskElement> = self
            .content
            .strokes
            .iter()
            .map(|s| DiskElement::Stroke(s.into()))
            .collect();
        elements.extend(self.content.images.iter().cloned().map(DiskElement::Image));
        elements.extend(self.content.texts.iter().cloned().map(DiskElement::Text));
        elements.extend(self.opaque_elements.iter().cloned().map(DiskElement::Unknown));
        NoteFileV1 {
            schema: NOTE_SCHEMA,
            id: self.id,
            title: self.title.clone(),
            folder: self.folder,
            order_key: self.order_key.clone(),
            created_ms: self.created_ms,
            modified_ms,
            background: self.background,
            elements,
        }
    }

    pub fn from_disk(f: NoteFileV1) -> Self {
        let mut content = NoteContent::default();
        let mut opaque = Vec::new();
        for el in f.elements {
            match el {
                DiskElement::Stroke(s) => content.strokes.push(s.into()),
                DiskElement::Image(i) => content.images.push(i),
                DiskElement::Text(t) => content.texts.push(t),
                DiskElement::Unknown(v) => opaque.push(v),
            }
        }
        NoteDoc {
            id: f.id,
            title: f.title,
            folder: f.folder,
            order_key: f.order_key,
            created_ms: f.created_ms,
            modified_ms: f.modified_ms,
            background: f.background,
            content,
            opaque_elements: opaque,
        }
    }
}
