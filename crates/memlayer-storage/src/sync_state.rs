// Generated with AI Coding Rules Hub
//! Sync-state helpers: SELECT/mark/upsert/count primitives backing the
//! Spec 3 git-chunk sync pipeline.
//!
//! Spec sections: FR1.1 (item 1, item 7), FR2.1.3, FR6.1, FR6.2, SC-1, SC-2,
//! SC-5, SC-16, EC-7.
//!
//! All functions take an `&Connection`. In production these run on the
//! per-project write thread via `WriteRequest::Custom` (see
//! `crate::write::WriteRequest`); the unit tests below open an in-memory
//! `rusqlite::Connection` directly. Reads only need a read-only handle.
//!
//! Parameterized queries only — never string-format SQL with values.

use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, instrument};

use memlayer_core::error::{Error, Result};

use crate::models::{Observation, Prompt, Session};

/// SQLite has a default compile-time bound on the number of host parameters
/// (`SQLITE_MAX_VARIABLE_NUMBER`); 999 on builds before 3.32 and 32766 on
/// modern builds. We chunk well below the conservative bound to stay safe
/// on every supported rusqlite version.
const MAX_IDS_PER_STATEMENT: usize = 500;

/// Primary keys grouped by table, ready to feed into [`mark_exported`].
#[derive(Debug, Clone, Default)]
pub struct ExportedIds {
    pub observations: Vec<i64>,
    pub sessions: Vec<String>,
    pub prompts: Vec<i64>,
}

impl ExportedIds {
    pub fn is_empty(&self) -> bool {
        self.observations.is_empty() && self.sessions.is_empty() && self.prompts.is_empty()
    }
}

/// Outcome of an upsert against an existing row, used for SC-5 telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// No row with the conflict key existed — incoming row was inserted.
    Inserted,
    /// A row existed, incoming `updated_at` was newer, and the row was overwritten.
    OverwroteOlder,
    /// A row existed and incoming `updated_at` was older or equal — local row kept.
    RetainedNewer,
}

