// Generated with AI Coding Rules Hub
//! Window-keyed cache for LLM fact extraction.
//!
//! Each cache entry is keyed by `sha256(window_json)` where `window_json` is
//! a deterministic serialization of the input turns + current_date for that
//! extraction window. The payload is the JSON-encoded `Vec<Fact>` that the
//! Haiku call returned. A boolean `extraction_failed` column lets us record
//! windows where parse_facts gave up (EH-4) so we don't keep retrying them
//! forever — the orchestrator (spec-task-18) checks this flag and skips
//! known-bad windows on resume.
//!
//! API mirrors `memlayer_embed::cache::EmbeddingCache` (sha256 key,
//! fingerprint collision check, batch get/put inside one transaction)
//! so callers don't have to learn two cache shapes.
//!
//! Spec links: TS-7, EH-4, SC-8. Plan: §1.4, P2 §4.6.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::Fact;

const CACHE_FILENAME: &str = "extraction.cache.sqlite";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS cache (
    key                TEXT PRIMARY KEY,
    fingerprint        TEXT NOT NULL,
    payload            TEXT NOT NULL,
    extraction_failed  INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

/// Compute the sha256-hex key for a window of input data.
pub fn hash_key(window_json: &str) -> String {
    let mut h = Sha256::new();
    h.update(window_json.as_bytes());
    format!("{:x}", h.finalize())
}

/// First 80 chars of the input for collision detection (mirrors EmbeddingCache).
pub fn fingerprint(window_json: &str) -> String {
    window_json.chars().take(80).collect()
}

/// Outcome stored for one window.
#[derive(Debug, Clone)]
pub enum CachedExtraction {
    Ok(Vec<Fact>),
    Failed,
}

/// SQLite-backed cache for `(window_json -> Vec<Fact>)`.
pub struct ExtractionCache {
    conn: Arc<Mutex<Connection>>,
}

impl ExtractionCache {
    /// Open or create the cache DB at `<data_dir>/extraction.cache.sqlite`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(CACHE_FILENAME);
        let conn = Connection::open(&path)
            .with_context(|| format!("open extraction cache at {}", path.display()))?;
        // journal_mode returns the new mode as a row, so use `pragma()` not
        // `pragma_update()` (matches EmbeddingCache's fix).
        conn.pragma(None, "journal_mode", "WAL", |_| Ok(()))
            .context("set journal_mode=WAL")?;
        conn.pragma(None, "synchronous", "NORMAL", |_| Ok(()))
            .context("set synchronous=NORMAL")?;
        conn.execute_batch(SCHEMA)
            .context("create extraction cache schema")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Look up the cached outcome for each `window_json` in input order.
    /// Returns `(hits, miss_indices)`.
    /// `hits[i]` is `Some(_)` if cached (whether Ok or Failed); `None` if miss.
    pub fn get_many(
        &self,
        windows: &[&str],
    ) -> Result<(Vec<Option<CachedExtraction>>, Vec<usize>)> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT fingerprint, payload, extraction_failed FROM cache WHERE key = ?1")
            .context("prepare cache lookup")?;
        let mut hits: Vec<Option<CachedExtraction>> = Vec::with_capacity(windows.len());
        let mut misses: Vec<usize> = Vec::new();
        for (i, w) in windows.iter().enumerate() {
            let key = hash_key(w);
            let live_fp = fingerprint(w);
            let row: Option<(String, String, i64)> = stmt
                .query_row(params![key], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
                })
                .ok();
            match row {
                Some((stored_fp, payload, failed)) if stored_fp == live_fp => {
                    if failed != 0 {
                        hits.push(Some(CachedExtraction::Failed));
                    } else {
                        match serde_json::from_str::<Vec<Fact>>(&payload) {
                            Ok(facts) => hits.push(Some(CachedExtraction::Ok(facts))),
                            Err(e) => {
                                tracing::warn!(error = %e, "extraction cache payload corrupt; treating as miss");
                                hits.push(None);
                                misses.push(i);
                            }
                        }
                    }
                }
                Some((_other_fp, _, _)) => {
                    tracing::warn!("extraction cache fingerprint mismatch; treating as miss");
                    hits.push(None);
                    misses.push(i);
                }
                None => {
                    hits.push(None);
                    misses.push(i);
                }
            }
        }
        Ok((hits, misses))
    }

    /// Store successful extractions in a single transaction.
    pub fn put_many(&self, items: &[(String, Vec<Fact>)]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction().context("begin extraction cache tx")?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO cache(key, fingerprint, payload, extraction_failed) \
                     VALUES (?1, ?2, ?3, 0)",
                )
                .context("prepare extraction cache insert")?;
            for (window, facts) in items {
                let key = hash_key(window);
                let fp = fingerprint(window);
                let payload =
                    serde_json::to_string(facts).context("serialize facts for cache")?;
                stmt.execute(params![key, fp, payload])
                    .context("insert extraction cache row")?;
            }
        }
        tx.commit().context("commit extraction cache tx")?;
        Ok(())
    }

    /// Mark a window as permanently failed (EH-4: malformed Haiku output).
    pub fn put_failed(&self, window: &str) -> Result<()> {
        let conn = self.conn.lock();
        let key = hash_key(window);
        let fp = fingerprint(window);
        conn.execute(
            "INSERT OR REPLACE INTO cache(key, fingerprint, payload, extraction_failed) \
             VALUES (?1, ?2, '[]', 1)",
            params![key, fp],
        )
        .context("insert failed extraction marker")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_creates_schema() {
        let tmp = TempDir::new().unwrap();
        let _c = ExtractionCache::open(tmp.path()).unwrap();
        // Re-open should still work.
        let _c2 = ExtractionCache::open(tmp.path()).unwrap();
    }

    #[test]
    fn put_then_get_roundtrips_facts() {
        let tmp = TempDir::new().unwrap();
        let c = ExtractionCache::open(tmp.path()).unwrap();
        let f = Fact {
            subject: "Caroline".into(),
            predicate: "attended".into(),
            object: "support group".into(),
            temporal: Some("2023-05-08".into()),
            salience: 0.9,
            evidence_obs_id: 101,
            source_session: Some("s-abc".into()),
        };
        c.put_many(&[("window-A".to_string(), vec![f.clone()])]).unwrap();
        let (hits, miss) = c.get_many(&["window-A"]).unwrap();
        assert!(miss.is_empty());
        match &hits[0] {
            Some(CachedExtraction::Ok(facts)) => {
                assert_eq!(facts.len(), 1);
                assert_eq!(facts[0].subject, "Caroline");
            }
            other => panic!("expected Ok hit, got {other:?}"),
        }
    }

    #[test]
    fn miss_returns_correct_indices() {
        let tmp = TempDir::new().unwrap();
        let c = ExtractionCache::open(tmp.path()).unwrap();
        c.put_many(&[("a".to_string(), vec![])]).unwrap();
        let (hits, miss) = c.get_many(&["a", "b", "c"]).unwrap();
        assert!(matches!(hits[0], Some(CachedExtraction::Ok(_))));
        assert!(hits[1].is_none());
        assert!(hits[2].is_none());
        assert_eq!(miss, vec![1, 2]);
    }

    #[test]
    fn put_failed_marker_returned_as_failed() {
        let tmp = TempDir::new().unwrap();
        let c = ExtractionCache::open(tmp.path()).unwrap();
        c.put_failed("bad-window").unwrap();
        let (hits, miss) = c.get_many(&["bad-window"]).unwrap();
        assert!(miss.is_empty());
        assert!(matches!(hits[0], Some(CachedExtraction::Failed)));
    }
}
