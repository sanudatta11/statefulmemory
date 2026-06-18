//! Atomic-fact storage helpers (V5 migration).
//!
//! Each `Fact` is an (`subject`, `predicate`, `object`, optional `temporal`)
//! tuple extracted from an observation by Haiku or Sonnet. Facts cascade-
//! delete with their source observation; supersession is modeled but not
//! enforced — readers filter `superseded_by IS NULL` for active facts.
//!
//! Public API:
//!   * [`insert_fact`] — insert one fact, return its rowid.
//!   * [`facts_for_obs`] — list every fact attached to an observation.
//!   * [`search_facts`] — BM25 search across the FTS5 mirror.
//!
//! Spec sections: SC-8 (extract worker writes facts), SC-9 (`obs facts <id>`),
//! SC-10 (idempotent V5 migration).

use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use memlayer_core::error::{Error, Result};

/// One row in the `facts` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: i64,
    pub obs_id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub temporal: Option<String>,
    pub salience: f64,
    pub superseded_by: Option<i64>,
    pub extracted_by: String,
    pub extracted_at: String,
}

/// Caller-supplied input for [`insert_fact`]. `id`, `extracted_at`, and
/// `superseded_by` are managed by the DB; the caller fills the rest.
#[derive(Debug, Clone)]
pub struct FactInput<'a> {
    pub obs_id: i64,
    pub subject: &'a str,
    pub predicate: &'a str,
    pub object: &'a str,
    pub temporal: Option<&'a str>,
    pub salience: Option<f64>,
    /// "haiku" | "sonnet". Stored verbatim — callers are responsible for the
    /// canonical lowercase form.
    pub extracted_by: &'a str,
}

fn fact_from_row(row: &Row<'_>) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: row.get(0)?,
        obs_id: row.get(1)?,
        subject: row.get(2)?,
        predicate: row.get(3)?,
        object: row.get(4)?,
        temporal: row.get(5)?,
        salience: row.get(6)?,
        superseded_by: row.get(7)?,
        extracted_by: row.get(8)?,
        extracted_at: row.get(9)?,
    })
}

const SELECT_COLS: &str =
    "id, obs_id, subject, predicate, object, temporal, salience, superseded_by, extracted_by, extracted_at";

/// Insert one fact and return its newly-assigned rowid.
pub fn insert_fact(conn: &Connection, f: &FactInput<'_>) -> Result<i64> {
    let salience = f.salience.unwrap_or(0.5);
    conn.execute(
        "INSERT INTO facts (obs_id, subject, predicate, object, temporal, salience, extracted_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            f.obs_id,
            f.subject,
            f.predicate,
            f.object,
            f.temporal,
            salience,
            f.extracted_by,
        ],
    )
    .map_err(|e| Error::internal(format!("insert_fact: {e}")))?;
    Ok(conn.last_insert_rowid())
}

