// Generated with AI Coding Rules Hub
//! TS-8 / TS-9 integration tests for the eval-side facts.db.
//!
//! Mirrors the unit tests inside `facts_db.rs::tests` but lives in
//! `tests/` so they're exercised on the public API only and don't depend
//! on `#[cfg(test)]` private bits.

use memlayer_eval::facts_db::FactsDb;
use rusqlite::params;
use tempfile::TempDir;

#[test]
fn ts8_facts_db_open_applies_v1_cleanly() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("facts.db");
    let db = FactsDb::open(&path).expect("open + migrate");

    let v: String = db
        .conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "5");

    // Re-opening must be idempotent.
    drop(db);
    let _db2 = FactsDb::open(&path).expect("reopen");
}

#[test]
fn ts9_facts_fts_trigger_indexes_inserted_rows() {
    let tmp = TempDir::new().unwrap();
    let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();

    db.conn
        .execute(
            "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, temporal, salience) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "test-proj",
                42_i64,
                "Caroline",
                "attended",
                "uniqueneedlestring",
                "2023-05-08",
                0.9_f64,
            ],
        )
        .unwrap();

    let count: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH ?1",
            params!["uniqueneedlestring"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "facts_fts trigger must mirror INSERT");
}

#[test]
fn facts_vec_accepts_384_dim_blob() {
    let tmp = TempDir::new().unwrap();
    let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();

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