/// SELECT every row whose `exported_at IS NULL`, by table.
///
/// Returns three Vecs (observations, sessions, prompts) ready to be JSONL-serialized
/// by `memlayer-sync::chunk::build_jsonl`. Only the dedicated partial indexes are
/// scanned, so the cost is O(unexported) — independent of total row count (NFR5).
#[instrument(level = "debug", skip(conn))]
pub fn select_unexported_for_export(
    conn: &Connection,
) -> Result<(Vec<Observation>, Vec<Session>, Vec<Prompt>)> {
    // Observations.
    let mut stmt = conn
        .prepare(
            "SELECT id, sync_id, session_id, type, title, content, tool_name, scope, \
                    created_by, topic_key, normalized_hash, revision_count, duplicate_count, \
                    last_seen_at, created_at, updated_at, deleted_at, review_after \
             FROM observations \
             WHERE exported_at IS NULL \
             ORDER BY id ASC",
        )
        .map_err(|e| Error::internal(format!("prepare select_unexported observations: {e}")))?;
    let observations: Vec<Observation> = stmt
        .query_map([], Observation::from_row)
        .map_err(|e| Error::internal(format!("query_map observations: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("collect observations: {e}")))?;

    // Sessions.
    let mut stmt = conn
        .prepare(
            "SELECT id, directory, started_at, ended_at, summary \
             FROM sessions \
             WHERE exported_at IS NULL \
             ORDER BY id ASC",
        )
        .map_err(|e| Error::internal(format!("prepare select_unexported sessions: {e}")))?;
    let sessions: Vec<Session> = stmt
        .query_map([], Session::from_row)
        .map_err(|e| Error::internal(format!("query_map sessions: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("collect sessions: {e}")))?;

    // Prompts.
    let mut stmt = conn
        .prepare(
            "SELECT id, sync_id, session_id, content, created_at \
             FROM user_prompts \
             WHERE exported_at IS NULL \
             ORDER BY id ASC",
        )
        .map_err(|e| Error::internal(format!("prepare select_unexported prompts: {e}")))?;
    let prompts: Vec<Prompt> = stmt
        .query_map([], Prompt::from_row)
        .map_err(|e| Error::internal(format!("query_map prompts: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("collect prompts: {e}")))?;

    debug!(
        observations = observations.len(),
        sessions = sessions.len(),
        prompts = prompts.len(),
        "selected unexported rows"
    );
    Ok((observations, sessions, prompts))
}

/// Mark the given rows as exported by writing `exported_at = ?now` to all
/// matching rows. Chunks the IN-list to stay under the SQLite parameter cap.
///
/// Caller is responsible for executing this on the dedicated write thread
/// and for performing it under the per-project export mutex (Spec §11.3).
#[instrument(level = "debug", skip(conn, ids), fields(
    observations = ids.observations.len(),
    sessions = ids.sessions.len(),
    prompts = ids.prompts.len()
))]
pub fn mark_exported(conn: &Connection, ids: &ExportedIds, now: &str) -> Result<()> {
    mark_exported_chunked_i64(conn, "observations", &ids.observations, now)?;
    mark_exported_chunked_string(conn, "sessions", &ids.sessions, now)?;
    mark_exported_chunked_i64(conn, "user_prompts", &ids.prompts, now)?;
    Ok(())
}

fn mark_exported_chunked_i64(conn: &Connection, table: &str, ids: &[i64], now: &str) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    for chunk in ids.chunks(MAX_IDS_PER_STATEMENT) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        // `now` is bound separately as parameter 1.
        let sql = format!("UPDATE {table} SET exported_at = ?1 WHERE id IN ({placeholders})");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| Error::internal(format!("prepare mark_exported {table}: {e}")))?;
        // Bind parameters: ?1 = now, ?2.. = ids.
        let mut params_iter: Vec<rusqlite::types::Value> = Vec::with_capacity(chunk.len() + 1);
        params_iter.push(rusqlite::types::Value::Text(now.to_string()));
        for id in chunk {
            params_iter.push(rusqlite::types::Value::Integer(*id));
        }
        stmt.execute(rusqlite::params_from_iter(params_iter.iter()))
            .map_err(|e| Error::internal(format!("execute mark_exported {table}: {e}")))?;
    }
    Ok(())
}

fn mark_exported_chunked_string(
    conn: &Connection,
    table: &str,
    ids: &[String],
    now: &str,
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    for chunk in ids.chunks(MAX_IDS_PER_STATEMENT) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!("UPDATE {table} SET exported_at = ?1 WHERE id IN ({placeholders})");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| Error::internal(format!("prepare mark_exported {table}: {e}")))?;
        let mut params_iter: Vec<rusqlite::types::Value> = Vec::with_capacity(chunk.len() + 1);
        params_iter.push(rusqlite::types::Value::Text(now.to_string()));
        for id in chunk {
            params_iter.push(rusqlite::types::Value::Text(id.clone()));
        }
        stmt.execute(rusqlite::params_from_iter(params_iter.iter()))
            .map_err(|e| Error::internal(format!("execute mark_exported {table}: {e}")))?;
    }
    Ok(())
}

/// Has the given chunk_id already been imported into this DB?
///
/// Used by the import pipeline to skip already-applied chunks (SC-4).
#[instrument(level = "debug", skip(conn))]
pub fn chunk_exists(conn: &Connection, chunk_id: &str) -> Result<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sync_chunks WHERE chunk_id = ?1",
            params![chunk_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| Error::internal(format!("chunk_exists: {e}")))?;
    Ok(exists.is_some())
}

/// Record that the given chunk_id has been imported. Idempotent: a second
/// call for the same id is a no-op (SC-4).
#[instrument(level = "debug", skip(conn))]
pub fn record_chunk_imported(conn: &Connection, chunk_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO sync_chunks (chunk_id, imported_at) VALUES (?1, datetime('now'))",
        params![chunk_id],
    )
    .map_err(|e| Error::internal(format!("record_chunk_imported: {e}")))?;
    Ok(())
}

