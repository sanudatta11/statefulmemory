// Generated with AI Coding Rules Hub
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("zstd compress error: {0}")]
    Compress(String),

    #[error("zstd decompress error: {0}")]
    Decompress(String),

    #[error("JSON serialise error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("manifest parse error at {path}: {detail}")]
    ManifestParse { path: PathBuf, detail: String },

    #[error("chunk is corrupt: {0}")]
    CorruptChunk(String),

    #[error("storage error: {0}")]
    Storage(#[from] memlayer_core::Error),

    #[error("task join error: {0}")]
    Join(String),

    #[error("not a memlayer archive (bad magic)")]
    NotArchive,

    #[error("unsupported memlayer archive version {0}")]
    UnsupportedVersion(u16),

    #[error("memlayer archive truncated")]
    Truncated,

    #[error("memlayer archive checksum mismatch")]
    ChecksumMismatch,

    #[error("seed phrase must be at least 12 characters after trim")]
    SeedTooShort,

    #[error("this archive is seed-encrypted and cannot be imported without the seed phrase")]
    SeedRequired,

    #[error("invalid seed phrase or corrupt archive")]
    InvalidSeed,
}

impl SyncError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        SyncError::Io { path: path.into(), source }
    }
}

pub type Result<T> = std::result::Result<T, SyncError>;
