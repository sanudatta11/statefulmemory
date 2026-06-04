// Generated with AI Coding Rules Hub
//! `<repo>/.memlayer/manifest.json` — append-only chunk registry.
//!
//! Spec §11.2: manifest entries are append-only; the manifest is atomically
//! replaced (`.tmp` + rename) on each append (SC-14, EH-2).

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use crate::error::{Result, SyncError};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub chunk_id: String,
    pub created_at: String, // RFC3339
    pub observations: i64,
    pub sessions: i64,
    pub prompts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub version: u32,
    pub chunks: Vec<ManifestEntry>,
}

impl Manifest {
    pub fn new() -> Self {
        Manifest { version: 1, chunks: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// `<repo>/.memlayer/` directory for a given repo root.
pub fn memlayer_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".memlayer")
}

/// `<repo>/.memlayer/chunks/` directory.
pub fn chunks_dir(repo_root: &Path) -> PathBuf {
    memlayer_dir(repo_root).join("chunks")
}

/// `<repo>/.memlayer/manifest.json` path.
pub fn manifest_path(repo_root: &Path) -> PathBuf {
    memlayer_dir(repo_root).join("manifest.json")
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Read the manifest. Returns an empty `Manifest` if the file does not exist.
#[instrument(level = "debug", skip_all, fields(?repo_root))]
pub async fn read(repo_root: &Path) -> Result<Manifest> {
    let path = manifest_path(repo_root);
    match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| SyncError::ManifestParse {
            path: path.clone(),
            detail: e.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!(?path, "manifest not found, starting fresh");
            Ok(Manifest::new())
        }
        Err(e) => Err(SyncError::io(path, e)),
    }
}

// ---------------------------------------------------------------------------
// Atomic append
// ---------------------------------------------------------------------------

/// Append `entry` to the manifest and atomically replace the file.
///
/// Read → deserialise → push → serialise → write `.tmp` → rename.
/// Idempotent: if a chunk with the same `chunk_id` already exists in the
/// manifest, the entry is not duplicated.
#[instrument(level = "debug", skip_all, fields(?repo_root, chunk_id = %entry.chunk_id))]
pub async fn append_atomic(repo_root: &Path, entry: ManifestEntry) -> Result<()> {
    let dir = memlayer_dir(repo_root);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| SyncError::io(&dir, e))?;

    let mut manifest = read(repo_root).await?;

    // Idempotency: skip if already present.
    if manifest.chunks.iter().any(|c| c.chunk_id == entry.chunk_id) {
        debug!(chunk_id = %entry.chunk_id, "chunk already in manifest, skipping");
        return Ok(());
    }

    manifest.chunks.push(entry);

    let json = serde_json::to_vec_pretty(&manifest)?;
    let path = manifest_path(repo_root);
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, &json)
        .await
        .map_err(|e| SyncError::io(&tmp, e))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| SyncError::io(&path, e))?;
    debug!(?path, chunks = manifest.chunks.len(), "manifest updated");
    Ok(())
}

/// Return a new `ManifestEntry` stamped with the current UTC time.
pub fn new_entry(
    chunk_id: String,
    observations: i64,
    sessions: i64,
    prompts: i64,
) -> ManifestEntry {
    ManifestEntry {
        chunk_id,
        created_at: Utc::now().to_rfc3339(),
        observations,
        sessions,
        prompts,
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_missing_manifest_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let m = read(dir.path()).await.unwrap();
        assert_eq!(m.version, 1);
        assert!(m.chunks.is_empty());
    }

    #[tokio::test]
    async fn append_atomic_creates_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let entry = new_entry("abcd1234efgh5678".into(), 10, 1, 5);
        append_atomic(dir.path(), entry.clone()).await.unwrap();

        let m = read(dir.path()).await.unwrap();
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.chunks[0].chunk_id, "abcd1234efgh5678");
        assert_eq!(m.chunks[0].observations, 10);
    }

    #[tokio::test]
    async fn append_atomic_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let entry = new_entry("abcd1234efgh5678".into(), 10, 1, 5);
        append_atomic(dir.path(), entry.clone()).await.unwrap();
        append_atomic(dir.path(), entry).await.unwrap();

        let m = read(dir.path()).await.unwrap();
        assert_eq!(m.chunks.len(), 1, "duplicate chunk must not be appended");
    }

    #[tokio::test]
    async fn append_atomic_accumulates_entries() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0u8..3 {
            let id = format!("{:016x}", i);
            append_atomic(dir.path(), new_entry(id, 1, 0, 0)).await.unwrap();
        }
        let m = read(dir.path()).await.unwrap();
        assert_eq!(m.chunks.len(), 3);
    }

    #[tokio::test]
    async fn path_helpers_compose() {
        let root = Path::new("/tmp/repo");
        assert_eq!(memlayer_dir(root), Path::new("/tmp/repo/.memlayer"));
        assert_eq!(chunks_dir(root), Path::new("/tmp/repo/.memlayer/chunks"));
        assert_eq!(
            manifest_path(root),
            Path::new("/tmp/repo/.memlayer/manifest.json")
        );
    }
}