/// Upsert an observation arriving from a remote chunk.
///
/// Conflict resolution per FR2.1.3 / SC-5:
/// - No existing row with the same `sync_id` → [`UpsertOutcome::Inserted`].
/// - Existing row, incoming `updated_at > existing.updated_at` → overwrite,
///   return [`UpsertOutcome::OverwroteOlder`].
/// - Otherwise → keep the local row, return [`UpsertOutcome::RetainedNewer`].
#[instrument(level = "debug", skip(conn, obs), fields(sync_id = %obs.sync_id))]
pub fn upsert_observation_from_remote(
    conn: &Connection,
    obs: &Observation,
) -> Result<UpsertOutcome> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT updated_at FROM observations WHERE sync_id = ?1",
            params![obs.sync_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| Error::internal(format!("select existing observation: {e}")))?;

    match existing {
        None => {
            conn.execute(
                "INSERT INTO observations \
                 (sync_id, session_id, type, title, content, tool_name, scope, created_by, \
                  topic_key, normalized_hash, revision_count, duplicate_count, last_seen_at, \
                  created_at, updated_at, deleted_at, review_after) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    obs.sync_id,
                    obs.session_id,
                    obs.r#type,
                    obs.title,
                    obs.content,
                    obs.tool_name,
                    obs.scope,
                    obs.created_by,
                    obs.topic_key,
                    obs.normalized_hash,
                    obs.revision_count,
                    obs.duplicate_count,
                    obs.last_seen_at,
                    obs.created_at,
                    obs.updated_at,
                    obs.deleted_at,
                    obs.review_after,
                ],
            )
            .map_err(|e| Error::internal(format!("insert observation: {e}")))?;
            Ok(UpsertOutcome::Inserted)
        }
        Some(local_updated_at) => {
            if obs.updated_at > local_updated_at {
                conn.execute(
                    "UPDATE observations SET \
                       session_id = ?2, type = ?3, title = ?4, content = ?5, tool_name = ?6, \
                       scope = ?7, created_by = ?8, topic_key = ?9, normalized_hash = ?10, \
                       revision_count = ?11, duplicate_count = ?12, last_seen_at = ?13, \
                       created_at = ?14, updated_at = ?15, deleted_at = ?16, review_after = ?17 \
                     WHERE sync_id = ?1",
                    params![
                        obs.sync_id,
                        obs.session_id,
                        obs.r#type,
                        obs.title,
                        obs.content,
                        obs.tool_name,
                        obs.scope,
                        obs.created_by,
                        obs.topic_key,
                        obs.normalized_hash,
                        obs.revision_count,
                        obs.duplicate_count,
                        obs.last_seen_at,
                        obs.created_at,
                        obs.updated_at,
                        obs.deleted_at,
                        obs.review_after,
                    ],
                )
                .map_err(|e| Error::internal(format!("update observation: {e}")))?;
                Ok(UpsertOutcome::OverwroteOlder)
            } else {
                Ok(UpsertOutcome::RetainedNewer)
            }
        }
    }
}

