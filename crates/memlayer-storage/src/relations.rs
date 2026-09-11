//! Database storage for observation relationship graph edges (Phase 4).

use memlayer_core::error::{Error, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservationRelation {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub relation_type: String,
    pub confidence: f64,
    pub created_at: String,
}

/// Add an explicit graph edge between two observations.
pub fn add_relation(
    conn: &Connection,
    source_id: i64,
    target_id: i64,
    relation_type: &str,
    confidence: f64,
) -> Result<ObservationRelation> {
    let now = memlayer_core::time::now_rfc3339();
    conn.execute(
        "INSERT INTO observation_relations (source_id, target_id, relation_type, confidence, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(source_id, target_id, relation_type) DO UPDATE SET
            confidence = ?4,
            created_at = ?5",
        params![source_id, target_id, relation_type, confidence, &now],
    )
    .map_err(|e| Error::internal(format!("insert relation: {e}")))?;

    let id = conn.last_insert_rowid();
    Ok(ObservationRelation {
        id,
        source_id,
        target_id,
        relation_type: relation_type.to_string(),
        confidence,
        created_at: now,
    })
}

/// Fetch all outgoing and incoming relations for a given observation ID.
pub fn get_relations_for_observation(
    conn: &Connection,
    obs_id: i64,
) -> Result<Vec<ObservationRelation>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, source_id, target_id, relation_type, confidence, created_at
             FROM observation_relations
             WHERE source_id = ?1 OR target_id = ?1
             ORDER BY created_at DESC",
        )
        .map_err(|e| Error::internal(format!("prepare get_relations: {e}")))?;

    let rows = stmt
        .query_map(params![obs_id], |row| {
            Ok(ObservationRelation {
                id: row.get(0)?,
                source_id: row.get(1)?,
                target_id: row.get(2)?,
                relation_type: row.get(3)?,
                confidence: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| Error::internal(format!("query_map get_relations: {e}")))?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("row get_relations: {e}")))?);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn add_and_get_relations_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let mut conn = crate::db::open_write(&path).unwrap();
        crate::run_migrations(&mut conn).unwrap();

        // Create 2 test observations
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observations (id, sync_id, session_id, type, title, content)
             VALUES (1, 'sync1', 's1', 'decision', 'Obs 1', 'Content 1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observations (id, sync_id, session_id, type, title, content)
             VALUES (2, 'sync2', 's1', 'decision', 'Obs 2', 'Content 2')",
            [],
        )
        .unwrap();

        let rel = add_relation(&conn, 2, 1, "conflicts_with", 0.95).unwrap();
        assert_eq!(rel.source_id, 2);
        assert_eq!(rel.target_id, 1);
        assert_eq!(rel.relation_type, "conflicts_with");

        let rels = get_relations_for_observation(&conn, 1).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].relation_type, "conflicts_with");
    }
}