/// Return every fact attached to the given observation, including those that
/// have been superseded. Caller filters as needed.
pub fn facts_for_obs(conn: &Connection, obs_id: i64) -> Result<Vec<Fact>> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM facts WHERE obs_id = ?1 ORDER BY id ASC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("facts_for_obs prepare: {e}")))?;
    let rows = stmt
        .query_map(params![obs_id], fact_from_row)
        .map_err(|e| Error::internal(format!("facts_for_obs query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("facts_for_obs row: {e}")))?);
    }
    Ok(out)
}

/// FTS5 search across `(subject, predicate, object)`. Active facts only
/// (filters `superseded_by IS NULL`).
pub fn search_facts(conn: &Connection, q: &str, limit: i64) -> Result<Vec<Fact>> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM facts \
         WHERE id IN (SELECT rowid FROM facts_fts WHERE facts_fts MATCH ?1) \
           AND superseded_by IS NULL \
         ORDER BY salience DESC, id DESC LIMIT ?2"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("search_facts prepare: {e}")))?;
    let rows = stmt
        .query_map(params![q, limit], fact_from_row)
        .map_err(|e| Error::internal(format!("search_facts query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("search_facts row: {e}")))?);
    }
    Ok(out)
}

/// Mark one fact as superseded by another. Used when a future spec wires in
/// fact-supersession; the schema supports it from V5 onward.
pub fn mark_superseded(conn: &Connection, old_id: i64, new_id: i64) -> Result<()> {
    let n = conn
        .execute(
            "UPDATE facts SET superseded_by = ?1 WHERE id = ?2",
            params![new_id, old_id],
        )
        .map_err(|e| Error::internal(format!("mark_superseded: {e}")))?;
    if n == 0 {
        return Err(Error::NotFound(format!("fact id={old_id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_write;
    use tempfile::TempDir;

    fn seed_obs(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observations (sync_id, session_id, type, title, content) \
             VALUES ('sync-1', 's1', 'note', 'pgx vs gorm', 'team picked pgx for raw SQL')",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn fresh_db() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_write(&path).unwrap();
        (dir, conn)
    }

    #[test]
    fn fact_fts_returns_inserted() {
        let (_dir, conn) = fresh_db();
        let obs_id = seed_obs(&conn);
        let id = insert_fact(
            &conn,
            &FactInput {
                obs_id,
                subject: "team",
                predicate: "prefers",
                object: "raw SQL via pgx",
                temporal: None,
                salience: Some(0.9),
                extracted_by: "haiku",
            },
        )
        .unwrap();
        assert!(id > 0);

        let hits = search_facts(&conn, "pgx", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id);
        assert_eq!(hits[0].subject, "team");
        assert_eq!(hits[0].extracted_by, "haiku");
    }

    #[test]
    fn facts_for_obs_round_trip() {
        let (_dir, conn) = fresh_db();
        let obs_id = seed_obs(&conn);
        let f1 = insert_fact(
            &conn,
            &FactInput {
                obs_id,
                subject: "team",
                predicate: "prefers",
                object: "raw SQL via pgx",
                temporal: None,
                salience: None,
                extracted_by: "haiku",
            },
        )
        .unwrap();
        let f2 = insert_fact(
            &conn,
            &FactInput {
                obs_id,
                subject: "team",
                predicate: "rejected",
                object: "GORM",
                temporal: None,
                salience: None,
                extracted_by: "haiku",
            },
        )
        .unwrap();

        let facts = facts_for_obs(&conn, obs_id).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].id, f1);
        assert_eq!(facts[1].id, f2);
        assert!((facts[0].salience - 0.5).abs() < 1e-9);
    }

    #[test]
    fn cascade_delete_obs_drops_facts() {
        let (_dir, conn) = fresh_db();
        let obs_id = seed_obs(&conn);
        insert_fact(
            &conn,
            &FactInput {
                obs_id,
                subject: "x",
                predicate: "y",
                object: "z",
                temporal: None,
                salience: None,
                extracted_by: "haiku",
            },
        )
        .unwrap();

        let n: i64 = conn
            .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        conn.execute("DELETE FROM observations WHERE id = ?1", params![obs_id])
            .unwrap();

        let n: i64 = conn
            .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "facts must cascade-delete with the observation");

        // FTS mirror also drained via AFTER DELETE trigger on facts.
        let n_fts: i64 = conn
            .query_row("SELECT count(*) FROM facts_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n_fts, 0);
    }

    #[test]
    fn supersession_filtering() {
        let (_dir, conn) = fresh_db();
        let obs_id = seed_obs(&conn);
        let old = insert_fact(
            &conn,
            &FactInput {
                obs_id,
                subject: "team",
                predicate: "prefers",
                object: "diesel",
                temporal: None,
                salience: None,
                extracted_by: "haiku",
            },
        )
        .unwrap();
        let new = insert_fact(
            &conn,
            &FactInput {
                obs_id,
                subject: "team",
                predicate: "prefers",
                object: "pgx",
                temporal: None,
                salience: None,
                extracted_by: "haiku",
            },
        )
        .unwrap();
        mark_superseded(&conn, old, new).unwrap();

        // search_facts excludes superseded.
        let hits = search_facts(&conn, "diesel OR pgx", 10).unwrap();
        let ids: Vec<i64> = hits.iter().map(|f| f.id).collect();
        assert!(ids.contains(&new));
        assert!(!ids.contains(&old), "superseded fact must not surface");

        // facts_for_obs returns both (caller filters as needed).
        let all = facts_for_obs(&conn, obs_id).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn v5_idempotent() {
        // Running V5 twice (via two open_write sessions) is a no-op.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        drop(open_write(&path).unwrap());
        // Second open: refinery sees the migration as already applied; the
        // CREATE ... IF NOT EXISTS statements would also no-op if it tried.
        let conn = open_write(&path).unwrap();
        let obs_id = seed_obs(&conn);
        insert_fact(
            &conn,
            &FactInput {
                obs_id,
                subject: "x",
                predicate: "y",
                object: "z",
                temporal: None,
                salience: None,
                extracted_by: "haiku",
            },
        )
        .unwrap();
    }
}
