// Generated with AI Coding Rules Hub
//! Chunk building, hashing, compression, and atomic I/O.
//!
//! Spec sections: FR1.1 (items 2-5), FR1.2, §11.2, NFR5, NFR6, SC-1, SC-14, EH-2.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, instrument};

use memlayer_storage::models::{Observation, Prompt, Session};

use crate::error::{Result, SyncError};

// ---------------------------------------------------------------------------
// JSONL line types
// ---------------------------------------------------------------------------

/// One line in a `.jsonl.zst` chunk. The `_kind` field acts as a discriminator
/// so the importer can dispatch without a separate manifest per-line type.
/// (Confirmed in plan decisions §S2.)
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "_kind", rename_all = "snake_case")]
pub enum ChunkLine {
    Observation(Observation),
    Session(Session),
    Prompt(Prompt),
}

// ---------------------------------------------------------------------------
// JSONL builder
// ---------------------------------------------------------------------------

/// Build a deterministic newline-terminated JSONL byte buffer from the three
/// row slices. Row order is: observations (by DB `id` ASC) → sessions → prompts.
///
/// Returns an empty `Vec<u8>` (len == 0) when all three slices are empty; the
/// caller (SyncExport) must check `is_empty()` and skip writing a chunk (FR1.2).
#[instrument(level = "debug", skip_all, fields(obs=%obs.len(), sess=%sess.len(), prompts=%prompts.len()))]
pub fn build_jsonl(
    obs: &[Observation],
    sess: &[Session],
    prompts: &[Prompt],
) -> Result<Vec<u8>> {
    let total = obs.len() + sess.len() + prompts.len();
    let mut buf = Vec::with_capacity(total * 256);
    for o in obs {
        serde_json::to_writer(&mut buf, &ChunkLine::Observation(o.clone()))?;
        buf.push(b'\n');
    }
    for s in sess {
        serde_json::to_writer(&mut buf, &ChunkLine::Session(s.clone()))?;
        buf.push(b'\n');
    }
    for p in prompts {
        serde_json::to_writer(&mut buf, &ChunkLine::Prompt(p.clone()))?;
        buf.push(b'\n');
    }
    debug!(bytes = buf.len(), "build_jsonl complete");
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Chunk ID
// ---------------------------------------------------------------------------

/// Compute `chunk_id` = first 16 hex chars of `sha256(uncompressed_jsonl_bytes)`.
///
/// Spec §11.2: chunk_id = first 16 hex chars of SHA-256 of uncompressed JSONL.
pub fn chunk_id(jsonl_bytes: &[u8]) -> String {
    let hash = Sha256::digest(jsonl_bytes);
    hex::encode(&hash[..8]) // 8 bytes → 16 hex chars
}

// ---------------------------------------------------------------------------
// Compression / decompression  (run in tokio::task::spawn_blocking)
// ---------------------------------------------------------------------------

/// Read `MEMLAYER_ZSTD_LEVEL` env var; default 19.
pub fn zstd_level() -> i32 {
    std::env::var("MEMLAYER_ZSTD_LEVEL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(19)
}

/// Compress `data` with zstd at the configured level.
///
/// Must be called inside `tokio::task::spawn_blocking` — zstd is CPU-bound
/// and would otherwise block the async reactor (NFR6).
pub fn compress_sync(data: &[u8]) -> Result<Vec<u8>> {
    let level = zstd_level();
    zstd::encode_all(data, level)
        .map_err(|e| SyncError::Compress(e.to_string()))
}

/// Decompress a zstd-compressed blob.
///
/// Must be called inside `tokio::task::spawn_blocking`.
pub fn decompress_sync(data: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(data)
        .map_err(|e| SyncError::Decompress(e.to_string()))
}

/// Compress `data` inside a `spawn_blocking` task.
pub async fn compress(data: Vec<u8>) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || compress_sync(&data))
        .await
        .map_err(|e| SyncError::Join(e.to_string()))?
}

/// Decompress `data` inside a `spawn_blocking` task.
pub async fn decompress(data: Vec<u8>) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || decompress_sync(&data))
        .await
        .map_err(|e| SyncError::Join(e.to_string()))?
}

// ---------------------------------------------------------------------------
// Atomic chunk file write
// ---------------------------------------------------------------------------

/// Write `compressed_bytes` to `<chunks_dir>/<id>.jsonl.zst` atomically.
///
/// Algorithm: write to `<id>.tmp` then `rename` to final name (§11.2, SC-14).
/// Never overwrites an existing chunk (chunk immutability invariant §11.2).
/// Returns `Ok(())` if the file already exists (idempotent).
#[instrument(level = "debug", skip(compressed_bytes))]
pub async fn write_chunk_atomic(
    chunks_dir: &Path,
    id: &str,
    compressed_bytes: Vec<u8>,
) -> Result<()> {
    let final_path = chunks_dir.join(format!("{id}.jsonl.zst"));
    if final_path.exists() {
        debug!(?final_path, "chunk already exists, skipping write");
        return Ok(());
    }
    tokio::fs::create_dir_all(chunks_dir)
        .await
        .map_err(|e| SyncError::io(chunks_dir, e))?;

    let tmp_path = chunks_dir.join(format!("{id}.tmp"));
    tokio::fs::write(&tmp_path, &compressed_bytes)
        .await
        .map_err(|e| SyncError::io(&tmp_path, e))?;
    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| SyncError::io(&final_path, e))?;
    debug!(?final_path, "chunk written");
    Ok(())
}

