//! Dump a project SQLite DB into [`ArchivePayload`] and apply a payload back.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use memlayer_core::error::Error;

use crate::error::{Result, SyncError};
use crate::mem_archive::{
    ArchivedFact, ArchivedObservation, ArchivedPrompt, ArchivedRelation, ArchivedSession,
    ArchivePayload,
};

const SCHEMA_VERSION: i32 = 8;

#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub observations_imported: i64,
    pub sessions_imported: i64,
    pub prompts_imported: i64,
    pub facts_imported: i64,
    pub relations_imported: i64,
    pub skipped: i64,
}

fn storage_err(e: rusqlite::Error) -> SyncError {
    SyncError::Storage(Error::internal(e.to_string()))
}

/// Snapshot every session, observation (including soft-deleted), prompt, fact,
/// and relation. Embeddings are omitted and re-queued on import.
pub fn dump_payload(conn: &Connection, project: &str, exported_at: &str) -> Result<ArchivePayload> {
    let sessions = dump_sessions(conn)?;
    let observations = dump_observations(conn)?;
    let prompts = dump_prompts(conn)?;
    let facts = dump_facts(conn)?;
    let relations = dump_relations(conn)?;
    Ok(ArchivePayload {
        format: "memlayer.archive".into(),
        archive_version: 1,
        schema_version: SCHEMA_VERSION,
        exported_at: exported_at.into(),
        project: project.into(),
        observations,
        sessions,
        prompts,
        facts,
        relations,
    })
}

fn dump_sessions(conn: &Connection) -> Result<Vec<ArchivedSession>> {
    let mut stmt = conn
        .prepare("SELECT id, directory, started_at, ended_at, summary FROM sessions ORDER BY id")
        .map_err(storage_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArchivedSession {
                id: row.get(0)?,
                directory: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                summary: row.get(4)?,
            })
        })
        .map_err(storage_err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(storage_err)
}

fn dump_observations(conn: &Connection) -> Result<Vec<ArchivedObservation>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, sync_id, session_id, type, title, content, tool_name, scope,
                    created_by, topic_key, normalized_hash, revision_count, duplicate_count,
                    last_seen_at, created_at, updated_at, deleted_at, review_after, code_anchor
             FROM observations ORDER BY id",
        )
        .map_err(storage_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArchivedObservation {
                id: row.get(0)?,
                sync_id: row.get(1)?,
                session_id: row.get(2)?,
                r#type: row.get(3)?,
                title: row.get(4)?,
                content: row.get(5)?,
                tool_name: row.get(6)?,
                scope: row.get(7)?,
                created_by: row.get(8)?,
                topic_key: row.get(9)?,
                normalized_hash: row.get(10)?,
                revision_count: row.get(11)?,
                duplicate_count: row.get(12)?,
                last_seen_at: row.get(13)?,
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
                deleted_at: row.get(16)?,
                review_after: row.get(17)?,
                code_anchor: row.get(18)?,
            })
        })
        .map_err(storage_err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(storage_err)
}

