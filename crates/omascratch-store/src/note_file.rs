//! Reading and writing `.omanote` files: a zstd frame wrapping JSON.
//! Missing, corrupt and newer-schema files produce typed errors; nothing is
//! ever silently discarded or overwritten on read.

use std::path::Path;

use crate::atomic::atomic_write;
use crate::error::{Result, StoreError};
use crate::schema::{NoteDoc, NoteFileV1, NOTE_SCHEMA};

pub const NOTE_EXT: &str = "omanote";
const ZSTD_LEVEL: i32 = 3;

pub fn write_note(path: &Path, doc: &NoteDoc, modified_ms: u64) -> Result<()> {
    let file = doc.to_disk(modified_ms);
    let json = serde_json::to_vec(&file)
        .map_err(|e| StoreError::corrupt(path, format!("serialize: {e}")))?;
    let compressed = zstd::encode_all(json.as_slice(), ZSTD_LEVEL)
        .map_err(|e| StoreError::io(path, e))?;
    atomic_write(path, &compressed)
}

/// Directory holding a note's image assets: `notes/<uuid>.assets/`.
pub fn assets_dir(note_path: &Path) -> std::path::PathBuf {
    let stem = note_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    note_path.with_file_name(format!("{stem}.assets"))
}

/// Write an image asset beside the note. Names are unique (uuid v7) and the
/// file is never rewritten, so syncing machines can't conflict on assets and
/// undo of a deleted image always finds its file. Returns the asset name.
pub fn write_asset(note_path: &Path, bytes: &[u8], ext: &str) -> Result<String> {
    let dir = assets_dir(note_path);
    std::fs::create_dir_all(&dir).map_err(|e| StoreError::io(&dir, e))?;
    let name = format!("img-{}.{ext}", uuid::Uuid::now_v7());
    atomic_write(&dir.join(&name), bytes)?;
    Ok(name)
}

/// Just the sidebar-relevant header of a note. Element payloads are skipped
/// (`IgnoredAny`), so no stroke data is allocated.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoteMeta {
    pub id: omascratch_core::NoteId,
    pub title: String,
    pub folder: Option<omascratch_core::FolderId>,
    pub order_key: String,
}

pub fn read_note_meta(path: &Path) -> Result<NoteMeta> {
    #[derive(serde::Deserialize)]
    struct Header {
        schema: u32,
        id: omascratch_core::NoteId,
        title: String,
        folder: Option<omascratch_core::FolderId>,
        order_key: String,
        #[allow(dead_code)]
        #[serde(default)]
        elements: serde::de::IgnoredAny,
    }
    let bytes = std::fs::read(path).map_err(|e| StoreError::io(path, e))?;
    let json = zstd::decode_all(bytes.as_slice())
        .map_err(|e| StoreError::corrupt(path, format!("zstd: {e}")))?;
    let h: Header = serde_json::from_slice(&json)
        .map_err(|e| StoreError::corrupt(path, format!("json: {e}")))?;
    if h.schema > NOTE_SCHEMA {
        return Err(StoreError::NewerSchema { path: path.to_path_buf(), found: h.schema, supported: NOTE_SCHEMA });
    }
    Ok(NoteMeta { id: h.id, title: h.title, folder: h.folder, order_key: h.order_key })
}

