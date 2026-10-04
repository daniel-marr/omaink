//! Atomic file replacement: same-directory temp file + fsync + rename +
//! parent-directory fsync. A reader never observes a partial file; a crash
//! leaves either the old content or the new content, plus at worst a stale
//! `.omatmp-*` file that `clean_stale_temps` removes.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::error::{Result, StoreError};

pub const TMP_PREFIX: &str = ".omatmp-";

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| StoreError::corrupt(path, "path has no parent directory"))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(TMP_PREFIX)
        .tempfile_in(dir)
        .map_err(|e| StoreError::io(dir, e))?;
    tmp.write_all(bytes).map_err(|e| StoreError::io(path, e))?;
    tmp.as_file().sync_all().map_err(|e| StoreError::io(path, e))?;
    tmp.persist(path).map_err(|e| StoreError::io(path, e.error))?;
    // Durability of the rename itself.
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| StoreError::io(dir, e))?;
    Ok(())
}

/// Remove `.omatmp-*` leftovers older than one hour (a crashed writer's
/// debris). Never touches anything else.
pub fn clean_stale_temps(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let cutoff = SystemTime::now() - Duration::from_secs(3600);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(TMP_PREFIX) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        atomic_write(&p, b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        atomic_write(&p, b"replaced").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"replaced");
    }

    #[test]
    fn interrupted_write_leaves_original_intact() {
        // Simulate a crash between temp-write and rename: the temp file
        // exists, the destination still has the old bytes.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        atomic_write(&p, b"original").unwrap();

        let tmp_path = dir.path().join(format!("{TMP_PREFIX}crashed"));
        std::fs::write(&tmp_path, b"partial new conte").unwrap();

        assert_eq!(std::fs::read(&p).unwrap(), b"original");

        // An old temp is cleaned; a fresh one is left alone.
        let old = SystemTime::now() - Duration::from_secs(7200);
        let f = File::options().write(true).open(&tmp_path).unwrap();
        f.set_modified(old).unwrap();
        drop(f);
        clean_stale_temps(dir.path());
        assert!(!tmp_path.exists());
        assert!(p.exists());
    }
}
