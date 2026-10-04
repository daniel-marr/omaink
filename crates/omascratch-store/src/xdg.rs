//! XDG base-directory split for the app. Per spec, a *relative* XDG env
//! value is invalid and must be ignored in favor of the default.
//!
//! - config  (`~/.config/omascratch`): settings the user may edit/sync
//! - state   (`~/.local/state/omascratch`): window geometry, last note, last-good theme
//! - cache   (`~/.cache/omascratch`): thumbnails, disposable
//! - user documents (default `~/Documents/OmaScratch`): the notebooks root —
//!   user-owned files, never under `.local/share`, preserved on uninstall.

use std::path::{Path, PathBuf};

fn base(env_key: &str, home_rel_default: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    match std::env::var_os(env_key).map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => home.join(home_rel_default),
    }
}

pub fn config_dir() -> PathBuf {
    base("XDG_CONFIG_HOME", ".config").join("omascratch")
}

pub fn state_dir() -> PathBuf {
    base("XDG_STATE_HOME", ".local/state").join("omascratch")
}

pub fn cache_dir() -> PathBuf {
    base("XDG_CACHE_HOME", ".cache").join("omascratch")
}

pub fn default_notebooks_root() -> PathBuf {
    // XDG user dirs would need parsing user-dirs.dirs; Documents is the
    // conventional default and the setting below can override it.
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join("Documents").join("OmaScratch")
}

/// App settings (config dir, TOML, versioned).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub schema: u32,
    /// Where notebooks live. Point a sync client at this folder.
    pub notebooks_root: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self { schema: 1, notebooks_root: default_notebooks_root() }
    }
}

impl Settings {
    pub fn load_or_default(config_dir: &Path) -> Self {
        let path = config_dir.join("settings.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|_| {
                // Malformed settings: fall back to defaults but never
                // overwrite the user's file behind their back.
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// True once the user has picked (or accepted) a notebooks folder:
    /// a settings file exists.
    pub fn exists(config_dir: &Path) -> bool {
        config_dir.join("settings.toml").exists()
    }

    pub fn save(&self, config_dir: &Path) -> crate::error::Result<()> {
        std::fs::create_dir_all(config_dir)
            .map_err(|e| crate::error::StoreError::io(config_dir, e))?;
        let path = config_dir.join("settings.toml");
        let text = toml::to_string_pretty(self)
            .map_err(|e| crate::error::StoreError::corrupt(&path, e.to_string()))?;
        crate::atomic::atomic_write(&path, text.as_bytes())
    }
}

/// Per-viewer UI state (collapsed folders, …). Lives in the XDG state dir,
/// never in the synced notebooks tree.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ViewState {
    #[serde(default)]
    pub collapsed_folders: Vec<uuid::Uuid>,
}

impl ViewState {
    pub fn load(state_dir: &Path) -> Self {
        std::fs::read_to_string(state_dir.join("view.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state_dir: &Path) -> crate::error::Result<()> {
        std::fs::create_dir_all(state_dir)
            .map_err(|e| crate::error::StoreError::io(state_dir, e))?;
        let path = state_dir.join("view.json");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| crate::error::StoreError::corrupt(&path, e.to_string()))?;
        crate::atomic::atomic_write(&path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::now_v7();
        let vs = ViewState { collapsed_folders: vec![id] };
        vs.save(dir.path()).unwrap();
        let back = ViewState::load(dir.path());
        assert_eq!(back.collapsed_folders, vec![id]);
        // Missing file → default (empty), not an error.
        let empty = ViewState::load(tempfile::tempdir().unwrap().path());
        assert!(empty.collapsed_folders.is_empty());
    }

    #[test]
    fn settings_roundtrip_and_malformed_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.notebooks_root = PathBuf::from("/tmp/nb");
        s.save(dir.path()).unwrap();
        let back = Settings::load_or_default(dir.path());
        assert_eq!(back.notebooks_root, PathBuf::from("/tmp/nb"));

        std::fs::write(dir.path().join("settings.toml"), "not = [valid").unwrap();
        let fallback = Settings::load_or_default(dir.path());
        assert_eq!(fallback.notebooks_root, default_notebooks_root());
    }
}

/// Check a folder chosen as the notebooks root: it must be writable and must
/// not be one of the app's own config/state/cache folders, or inside the
/// `current` notebooks root (a notebook folder is not a root).
pub fn validate_notebooks_root(path: &Path, current: Option<&Path>) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Choose a full folder path.".into());
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let target = canon(path);
    for (dir, what) in [(config_dir(), "settings"), (state_dir(), "app state"), (cache_dir(), "cache")] {
        if target.starts_with(canon(&dir)) {
            return Err(format!("That folder is inside OmaScratch's {what} folder; choose somewhere else."));
        }
    }
    if let Some(cur) = current {
        let cur = canon(cur);
        if target != cur && target.starts_with(&cur) {
            return Err("That folder is inside your current notebooks folder; choose the folder that holds your notebooks.".into());
        }
    }
    std::fs::create_dir_all(&target).map_err(|e| format!("Can't create that folder: {e}"))?;
    let probe = target.join(".omascratch-write-test");
    std::fs::write(&probe, b"ok").map_err(|e| format!("Can't write to that folder: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

#[cfg(test)]
mod root_tests {
    use super::*;

    #[test]
    fn notebooks_root_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Notes");
        assert!(validate_notebooks_root(&root, None).is_ok(), "creatable, writable folder");
        assert!(root.is_dir() && !root.join(".omascratch-write-test").exists(), "probe cleaned up");
        let inner = root.join("My Notebook");
        assert!(validate_notebooks_root(&inner, Some(&root)).is_err(), "inside current root");
        assert!(validate_notebooks_root(&root, Some(&root)).is_ok(), "same folder is fine");
        assert!(validate_notebooks_root(Path::new("relative/dir"), None).is_err());
        assert!(validate_notebooks_root(&config_dir().join("x"), None).is_err(), "app config folder");
    }
}