/// Read a chunk file and return the raw compressed bytes.
pub async fn read_chunk(chunks_dir: &Path, id: &str) -> Result<Vec<u8>> {
    let path = chunks_dir.join(format!("{id}.jsonl.zst"));
    tokio::fs::read(&path)
        .await
        .map_err(|e| SyncError::io(&path, e))
}

/// Parse a decompressed JSONL byte buffer into `ChunkLine`s.
///
/// Lines that fail to parse are returned as `Err(SyncError::CorruptChunk)`.
/// The caller (SyncImport) wraps this per-chunk and skips the whole chunk on
/// any corruption (FR2.2).
pub fn parse_jsonl(jsonl_bytes: &[u8]) -> Result<Vec<ChunkLine>> {
    let text = std::str::from_utf8(jsonl_bytes)
        .map_err(|e| SyncError::CorruptChunk(format!("UTF-8 error: {e}")))?;
    let mut lines = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let item: ChunkLine = serde_json::from_str(line)
            .map_err(|e| SyncError::CorruptChunk(format!("line {}: {e}", i + 1)))?;
        lines.push(item);
    }
    Ok(lines)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use memlayer_storage::models::{Observation, Prompt, Session};

    fn sample_obs() -> Observation {
        Observation {
            id: 1,
            sync_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            session_id: "sess-1".into(),
            r#type: "fact".into(),
            title: "hello".into(),
            content: "world".into(),
            tool_name: None,
            scope: "global".into(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 0,
            duplicate_count: 0,
            last_seen_at: None,
            created_at: "2024-01-01T00:00:00+00:00".into(),
            updated_at: "2024-01-01T00:00:00+00:00".into(),
            deleted_at: None,
            review_after: None,
            superseded_ids: vec![],
        }
    }

    fn sample_sess() -> Session {
        Session {
            id: "sess-1".into(),
            directory: "/home/user/proj".into(),
            started_at: "2024-01-01T00:00:00+00:00".into(),
            ended_at: None,
            summary: None,
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt {
            id: 1,
            sync_id: "660e8400-e29b-41d4-a716-446655440001".into(),
            session_id: "sess-1".into(),
            content: "a prompt".into(),
            created_at: "2024-01-01T00:00:00+00:00".into(),
        }
    }

    #[test]
    fn build_jsonl_produces_valid_ndjson() {
        let obs = vec![sample_obs()];
        let sess = vec![sample_sess()];
        let prompts = vec![sample_prompt()];
        let buf = build_jsonl(&obs, &sess, &prompts).unwrap();
        let text = std::str::from_utf8(&buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        // Each line must be valid JSON with a _kind discriminator.
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.get("_kind").is_some(), "missing _kind: {line}");
        }
    }

    #[test]
    fn build_jsonl_empty_produces_empty_buf() {
        let buf = build_jsonl(&[], &[], &[]).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn chunk_id_is_16_hex_chars() {
        let id = chunk_id(b"hello world");
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn chunk_id_is_deterministic() {
        assert_eq!(chunk_id(b"abc"), chunk_id(b"abc"));
        assert_ne!(chunk_id(b"abc"), chunk_id(b"def"));
    }

    #[test]
    fn compress_decompress_roundtrip_sync() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(100);
        let compressed = compress_sync(&data).unwrap();
        assert!(compressed.len() < data.len(), "zstd should compress this");
        let decompressed = decompress_sync(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn parse_jsonl_roundtrips_chunk_lines() {
        let obs = vec![sample_obs()];
        let buf = build_jsonl(&obs, &[], &[]).unwrap();
        let lines = parse_jsonl(&buf).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0], ChunkLine::Observation(_)));
    }

    #[test]
    fn parse_jsonl_corrupt_line_returns_err() {
        let bad = b"not json\n";
        let result = parse_jsonl(bad);
        assert!(matches!(result, Err(SyncError::CorruptChunk(_))));
    }

    #[tokio::test]
    async fn write_chunk_atomic_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let chunks_dir = dir.path().join("chunks");
        let data = b"hello compressed world";
        write_chunk_atomic(&chunks_dir, "abcd1234efgh5678", data.to_vec())
            .await
            .unwrap();
        let path = chunks_dir.join("abcd1234efgh5678.jsonl.zst");
        assert!(path.exists());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), data);
    }

    #[tokio::test]
    async fn write_chunk_atomic_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let chunks_dir = dir.path().join("chunks");
        let id = "abcd1234efgh5678";
        write_chunk_atomic(&chunks_dir, id, b"v1".to_vec()).await.unwrap();
        // Second call with different data should NOT overwrite (immutability).
        write_chunk_atomic(&chunks_dir, id, b"v2".to_vec()).await.unwrap();
        let path = chunks_dir.join(format!("{id}.jsonl.zst"));
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"v1");
    }
}
