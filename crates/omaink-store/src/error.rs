use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a valid note file: {reason}")]
    Corrupt { path: PathBuf, reason: String },
    #[error("{path} uses schema {found}, newer than this app supports ({supported}); refusing to touch it")]
    NewerSchema { path: PathBuf, found: u32, supported: u32 },
}

impl StoreError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }

    pub fn corrupt(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::Corrupt { path: path.into(), reason: reason.into() }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;