/// Upsert a session arriving from a remote chunk.
///
/// Sessions don't carry an `updated_at` column. The "later wins" semantic is
/// approximated via `ended_at` and `summary`: an incoming session with
/// `ended_at IS NOT NULL` is considered newer than a local row with
/// `ended_at IS NULL`. If both are ended, the larger `ended_at` wins. If
/// nothing distinguishes the two, the local row is retained.
#[instrument(level = "debug", skip(conn, sess), fields(id = %sess.id))]
pub fn upsert_session_from_remote(conn: &Connection, sess: &Session) -> Result<UpsertOutcome> {
    let existing: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT ended_at, summary FROM sessions WHERE id = ?1",
            params![sess.id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(|e| Error::internal(format!("select existing session: {e}")))?;

    match existing {
        None => {
            conn.execute(
                "INSERT INTO sessions (id, directory, started_at, ended_at, summary) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    sess.id,
                    sess.directory,
                    sess.started_at,
                    sess.ended_at,
                    sess.summary,
                ],
            )
            .map_err(|e| Error::internal(format!("insert session: {e}")))?;
            Ok(UpsertOutcome::Inserted)
        }
        Some((local_ended_at, _local_summary)) => {
            let incoming_newer = match (&sess.ended_at, &local_ended_at) {
                (Some(_), None) => true,
                (Some(inc), Some(loc)) => inc > loc,
                _ => false,
            };
            if incoming_newer {
                conn.execute(
                    "UPDATE sessions SET directory = ?2, started_at = ?3, ended_at = ?4, summary = ?5 \
                     WHERE id = ?1",
                    params![
                        sess.id,
                        sess.directory,
                        sess.started_at,
                        sess.ended_at,
                        sess.summary,
                    ],
                )
                .map_err(|e| Error::internal(format!("update session: {e}")))?;
                Ok(UpsertOutcome::OverwroteOlder)
            } else {
                Ok(UpsertOutcome::RetainedNewer)
            }
        }
    }
}

/// Upsert a prompt arriving from a remote chunk.
///
/// Prompts are immutable in the v1 schema (only `created_at`, no `updated_at`).
/// Conflict resolution: insert-or-ignore keyed on `sync_id`. Existing row →
/// `RetainedNewer`. Absent row → `Inserted`. We never overwrite a prompt.
#[instrument(level = "debug", skip(conn, prompt), fields(sync_id = %prompt.sync_id))]
pub fn upsert_prompt_from_remote(conn: &Connection, prompt: &Prompt) -> Result<UpsertOutcome> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM user_prompts WHERE sync_id = ?1",
            params![prompt.sync_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| Error::internal(format!("select existing prompt: {e}")))?;

    match existing {
        None => {
            conn.execute(
                "INSERT INTO user_prompts (sync_id, session_id, content, created_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    prompt.sync_id,
                    prompt.session_id,
                    prompt.content,
                    prompt.created_at
                ],
            )
            .map_err(|e| Error::internal(format!("insert prompt: {e}")))?;
            Ok(UpsertOutcome::Inserted)
        }
        Some(_) => Ok(UpsertOutcome::RetainedNewer),
    }
}

/// Count of chunks this DB has produced via `SyncExport`.
///
/// **v1 stub.** The v1 `sync_chunks` table only records *imported* chunks.
/// There is no DB-side record of self-authored chunks; the manifest at
/// `<repo>/.memlayer/manifest.json` is the source of truth. `SyncStatus`
/// (spec-task-3 / spec-task-6) reads that manifest directly. We keep this
/// function in the public API for future symmetry but return 0 today.
///
/// TODO(spec-task-6): if a DB-side counter becomes useful, add a
/// `kind` column to `sync_chunks` in a future migration.
#[instrument(level = "debug", skip_all)]
pub fn count_exported_chunks(_conn: &Connection) -> Result<i64> {
    debug!("count_exported_chunks v1 stub returning 0; manifest is the source of truth");
    Ok(0)
}