pub fn read_note(path: &Path) -> Result<NoteDoc> {
    let bytes = std::fs::read(path).map_err(|e| StoreError::io(path, e))?;
    let json = zstd::decode_all(bytes.as_slice())
        .map_err(|e| StoreError::corrupt(path, format!("zstd: {e}")))?;

    // Check the schema number before committing to the full struct shape.
    #[derive(serde::Deserialize)]
    struct SchemaProbe {
        schema: u32,
    }
    let probe: SchemaProbe = serde_json::from_slice(&json)
        .map_err(|e| StoreError::corrupt(path, format!("json: {e}")))?;
    if probe.schema > NOTE_SCHEMA {
        return Err(StoreError::NewerSchema {
            path: path.to_path_buf(),
            found: probe.schema,
            supported: NOTE_SCHEMA,
        });
    }
    let file: NoteFileV1 = serde_json::from_slice(&json)
        .map_err(|e| StoreError::corrupt(path, format!("schema {}: {e}", probe.schema)))?;
    Ok(NoteDoc::from_disk(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use omascratch_core::{InkPoint, NoteContent, NoteId, SemanticColor, Stroke, StrokeId, Tool};

    fn doc() -> NoteDoc {
        NoteDoc {
            id: NoteId::new(),
            title: "Test note".into(),
            folder: None,
            order_key: "a0".into(),
            created_ms: 1_000,
            modified_ms: 1_000,
            background: Default::default(),
            content: NoteContent {
                strokes: vec![Stroke {
                    id: StrokeId::new(),
                    tool: Tool::Pen,
                    color: SemanticColor::Foreground,
                    width: 3.5,
                    t0_ms: 42,
                    points: vec![
                        InkPoint { x: 1.0, y: 2.0, pressure: 0.4, tilt_x: 0.1, tilt_y: -0.1, dt_ms: 0 },
                        InkPoint { x: 3.0, y: 4.0, pressure: 0.9, tilt_x: 0.0, tilt_y: 0.0, dt_ms: 7 },
                    ],
                }],
                images: vec![],
            },
            opaque_elements: vec![],
        }
    }

    #[test]
    fn write_read_roundtrip_preserves_everything() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("n.omanote");
        let d = doc();
        write_note(&p, &d, 2_000).unwrap();
        let back = read_note(&p).unwrap();
        assert_eq!(back.id, d.id);
        assert_eq!(back.title, d.title);
        assert_eq!(back.modified_ms, 2_000);
        assert_eq!(back.content.strokes, d.content.strokes);
    }

    #[test]
    fn corrupt_file_is_a_typed_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("broken.omanote");
        std::fs::write(&p, b"this is not zstd").unwrap();
        assert!(matches!(read_note(&p), Err(StoreError::Corrupt { .. })));

        // Valid zstd, invalid JSON inside.
        let garbage = zstd::encode_all(&b"not json"[..], 3).unwrap();
        std::fs::write(&p, garbage).unwrap();
        assert!(matches!(read_note(&p), Err(StoreError::Corrupt { .. })));
    }

    #[test]
    fn newer_schema_is_refused_without_data_loss() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("future.omanote");
        let payload = serde_json::json!({ "schema": 99, "from": "the future" });
        let bytes = zstd::encode_all(serde_json::to_vec(&payload).unwrap().as_slice(), 3).unwrap();
        std::fs::write(&p, &bytes).unwrap();
        assert!(matches!(
            read_note(&p),
            Err(StoreError::NewerSchema { found: 99, .. })
        ));
        // The file is untouched by the failed read.
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
    }

    #[test]
    fn meta_read_matches_full_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("n.omanote");
        let d = doc();
        write_note(&p, &d, 1).unwrap();
        let m = read_note_meta(&p).unwrap();
        assert_eq!((m.id, m.title.as_str(), m.folder, m.order_key.as_str()), (d.id, "Test note", None, "a0"));
    }

    #[test]
    fn images_round_trip_and_assets_live_beside_the_note() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("n.omanote");
        let mut d = doc();
        let name = write_asset(&p, b"\x89PNG fake", "png").unwrap();
        assert!(assets_dir(&p).join(&name).exists());
        assert!(assets_dir(&p).ends_with("n.assets"));
        d.content.images.push(omascratch_core::ImageItem {
            id: omascratch_core::ImageId::new(),
            asset: name.clone(),
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 200.0,
            pinned: true,
        });
        write_note(&p, &d, 5).unwrap();
        let back = read_note(&p).unwrap();
        assert_eq!(back.content.images, d.content.images);
        assert!(back.opaque_elements.is_empty(), "images are known elements, not opaque");
    }

    #[test]
    fn unknown_elements_round_trip_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("n.omanote");
        let mut d = doc();
        d.opaque_elements
            .push(serde_json::json!({ "type": "hologram", "data": [1, 2, 3] }));
        write_note(&p, &d, 1).unwrap();
        let back = read_note(&p).unwrap();
        assert_eq!(back.opaque_elements, d.opaque_elements);
        // Write again and make sure it survives a second cycle too.
        write_note(&p, &back, 2).unwrap();
        let back2 = read_note(&p).unwrap();
        assert_eq!(back2.opaque_elements, d.opaque_elements);
    }
}
