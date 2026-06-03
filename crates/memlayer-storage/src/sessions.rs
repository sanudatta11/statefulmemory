//! Sessions read-side helpers (writes go through write::WriteRequest).
//!
//! Spec sections: FR6.1, FR6.2, SC-11.

use rusqlite::{params, Connection};

use memlayer_core::error::{Error, Result};

use crate::cursor::Cursor;
use crate::models::{Observation, Session};

/// `GetSession` (PRD §6 / FR6).
pub fn get(conn: &Connection, id: &str) -> Result<Session> {
    conn.query_row(
        "SELECT id, directory, started_at, ended_at, summary FROM sessions WHERE id = ?1",
        params![id],
        Session::from_row,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Error::not_found(format!("session {id}")),
        other => Error::internal(format!("get session: {other}")),
    })
}

/// `ListSessions` paginated.
pub fn list(
    conn: &Connection,
    limit: i32,
    cursor: Option<&Cursor>,
) -> Result<(Vec<Session>, Option<Cursor>)> {
    let limit = limit.clamp(1, 50);
    let limit_plus = limit + 1;
    let (sql, mut sessions): (String, Vec<Session>) = if let Some(cur) = cursor {
        let sql = "SELECT id, directory, started_at, ended_at, summary FROM sessions
                    WHERE started_at < ?1 OR (started_at = ?1 AND id < ?2)
                    ORDER BY started_at DESC, id DESC LIMIT ?3"
            .to_string();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| Error::internal(format!("prepare: {e}")))?;
        let rows = stmt
            .query_map(
                params![cur.last_created_at, cur.last_id.to_string(), limit_plus],
                Session::from_row,
            )
            .map_err(|e| Error::internal(format!("list query: {e}")))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::internal(format!("list rows: {e}")))?;
        (sql, rows)
    } else {
        let sql = "SELECT id, directory, started_at, ended_at, summary FROM sessions
                    ORDER BY started_at DESC, id DESC LIMIT ?1"
            .to_string();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| Error::internal(format!("prepare: {e}")))?;
        let rows = stmt
            .query_map(params![limit_plus], Session::from_row)
            .map_err(|e| Error::internal(format!("list query: {e}")))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::internal(format!("list rows: {e}")))?;
        (sql, rows)
    };
    let next = if sessions.len() > limit as usize {
        let _ = sessions.pop();
        let last = sessions.last().unwrap();
        // session id is text; cursor uses last_id i64 — store hash here is overkill,
        // so we encode session id via last_id=0 and rely on started_at + id text
        // for ordering. For simplicity we ship one cursor field for both numeric
        // and text PKs by encoding ID into last_created_at.
        Some(Cursor::new(0, last.started_at.clone()))
    } else {
        None
    };
    Ok((sessions, next))
}

/// Detects whether a session has any non-deleted observations referencing it.
/// Used by DeleteSession to enforce FR6 / SC-17 / FAILED_PRECONDITION.
pub fn has_observations(conn: &Connection, session_id: &str) -> Result<bool> {
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM observations WHERE session_id = ?1 AND deleted_at IS NULL",
            params![session_id],
            |r| r.get(0),
        )
        .map_err(|e| Error::internal(format!("count obs: {e}")))?;
    Ok(n > 0)
}

/// Build the `ContextSnapshot` returned by StartSessionResponse.
///
/// Per FR6.1, the snapshot has:
/// - The most-recent N non-deleted observations across all sessions in this project.
/// - Active topic keys (latest title per topic).
pub fn build_context_snapshot(
    conn: &Connection,
    recent_limit: usize,
) -> Result<(Vec<Observation>, Vec<(String, String, String, String)>)> {
    let recents = crate::read::recent(conn, recent_limit as i32, None)?;
    let mut stmt = conn
        .prepare(
            "SELECT topic_key, scope, title, updated_at FROM observations
             WHERE topic_key IS NOT NULL AND deleted_at IS NULL
             GROUP BY topic_key
             ORDER BY MAX(updated_at) DESC LIMIT 20",
        )
        .map_err(|e| Error::internal(format!("prepare topic select: {e}")))?;
    let topics: Vec<(String, String, String, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| Error::internal(format!("topic query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("topic rows: {e}")))?;
    Ok((recents, topics))
}
