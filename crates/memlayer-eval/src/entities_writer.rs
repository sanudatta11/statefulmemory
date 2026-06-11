// Generated with AI Coding Rules Hub
//! Entity store writer for the eval-side facts.db.
//!
//! Provides:
//! * [`upsert_entity`] — insert-or-get an entity row by (project, name).
//! * [`link_entity_to_facts`] — attach an entity to one or more fact_ids.
//! * [`upsert_entity_vec`] — write the entity's embedding to `entities_vec`.
//! * [`bulk_upsert_entities`] — orchestrator wrapper that does all three in
//!   one transaction for efficiency.
//!
//! Mem0's design (spec §3 audit): entities are project-scoped with a UNIQUE
//! constraint on (project, name) so the same person mentioned across many
//! facts dedupes to one row. Embeddings are computed once at extraction
//! time and reused for every query at retrieval time (SC-12).
//!
//! Spec: retrieval-upgrade-v1 §5, SC-12. Plan: P2 §4.5.
//! Used by extract_pipeline (spec-task-19c) and retrieve_facts (spec-task-19d).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// One entity ready to write: name + 384-dim embedding + the fact ids it
/// describes. The orchestrator (spec-task-19c) builds these from the
/// per-fact entity lists produced by `EntityExtractor`.
pub struct EntityRow {
    pub name: String,
    pub embedding: Vec<f32>,
    pub linked_fact_ids: Vec<i64>,
    pub kind: Option<String>,
}

/// Insert or get an entity row by `(project, name)`. Returns the entity id.
///
/// `name` should already be lowercased and trimmed by the caller (the
/// EntityExtractor in memlayer-extract handles that contract).
pub fn upsert_entity(
    conn: &Connection,
    project: &str,
    name: &str,
    kind: Option<&str>,
) -> Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM entities WHERE project = ?1 AND name = ?2",
            params![project, name],
            |r| r.get(0),
        )
        .optional()
        .context("look up existing entity")?;
    if let Some(id) = existing {
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO entities(project, name, kind) VALUES (?1, ?2, ?3)",
        params![project, name, kind],
    )
    .context("insert new entity")?;
    Ok(conn.last_insert_rowid())
}

/// Attach an entity to one or more facts via the entity_links join table.
/// PRIMARY KEY(entity_id, fact_id) makes this idempotent on duplicates
/// (PK conflict is silently ignored via INSERT OR IGNORE).
pub fn link_entity_to_facts(
    conn: &Connection,
    entity_id: i64,
    fact_ids: &[i64],
) -> Result<usize> {
    if fact_ids.is_empty() {
        return Ok(0);
    }
    let mut stmt = conn
        .prepare("INSERT OR IGNORE INTO entity_links(entity_id, fact_id) VALUES (?1, ?2)")
        .context("prepare entity_links insert")?;
    let mut written = 0usize;
    for &fid in fact_ids {
        let n = stmt
            .execute(params![entity_id, fid])
            .context("insert entity_links row")?;
        written += n;
    }
    Ok(written)
}

/// Write or replace the embedding for `entity_id` in `entities_vec`.
///
/// vec0 doesn't support INSERT OR REPLACE on older sqlite-vec versions, so
/// we DELETE + INSERT to make this idempotent across re-runs of extraction.
pub fn upsert_entity_vec(
    conn: &Connection,
    entity_id: i64,
    embedding: &[f32],
) -> Result<()> {
    let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "DELETE FROM entities_vec WHERE rowid = ?1",
        params![entity_id],
    )
    .context("clear stale entities_vec row")?;
    conn.execute(
        "INSERT INTO entities_vec(rowid, embedding) VALUES (?1, ?2)",
        params![entity_id, bytes],
    )
    .context("insert entities_vec row")?;
    Ok(())
}

