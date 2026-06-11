// Generated with AI Coding Rules Hub
//! Open and migrate the eval-side `facts.db`.
//!
//! Lifecycle (per spec §1.2):
//!   1. Ensure parent directory exists.
//!   2. Register `sqlite_vec` as a SQLite auto-extension (Once-guarded).
//!   3. `Connection::open(path)` — vec0 is now loadable.
//!   4. Smoke-test `vec_version()` — EH-1 hard fails on missing extension.
//!   5. Run refinery migrations from `crates/memlayer-eval/migrations_eval/`.
//!
//! This is intentionally a **separate** migration root from the storage
//! daemon's `crates/memlayer-storage/migrations/` (SC-10 tripwire). refinery
//! supports multiple `embed_migrations!` macro instances per workspace, so
//! the two roots coexist.
//!
//! Spec: retrieval-upgrade-v1 §1.1, §1.2, §4.2. Refs: TS-8, TS-9, SC-10.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::vec_index::open_with_vec;

mod migrations {
    refinery::embed_migrations!("../memlayer-eval/migrations_eval");
}

/// Wrapper around an open `facts.db` connection.
///
/// Holds the connection by value so callers can run statements through
/// the public methods or access `.conn` directly for ad-hoc queries.
pub struct FactsDb {
    pub conn: Connection,
}

impl FactsDb {
    /// Open or create the facts DB at `path`, load sqlite-vec, and run all
    /// pending refinery migrations.
    pub fn open(path: &Path) -> Result<Self> {
        // open_with_vec handles parent dir creation, sqlite-vec
        // auto-extension registration, and vec_version() smoke test (EH-1).
        let mut conn = open_with_vec(path).context("open facts.db with vec")?;

        migrations::migrations::runner()
            .run(&mut conn)
            .context("run facts.db migrations")?;

        Ok(Self { conn })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::TempDir;

    #[test]
    fn ts8_open_applies_v1_migration_cleanly() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("facts.db");
        let db = FactsDb::open(&path).expect("open + migrate");

        // schema_meta should reflect version 1.
        let v: String = db
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key='version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "1");

        // Re-opening must be idempotent (refinery records applied versions).
        drop(db);
        let _db2 = FactsDb::open(&path).expect("reopen");
    }

    #[test]
    fn ts9_facts_fts_trigger_fires_on_insert() {
        let tmp = TempDir::new().unwrap();
        let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();

        db.conn
            .execute(
                "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, temporal, salience) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    "test-proj",
                    101_i64,
                    "Caroline",
                    "attended",
                    "LGBTQ-support-uniqueneedle",
                    "2023-05-08",
                    0.9_f64,
                ],
            )
            .unwrap();

        // The trigger should have populated facts_fts. MATCH on the unique
        // object string proves the row is FTS-indexed.
        let count: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH ?1",
                params!["LGBTQ-support-uniqueneedle"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn vec0_table_accepts_inserts() {
        let tmp = TempDir::new().unwrap();
        let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();

        // Build a 384-dim f32 vector as little-endian bytes.
        let v: Vec<f32> = (0..384).map(|i| i as f32 / 384.0).collect();
        let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        db.conn
            .execute(
                "INSERT INTO facts_vec(rowid, embedding) VALUES (?1, ?2)",
                params![1_i64, bytes],
            )
            .unwrap();

        let n: i64 = db
            .conn
            .query_row("SELECT count(*) FROM facts_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
