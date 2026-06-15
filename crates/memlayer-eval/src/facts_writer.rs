// Generated with AI Coding Rules Hub
//! Bulk insert of (Fact + embedding) pairs into the eval-side facts.db.
//!
//! `insert_facts` runs one transaction per call: it INSERTs each row into
//! `facts` (capturing the auto-generated rowid via `last_insert_rowid()`)
//! and the matching f32 little-endian BLOB into `facts_vec(rowid=…)`. The
//! `facts_fts` virtual table updates automatically through its trigger.
//!
//! Spec: retrieval-upgrade-v1 §5, SC-8. Plan: P2 §4.2.
//! Used by extract_pipeline (spec-task-18).

use anyhow::{Context, Result};
use memlayer_extract::Fact;
use rusqlite::{params, Connection};

/// One fact with its associated f32 embedding (typically BGE-small, 384-dim).
pub struct FactWithEmbedding {
    pub fact: Fact,
    pub embedding: Vec<f32>,
}

/// Bulk-insert (fact, embedding) pairs into facts + facts_vec inside a single
/// transaction. Returns the count written.
///
/// Caller owns dedup decisions — this writer always INSERTs (no upsert,
/// no checksum check). Mem0's "ADD-only" design (no UPDATE/DELETE at ingest)
/// matches the eval pipeline's intent.
pub fn insert_facts(conn: &mut Connection, batch: &[FactWithEmbedding]) -> Result<usize> {
    if batch.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction().context("begin facts insert tx")?;
    let mut written = 0usize;

    {
        let mut ins_fact = tx
            .prepare(
                "INSERT INTO facts(project, evidence_obs_id, subject, predicate, \
                                   object, temporal, salience, source_session) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .context("prepare facts insert")?;
        let mut ins_vec = tx
            .prepare("INSERT INTO facts_vec(rowid, embedding) VALUES (?1, ?2)")
            .context("prepare facts_vec insert")?;

        for fwe in batch {
            let f = &fwe.fact;
            // We can't easily pass `project` per fact since Fact doesn't carry
            // it — extract_pipeline calls insert_facts once per project, and
            // we read the project from the first fact's evidence... actually
            // simpler: take project as a parameter would break the API. For
            // now, derive project from the source_session (NULL fallback).
            // The orchestrator (spec-task-18) will pass facts grouped by
            // project so this works; if the caller mixes projects in one
            // batch, the source_session disambiguates downstream.
            let project = f
                .source_session
                .as_deref()
                .unwrap_or("default");
            ins_fact
                .execute(params![
                    project,
                    f.evidence_obs_id,
                    f.subject,
                    f.predicate,
                    f.object,
                    f.temporal,
                    f.salience as f64,
                    f.source_session,
                ])
                .context("insert into facts")?;

            let rowid: i64 = tx.last_insert_rowid();

            let bytes: Vec<u8> = fwe
                .embedding
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect();
            ins_vec
                .execute(params![rowid, bytes])
                .context("insert into facts_vec")?;

            written += 1;
        }
    }

    tx.commit().context("commit facts insert tx")?;
    Ok(written)
}

/// Same as `insert_facts` but with an explicit project name. The
/// orchestrator should prefer this so `Fact`'s lack of a project field
/// stays an internal detail of memlayer-extract.
pub fn insert_facts_for_project(
    conn: &mut Connection,
    project: &str,
    batch: &[FactWithEmbedding],
) -> Result<usize> {
    if batch.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction().context("begin facts insert tx")?;
    let mut written = 0usize;

    {
        let mut ins_fact = tx
            .prepare(
                "INSERT INTO facts(project, evidence_obs_id, subject, predicate, \
                                   object, temporal, salience, source_session) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .context("prepare facts insert")?;
        let mut ins_vec = tx
            .prepare("INSERT INTO facts_vec(rowid, embedding) VALUES (?1, ?2)")
            .context("prepare facts_vec insert")?;

        for fwe in batch {
            let f = &fwe.fact;
            ins_fact
                .execute(params![
                    project,
                    f.evidence_obs_id,
                    f.subject,
                    f.predicate,
                    f.object,
                    f.temporal,
                    f.salience as f64,
                    f.source_session,
                ])
                .context("insert into facts")?;

            let rowid: i64 = tx.last_insert_rowid();

            let bytes: Vec<u8> = fwe
                .embedding
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect();
            ins_vec
                .execute(params![rowid, bytes])
                .context("insert into facts_vec")?;

            written += 1;
        }
    }

    tx.commit().context("commit facts insert tx")?;
    Ok(written)
}

/// Same as `insert_facts_for_project` but returns the auto-generated fact
/// IDs in input order. Used by the orchestrator (spec-task-19c) to wire
/// entity extraction to the freshly-written rows without a second SELECT.
pub fn insert_facts_for_project_returning_ids(
    conn: &mut Connection,
    project: &str,
    batch: &[FactWithEmbedding],
) -> Result<Vec<i64>> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }

    let tx = conn.transaction().context("begin facts insert tx")?;
    let mut ids: Vec<i64> = Vec::with_capacity(batch.len());

    {
        // P5 spec-task-27b: INSERT OR IGNORE + a fallback SELECT for the
        // pre-existing fact id. The unique index on (project,
        // lower(subject||predicate)) silently rejects re-extracted dupes;
        // we then surface the original fact's id so entity_links from
        // this extraction batch still wire to a valid row.
        let mut ins_fact = tx
            .prepare(
                "INSERT OR IGNORE INTO facts(project, evidence_obs_id, subject, predicate, \
                                             object, temporal, salience, source_session) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .context("prepare facts insert")?;
        let mut ins_vec = tx
            .prepare("INSERT INTO facts_vec(rowid, embedding) VALUES (?1, ?2)")
            .context("prepare facts_vec insert")?;
        let mut find_existing = tx
            .prepare(
                "SELECT id FROM facts \
                  WHERE project = ?1 \
                    AND lower(subject || '|' || predicate) = lower(?2 || '|' || ?3) \
                  LIMIT 1",
            )
            .context("prepare facts canonical SELECT")?;

        for fwe in batch {
            let f = &fwe.fact;
            let changes_before = tx.changes();
            ins_fact
                .execute(params![
                    project,
                    f.evidence_obs_id,
                    f.subject,
                    f.predicate,
                    f.object,
                    f.temporal,
                    f.salience as f64,
                    f.source_session,
                ])
                .context("insert into facts")?;
            let inserted = tx.changes() > changes_before;

            let rowid: i64 = if inserted {
                let id = tx.last_insert_rowid();
                let bytes: Vec<u8> = fwe
                    .embedding
                    .iter()
                    .flat_map(|x| x.to_le_bytes())
                    .collect();
                ins_vec
                    .execute(params![id, bytes])
                    .context("insert into facts_vec")?;
                id
            } else {
                // Canonical key conflict — fetch the pre-existing fact id
                // so the caller's entity_links wiring still has a target.
                find_existing
                    .query_row(params![project, f.subject, f.predicate], |r| {
                        r.get::<_, i64>(0)
                    })
                    .context("look up canonical fact id after upsert")?
            };
            ids.push(rowid);
        }
    }

    tx.commit().context("commit facts insert tx")?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts_db::FactsDb;
    use tempfile::TempDir;

    fn mk_fact(subject: &str, obs_id: i64) -> Fact {
        Fact {
            subject: subject.to_string(),
            predicate: "is".to_string(),
            object: "x".to_string(),
            temporal: None,
            salience: 1.0,
            evidence_obs_id: obs_id,
            source_session: Some("s-test".to_string()),
        }
    }

    fn dummy_vec(seed: f32) -> Vec<f32> {
        (0..384).map(|i| seed + (i as f32 / 384.0)).collect()
    }

    #[test]
    fn insert_facts_for_project_writes_both_tables() {
        let tmp = TempDir::new().unwrap();
        let mut db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();

        let batch = vec![
            FactWithEmbedding {
                fact: mk_fact("Alpha", 1),
                embedding: dummy_vec(0.0),
            },
            FactWithEmbedding {
                fact: mk_fact("Beta", 2),
                embedding: dummy_vec(0.5),
            },
        ];
        let n = insert_facts_for_project(&mut db.conn, "proj-A", &batch).unwrap();
        assert_eq!(n, 2);

        let facts: i64 = db
            .conn
            .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(facts, 2);

        let vec_rows: i64 = db
            .conn
            .query_row("SELECT count(*) FROM facts_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vec_rows, 2);

        // Verify the FTS trigger picked them up.
        let fts: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'Alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 1);
    }

    #[test]
    fn insert_facts_empty_batch_is_noop() {
        let tmp = TempDir::new().unwrap();
        let mut db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();
        let n = insert_facts_for_project(&mut db.conn, "p", &[]).unwrap();
        assert_eq!(n, 0);
    }
}