/// Bulk-write a batch of entity rows in a single transaction.
///
/// For each `EntityRow`:
///   1. `upsert_entity(project, name)` -> entity_id
///   2. `link_entity_to_facts(entity_id, linked_fact_ids)`
///   3. `upsert_entity_vec(entity_id, embedding)`
///
/// Returns the count of distinct entities touched (whether newly inserted
/// or already-existing).
pub fn bulk_upsert_entities(
    conn: &mut Connection,
    project: &str,
    rows: &[EntityRow],
) -> Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction().context("begin entities tx")?;
    let mut touched = 0usize;
    for row in rows {
        let id = upsert_entity(&tx, project, &row.name, row.kind.as_deref())
            .with_context(|| format!("upsert entity {}", row.name))?;
        link_entity_to_facts(&tx, id, &row.linked_fact_ids)
            .with_context(|| format!("link entity {} to facts", row.name))?;
        upsert_entity_vec(&tx, id, &row.embedding)
            .with_context(|| format!("write entity_vec for {}", row.name))?;
        touched += 1;
    }
    tx.commit().context("commit entities tx")?;
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts_db::FactsDb;
    use tempfile::TempDir;

    fn dummy_vec(seed: f32) -> Vec<f32> {
        (0..384).map(|i| seed + (i as f32 / 384.0)).collect()
    }

    fn insert_test_fact(conn: &Connection, project: &str, obs_id: i64, subject: &str) -> i64 {
        conn.execute(
            "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, salience) \
             VALUES (?1, ?2, ?3, 'is', 'x', 1.0)",
            params![project, obs_id, subject],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn upsert_entity_returns_same_id_on_duplicate() {
        let tmp = TempDir::new().unwrap();
        let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();
        let id1 = upsert_entity(&db.conn, "p1", "caroline", Some("person")).unwrap();
        let id2 = upsert_entity(&db.conn, "p1", "caroline", None).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn upsert_entity_isolates_by_project() {
        let tmp = TempDir::new().unwrap();
        let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();
        let id1 = upsert_entity(&db.conn, "p1", "caroline", None).unwrap();
        let id2 = upsert_entity(&db.conn, "p2", "caroline", None).unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn link_entity_to_facts_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();
        let fact_id = insert_test_fact(&db.conn, "p1", 100, "Caroline");
        let ent_id = upsert_entity(&db.conn, "p1", "caroline", None).unwrap();
        let n1 = link_entity_to_facts(&db.conn, ent_id, &[fact_id]).unwrap();
        let n2 = link_entity_to_facts(&db.conn, ent_id, &[fact_id]).unwrap();
        assert_eq!(n1, 1);
        assert_eq!(n2, 0, "duplicate link should be ignored");
    }

    #[test]
    fn upsert_entity_vec_replaces_existing() {
        let tmp = TempDir::new().unwrap();
        let db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();
        let ent_id = upsert_entity(&db.conn, "p1", "caroline", None).unwrap();
        upsert_entity_vec(&db.conn, ent_id, &dummy_vec(0.0)).unwrap();
        upsert_entity_vec(&db.conn, ent_id, &dummy_vec(0.5)).unwrap();
        let n: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM entities_vec WHERE rowid = ?1",
                params![ent_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "replace must not duplicate the row");
    }

    #[test]
    fn bulk_upsert_entities_writes_all_three_tables() {
        let tmp = TempDir::new().unwrap();
        let mut db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();
        let f1 = insert_test_fact(&db.conn, "p1", 100, "Caroline");
        let f2 = insert_test_fact(&db.conn, "p1", 101, "Melanie");
        let rows = vec![
            EntityRow {
                name: "caroline".into(),
                embedding: dummy_vec(0.0),
                linked_fact_ids: vec![f1],
                kind: Some("person".into()),
            },
            EntityRow {
                name: "melanie".into(),
                embedding: dummy_vec(0.5),
                linked_fact_ids: vec![f1, f2],
                kind: Some("person".into()),
            },
        ];
        let n = bulk_upsert_entities(&mut db.conn, "p1", &rows).unwrap();
        assert_eq!(n, 2);

        let ents: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM entities WHERE project='p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ents, 2);

        let links: i64 = db
            .conn
            .query_row("SELECT count(*) FROM entity_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(links, 3);
    }
}