/// Count of chunks imported into this DB.
#[instrument(level = "debug", skip(conn))]
pub fn count_imported_chunks(conn: &Connection) -> Result<i64> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM sync_chunks", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("count_imported_chunks: {e}")))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Open an in-memory connection and apply both V1 and V2 by raw `execute_batch`.
    /// Avoids depending on the refinery runner (which would require a file path).
    fn open_test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory");
        let v1 = include_str!("../../../migrations/V1__init.sql");
        let v2 = include_str!("../../../migrations/V2__export_tracking.sql");
        conn.execute_batch(v1).expect("apply V1");
        conn.execute_batch(v2).expect("apply V2");
        conn
    }

    /// Helper: insert a session row directly. Returns the session id.
    fn insert_session(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO sessions (id, directory, started_at) VALUES (?1, '/tmp', '2026-01-01T00:00:00Z')",
            params![id],
        )
        .unwrap();
    }

    /// Helper: insert an observation. Returns the auto-incremented id.
    fn insert_obs(conn: &Connection, sync_id: &str, session_id: &str, updated_at: &str) -> i64 {
        conn.execute(
            "INSERT INTO observations \
               (sync_id, session_id, type, title, content, scope, revision_count, duplicate_count, \
                created_at, updated_at) \
             VALUES (?1, ?2, 'note', 't', 'c', 'project', 1, 1, ?3, ?3)",
            params![sync_id, session_id, updated_at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_prompt(conn: &Connection, sync_id: &str, session_id: &str) -> i64 {
        conn.execute(
            "INSERT INTO user_prompts (sync_id, session_id, content, created_at) \
             VALUES (?1, ?2, 'hi', '2026-01-01T00:00:00Z')",
            params![sync_id, session_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn v2_migration_preserves_rows_with_null_exported_at() {
        // TS-14, SC-16, FR6.2: pre-V2 rows survive V2 with exported_at = NULL.
        let conn = Connection::open_in_memory().unwrap();
        let v1 = include_str!("../../../migrations/V1__init.sql");
        conn.execute_batch(v1).unwrap();
        insert_session(&conn, "s1");
        insert_obs(&conn, "obs-a", "s1", "2026-01-01T00:00:00Z");
        insert_prompt(&conn, "p-a", "s1");

        // Apply V2.
        let v2 = include_str!("../../../migrations/V2__export_tracking.sql");
        conn.execute_batch(v2).unwrap();

        // schema_meta bumped.
        let v: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key='version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "2");

        // exported_at IS NULL on every pre-existing row.
        let null_obs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE exported_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_obs, 1);
        let null_sess: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE exported_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_sess, 1);
        let null_prompts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_prompts WHERE exported_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_prompts, 1);
    }

    #[test]
    fn select_unexported_excludes_marked_rows() {
        // SC-2: after mark_exported, the row no longer appears in select_unexported.
        let conn = open_test_conn();
        insert_session(&conn, "s1");
        let id_a = insert_obs(&conn, "obs-a", "s1", "2026-01-01T00:00:00Z");
        let _id_b = insert_obs(&conn, "obs-b", "s1", "2026-01-02T00:00:00Z");

        let (obs0, _, _) = select_unexported_for_export(&conn).unwrap();
        assert_eq!(obs0.len(), 2);

        let ids = ExportedIds {
            observations: vec![id_a],
            sessions: vec![],
            prompts: vec![],
        };
        mark_exported(&conn, &ids, "2026-06-04T00:00:00Z").unwrap();

        let (obs1, _, _) = select_unexported_for_export(&conn).unwrap();
        assert_eq!(obs1.len(), 1);
        assert_eq!(obs1[0].sync_id, "obs-b");
    }

    #[test]
    fn mark_exported_chunked_above_500_ids() {
        // Proves the chunked UPDATE works for IN-lists larger than the per-statement cap.
        let conn = open_test_conn();
        insert_session(&conn, "s1");
        let mut ids: Vec<i64> = Vec::with_capacity(1200);
        for i in 0..1200 {
            ids.push(insert_obs(
                &conn,
                &format!("obs-{i}"),
                "s1",
                "2026-01-01T00:00:00Z",
            ));
        }
        let exported = ExportedIds {
            observations: ids.clone(),
            sessions: vec![],
            prompts: vec![],
        };
        mark_exported(&conn, &exported, "2026-06-04T00:00:00Z").unwrap();

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE exported_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1200);

        let (left, _, _) = select_unexported_for_export(&conn).unwrap();
        assert_eq!(left.len(), 0);
    }

    #[test]
    fn upsert_observation_inserts_when_absent() {
        // SC-5: previously-unknown sync_id → Inserted.
        let conn = open_test_conn();
        insert_session(&conn, "s1");
        let incoming = Observation {
            id: 0, // ignored on insert (autoincrement)
            sync_id: "obs-x".to_string(),
            session_id: "s1".to_string(),
            r#type: "note".to_string(),
            title: "remote-title".to_string(),
            content: "remote-content".to_string(),
            tool_name: None,
            scope: "project".to_string(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 1,
            duplicate_count: 1,
            last_seen_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            deleted_at: None,
            review_after: None,
        };
        let outcome = upsert_observation_from_remote(&conn, &incoming).unwrap();
        assert_eq!(outcome, UpsertOutcome::Inserted);
        let title: String = conn
            .query_row(
                "SELECT title FROM observations WHERE sync_id = 'obs-x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "remote-title");
    }

    #[test]
    fn upsert_observation_overwrites_when_incoming_newer() {
        // SC-5: incoming.updated_at > local.updated_at → OverwroteOlder + content replaced.
        let conn = open_test_conn();
        insert_session(&conn, "s1");
        insert_obs(&conn, "obs-x", "s1", "2026-01-01T00:00:00Z");

        let incoming = Observation {
            id: 0,
            sync_id: "obs-x".to_string(),
            session_id: "s1".to_string(),
            r#type: "note".to_string(),
            title: "newer-title".to_string(),
            content: "newer-content".to_string(),
            tool_name: None,
            scope: "project".to_string(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 2,
            duplicate_count: 1,
            last_seen_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-02T00:00:00Z".to_string(),
            deleted_at: None,
            review_after: None,
        };
        let outcome = upsert_observation_from_remote(&conn, &incoming).unwrap();
        assert_eq!(outcome, UpsertOutcome::OverwroteOlder);
        let title: String = conn
            .query_row(
                "SELECT title FROM observations WHERE sync_id = 'obs-x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "newer-title");
    }

    #[test]
    fn upsert_observation_retains_when_incoming_older() {
        // SC-5: incoming.updated_at <= local.updated_at → RetainedNewer + row unchanged.
        let conn = open_test_conn();
        insert_session(&conn, "s1");
        insert_obs(&conn, "obs-x", "s1", "2026-06-01T00:00:00Z");

        let incoming = Observation {
            id: 0,
            sync_id: "obs-x".to_string(),
            session_id: "s1".to_string(),
            r#type: "note".to_string(),
            title: "older-title".to_string(),
            content: "older-content".to_string(),
            tool_name: None,
            scope: "project".to_string(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 1,
            duplicate_count: 1,
            last_seen_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-02T00:00:00Z".to_string(),
            deleted_at: None,
            review_after: None,
        };
        let outcome = upsert_observation_from_remote(&conn, &incoming).unwrap();
        assert_eq!(outcome, UpsertOutcome::RetainedNewer);
        let title: String = conn
            .query_row(
                "SELECT title FROM observations WHERE sync_id = 'obs-x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // Original title from insert_obs.
        assert_eq!(title, "t");
    }

    #[test]
    fn chunk_exists_and_record() {
        let conn = open_test_conn();
        assert!(!chunk_exists(&conn, "abcd1234").unwrap());
        record_chunk_imported(&conn, "abcd1234").unwrap();
        assert!(chunk_exists(&conn, "abcd1234").unwrap());
        // Idempotent: a second record_chunk_imported is a no-op.
        record_chunk_imported(&conn, "abcd1234").unwrap();
        assert_eq!(count_imported_chunks(&conn).unwrap(), 1);
    }

    #[test]
    fn count_imported_chunks_increments() {
        // FR3: count_imported_chunks reflects sync_chunks size.
        let conn = open_test_conn();
        for id in &["aaaa", "bbbb", "cccc"] {
            record_chunk_imported(&conn, id).unwrap();
        }
        assert_eq!(count_imported_chunks(&conn).unwrap(), 3);
        // count_exported_chunks is the v1 stub.
        assert_eq!(count_exported_chunks(&conn).unwrap(), 0);
    }
}