fn dump_prompts(conn: &Connection) -> Result<Vec<ArchivedPrompt>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, sync_id, session_id, content, created_at FROM user_prompts ORDER BY id",
        )
        .map_err(storage_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArchivedPrompt {
                id: row.get(0)?,
                sync_id: row.get(1)?,
                session_id: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(storage_err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(storage_err)
}

fn dump_facts(conn: &Connection) -> Result<Vec<ArchivedFact>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, obs_id, subject, predicate, object, temporal, salience,
                    superseded_by, extracted_by, extracted_at
             FROM facts ORDER BY id",
        )
        .map_err(storage_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArchivedFact {
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
        })
        .map_err(storage_err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(storage_err)
}

fn dump_relations(conn: &Connection) -> Result<Vec<ArchivedRelation>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, source_id, target_id, relation_type, confidence, created_at
             FROM observation_relations ORDER BY id",
        )
        .map_err(storage_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArchivedRelation {
                id: row.get(0)?,
                source_id: row.get(1)?,
                target_id: row.get(2)?,
                relation_type: row.get(3)?,
                confidence: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(storage_err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(storage_err)
}

/// Apply a payload. `mode` is `"merge"` (default) or `"replace"`.
pub fn apply_payload(
    conn: &mut Connection,
    payload: &ArchivePayload,
    mode: &str,
) -> Result<ApplyReport> {
    let mode = if mode.trim().is_empty() {
        "merge"
    } else {
        mode.trim()
    };
    if mode != "merge" && mode != "replace" {
        return Err(SyncError::Storage(Error::invalid(format!(
            "unknown import mode '{mode}' (expected merge or replace)"
        ))));
    }

    let tx = conn
        .transaction()
        .map_err(storage_err)?;

    if mode == "replace" {
        wipe_project(&tx)?;
    }

    let mut report = ApplyReport::default();
    let mut obs_id_map: HashMap<i64, i64> = HashMap::new();

    for s in &payload.sessions {
        let existed: Option<String> = tx
            .query_row(
                "SELECT id FROM sessions WHERE id = ?1",
                params![&s.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)?;
        if existed.is_some() {
            tx.execute(
                "UPDATE sessions SET directory = ?2, started_at = ?3, ended_at = ?4, summary = ?5
                 WHERE id = ?1",
                params![&s.id, &s.directory, &s.started_at, &s.ended_at, &s.summary],
            )
            .map_err(storage_err)?;
            report.skipped += 1;
        } else {
            tx.execute(
                "INSERT INTO sessions (id, directory, started_at, ended_at, summary)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![&s.id, &s.directory, &s.started_at, &s.ended_at, &s.summary],
            )
            .map_err(storage_err)?;
            report.sessions_imported += 1;
        }
    }

    for o in &payload.observations {
        ensure_session(&tx, &o.session_id)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM observations WHERE sync_id = ?1",
                params![&o.sync_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)?;
        if let Some(id) = existing {
            tx.execute(
                "UPDATE observations SET
                    session_id = ?2, type = ?3, title = ?4, content = ?5, tool_name = ?6,
                    scope = ?7, created_by = ?8, topic_key = ?9, normalized_hash = ?10,
                    revision_count = ?11, duplicate_count = ?12, last_seen_at = ?13,
                    created_at = ?14, updated_at = ?15, deleted_at = ?16, review_after = ?17,
                    code_anchor = ?18
                 WHERE id = ?1",
                params![
                    id,
                    &o.session_id,
                    &o.r#type,
                    &o.title,
                    &o.content,
                    &o.tool_name,
                    &o.scope,
                    &o.created_by,
                    &o.topic_key,
                    &o.normalized_hash,
                    o.revision_count,
                    o.duplicate_count,
                    &o.last_seen_at,
                    &o.created_at,
                    &o.updated_at,
                    &o.deleted_at,
                    &o.review_after,
                    &o.code_anchor,
                ],
            )
            .map_err(storage_err)?;
            obs_id_map.insert(o.id, id);
            report.skipped += 1;
        } else {
            tx.execute(
                "INSERT INTO observations
                    (sync_id, session_id, type, title, content, tool_name, scope,
                     created_by, topic_key, normalized_hash, revision_count, duplicate_count,
                     last_seen_at, created_at, updated_at, deleted_at, review_after, code_anchor)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                params![
                    &o.sync_id,
                    &o.session_id,
                    &o.r#type,
                    &o.title,
                    &o.content,
                    &o.tool_name,
                    &o.scope,
                    &o.created_by,
                    &o.topic_key,
                    &o.normalized_hash,
                    o.revision_count,
                    o.duplicate_count,
                    &o.last_seen_at,
                    &o.created_at,
                    &o.updated_at,
                    &o.deleted_at,
                    &o.review_after,
                    &o.code_anchor,
                ],
            )
            .map_err(storage_err)?;
            let new_id = tx.last_insert_rowid();
            obs_id_map.insert(o.id, new_id);
            report.observations_imported += 1;
        }
    }

    let mut fact_id_map: HashMap<i64, i64> = HashMap::new();
    for f in &payload.facts {
        let Some(&obs_id) = obs_id_map.get(&f.obs_id) else {
            report.skipped += 1;
            continue;
        };
        tx.execute(
            "INSERT INTO facts
                (obs_id, subject, predicate, object, temporal, salience, extracted_by, extracted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                obs_id,
                &f.subject,
                &f.predicate,
                &f.object,
                &f.temporal,
                f.salience,
                &f.extracted_by,
                &f.extracted_at,
            ],
        )
        .map_err(storage_err)?;
        fact_id_map.insert(f.id, tx.last_insert_rowid());
        report.facts_imported += 1;
    }
    for f in &payload.facts {
        if let (Some(old_super), Some(&new_id)) = (f.superseded_by, fact_id_map.get(&f.id)) {
            if let Some(&new_super) = fact_id_map.get(&old_super) {
                tx.execute(
                    "UPDATE facts SET superseded_by = ?1 WHERE id = ?2",
                    params![new_super, new_id],
                )
                .map_err(storage_err)?;
            }
        }
    }

    for p in &payload.prompts {
        ensure_session(&tx, &p.session_id)?;
        let existed: Option<i64> = tx
            .query_row(
                "SELECT id FROM user_prompts WHERE sync_id = ?1",
                params![&p.sync_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)?;
        if existed.is_some() {
            report.skipped += 1;
            continue;
        }
        tx.execute(
            "INSERT INTO user_prompts (sync_id, session_id, content, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![&p.sync_id, &p.session_id, &p.content, &p.created_at],
        )
        .map_err(storage_err)?;
        report.prompts_imported += 1;
    }

    for r in &payload.relations {
        let (Some(&src), Some(&tgt)) = (obs_id_map.get(&r.source_id), obs_id_map.get(&r.target_id))
        else {
            report.skipped += 1;
            continue;
        };
        tx.execute(
            "INSERT INTO observation_relations
                (source_id, target_id, relation_type, confidence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_id, target_id, relation_type) DO UPDATE SET
                confidence = excluded.confidence,
                created_at = excluded.created_at",
            params![src, tgt, &r.relation_type, r.confidence, &r.created_at],
        )
        .map_err(storage_err)?;
        report.relations_imported += 1;
    }

    tx.commit().map_err(storage_err)?;
    Ok(report)
}

fn ensure_session(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO sessions (id, directory) VALUES (?1, '')",
        params![id],
    )
    .map_err(storage_err)?;
    Ok(())
}

fn wipe_project(conn: &Connection) -> Result<()> {
    for sql in [
        "DELETE FROM observation_relations",
        "DELETE FROM facts",
        "DELETE FROM observation_embedding_meta",
        "DELETE FROM user_prompts",
        "DELETE FROM observations",
        "DELETE FROM sessions",
    ] {
        conn.execute(sql, []).map_err(storage_err)?;
    }
    Ok(())
}

/// Observation ids that were inserted or updated, for embed re-queue.
pub fn imported_observation_ids(conn: &Connection) -> Result<Vec<(i64, String, String)>> {
    let mut stmt = conn
        .prepare("SELECT id, title, content FROM observations WHERE deleted_at IS NULL")
        .map_err(storage_err)?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(storage_err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(storage_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_db() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let mut conn = memlayer_storage::db::open_write(&path).unwrap();
        memlayer_storage::run_migrations(&mut conn).unwrap();
        (dir, conn)
    }

    fn seed(conn: &Connection) {
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observations (sync_id, session_id, type, title, content)
             VALUES ('sync-a', 's1', 'note', 'Title A', 'Body A')",
            [],
        )
        .unwrap();
        let obs_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO facts (obs_id, subject, predicate, object, extracted_by)
             VALUES (?1, 'team', 'chose', 'pgx', 'haiku')",
            params![obs_id],
        )
        .unwrap();
    }

    #[test]
    fn dump_apply_merge_round_trip() {
        let (_dir, src) = open_db();
        seed(&src);
        let payload = dump_payload(&src, "demo", "2026-09-12T00:00:00Z").unwrap();
        assert_eq!(payload.observations.len(), 1);
        assert_eq!(payload.facts.len(), 1);

        let (_dir2, mut dst) = open_db();
        let report = apply_payload(&mut dst, &payload, "merge").unwrap();
        assert_eq!(report.observations_imported, 1);
        assert_eq!(report.facts_imported, 1);
        let n: i64 = dst
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        let again = apply_payload(&mut dst, &payload, "merge").unwrap();
        assert_eq!(again.observations_imported, 0);
        assert!(again.skipped >= 1);
        let n2: i64 = dst
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 1);
    }

    #[test]
    fn replace_wipes_existing() {
        let (_dir, src) = open_db();
        seed(&src);
        let payload = dump_payload(&src, "demo", "2026-09-12T00:00:00Z").unwrap();

        let (_dir2, mut dst) = open_db();
        dst.execute(
            "INSERT INTO sessions (id, directory) VALUES ('old', '/x')",
            [],
        )
        .unwrap();
        dst.execute(
            "INSERT INTO observations (sync_id, session_id, type, title, content)
             VALUES ('other', 'old', 'note', 'Keep me', 'nope')",
            [],
        )
        .unwrap();
        apply_payload(&mut dst, &payload, "replace").unwrap();
        let titles: Vec<String> = dst
            .prepare("SELECT title FROM observations")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(titles, vec!["Title A".to_string()]);
    }
}
