// Generated with AI Coding Rules Hub
//! SQLite-backed embedding cache, sha256-keyed, with fingerprint collision check.
//!
//! Schema (per plan §1.4):
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS cache (
//!     key         TEXT PRIMARY KEY,        -- sha256 hex of source text
//!     fingerprint TEXT NOT NULL,           -- first 80 chars of source text
//!     payload     BLOB NOT NULL,           -- f32 vector as little-endian bytes
//!     created_at  TEXT NOT NULL DEFAULT (datetime('now'))
//! );
//! ```
//!
//! Vectors are stored as little-endian f32 byte BLOBs. EH-7 (collision detection):
//! on `get_many`, the stored `fingerprint` is compared against the live
//! `fingerprint(text)`; mismatch → treat as miss.
//!
//! See spec retrieval-upgrade-v1 §3 (TS-3, EH-7, SC-8).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

/// Filename used inside the data dir.
pub const CACHE_FILENAME: &str = "embeddings.cache.sqlite";

/// Number of leading characters of source text retained as the collision-check
/// fingerprint. 80 chars is plenty to disambiguate sha256 collisions in practice
/// while staying short for storage.
pub const FINGERPRINT_CHARS: usize = 80;

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS cache (
    key         TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    payload     BLOB NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);";

/// SQLite-backed embedding cache.
///
/// Cheaply cloneable: the inner connection is shared behind an `Arc<Mutex>`.
pub struct EmbeddingCache {
    conn: Arc<Mutex<Connection>>,
}

impl EmbeddingCache {
    /// Open (creating if missing) the cache DB at `<data_dir>/embeddings.cache.sqlite`.
    ///
    /// The parent directory must already exist; callers (the daemon) own data
    /// directory creation so this function does not `mkdir -p`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(CACHE_FILENAME);
        let conn = Connection::open(&path)
            .with_context(|| format!("open embedding cache at {}", path.display()))?;
        // Reasonable defaults for a single-writer cache; embedding workloads
        // are tiny relative to the main store.
        // journal_mode returns the new mode as a row, so use `pragma()` not
        // `pragma_update()` (which assumes the setter is silent).
        conn.pragma(None, "journal_mode", "WAL", |_| Ok(()))
            .context("set journal_mode=WAL")?;
        conn.pragma(None, "synchronous", "NORMAL", |_| Ok(()))
            .context("set synchronous=NORMAL")?;
        conn.execute(SCHEMA, [])
            .context("create cache table if missing")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Returns `(hits, miss_indices)`.
    ///
    /// * `hits[i]` is `Some(vec)` if `texts[i]` was cached and its fingerprint
    ///   matches; otherwise `None`.
    /// * `miss_indices` lists, in input order, every `i` where `hits[i]` is `None`.
    #[allow(clippy::type_complexity)]
    pub fn get_many(&self, texts: &[&str]) -> Result<(Vec<Option<Vec<f32>>>, Vec<usize>)> {
        let mut hits: Vec<Option<Vec<f32>>> = Vec::with_capacity(texts.len());
        let mut miss_indices: Vec<usize> = Vec::new();

        if texts.is_empty() {
            return Ok((hits, miss_indices));
        }

        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT fingerprint, payload FROM cache WHERE key = ?1")
            .context("prepare cache select")?;

        for (i, text) in texts.iter().enumerate() {
            let key = hash_key(text);
            let live_fp = fingerprint(text);
            let row = stmt
                .query_row(params![key], |r| {
                    let stored_fp: String = r.get(0)?;
                    let payload: Vec<u8> = r.get(1)?;
                    Ok((stored_fp, payload))
                })
                .ok();

            match row {
                Some((stored_fp, payload)) if stored_fp == live_fp => {
                    hits.push(Some(bytes_to_f32_vec(&payload)?));
                }
                _ => {
                    // Miss, or fingerprint collision (EH-7).
                    if let Some((stored_fp, _)) = row {
                        if stored_fp != live_fp {
                            tracing::warn!(
                                key = %key,
                                "embedding cache fingerprint mismatch — treating as miss (EH-7)"
                            );
                        }
                    }
                    hits.push(None);
                    miss_indices.push(i);
                }
            }
        }

        Ok((hits, miss_indices))
    }

    /// Insert or replace `(text, vector)` pairs in a single transaction.
    /// Idempotent: re-inserting the same text overwrites the prior vector and
    /// refreshes the fingerprint.
    pub fn put_many(&self, items: &[(String, Vec<f32>)]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction().context("begin cache transaction")?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO cache (key, fingerprint, payload) \
                     VALUES (?1, ?2, ?3)",
                )
                .context("prepare cache insert")?;
            for (text, vec) in items {
                let key = hash_key(text);
                let fp = fingerprint(text);
                let payload = f32_vec_to_bytes(vec);
                stmt.execute(params![key, fp, payload])
                    .with_context(|| format!("insert cache row for key {key}"))?;
            }
        }
        tx.commit().context("commit cache transaction")?;
        Ok(())
    }
}

/// sha256-hex of the input. 64 lowercase hex chars.
pub fn hash_key(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Hex is ASCII; write! into String never fails.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// First `FINGERPRINT_CHARS` Unicode scalar values (chars, not bytes) of the input.
/// Used as a cheap, human-readable disambiguator against sha256 collisions (EH-7).
pub fn fingerprint(text: &str) -> String {
    text.chars().take(FINGERPRINT_CHARS).collect()
}

/// Encode a slice of `f32` as little-endian bytes.
fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Decode little-endian bytes back into a `Vec<f32>`. Errors if the byte count
/// is not a multiple of 4 (corrupt cache row).
fn bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        anyhow::bail!(
            "corrupt embedding cache payload: {} bytes is not a multiple of 4",
            bytes.len()
        );
    }
    let (chunks, _) = bytes.as_chunks::<4>();
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_key_is_deterministic_and_64_hex() {
        let a = hash_key("hello");
        let b = hash_key("hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_counts_chars_not_bytes() {
        // Each emoji is multi-byte in UTF-8 but counts as a single char here.
        let s: String = "🦀".repeat(FINGERPRINT_CHARS + 5);
        let fp = fingerprint(&s);
        assert_eq!(fp.chars().count(), FINGERPRINT_CHARS);
    }

    #[test]
    fn f32_roundtrip_preserves_exact_bits() {
        let v = vec![0.0_f32, -0.0, 1.0, -1.0, f32::NAN, f32::INFINITY, std::f32::consts::PI];
        let bytes = f32_vec_to_bytes(&v);
        let back = bytes_to_f32_vec(&bytes).expect("roundtrip");
        assert_eq!(back.len(), v.len());
        for (a, b) in v.iter().zip(back.iter()) {
            assert_eq!(a.to_le_bytes(), b.to_le_bytes(), "exact bit equality");
        }
    }

    #[test]
    fn bytes_to_f32_rejects_misaligned_payload() {
        let err = bytes_to_f32_vec(&[1, 2, 3]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("multiple of 4"), "got: {msg}");
    }
}
