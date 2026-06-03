//! Read path: Get / Search / List / Recent observations.
//!
//! Spec sections: FR5, SC-8/9/10/12, EC-7/8/10.
//!
//! All read queries filter `deleted_at IS NULL` so soft-deleted rows are
//! invisible across the board (SC-12). FTS5 search uses BM25 ranking.

use rusqlite::{params, params_from_iter, Connection, ToSql};
use tracing::warn;

use memlayer_core::error::{Error, Result};

use crate::cursor::Cursor;
use crate::models::Observation;
use crate::write::ObservationKey;

/// Default page size for List/Recent.
pub const DEFAULT_LIMIT: i32 = 10;
/// Max page size for List/Recent (PRD §6.4).
pub const MAX_LIMIT: i32 = 50;
/// Cap for `--all-projects` (EC-10).
pub const ALL_PROJECTS_CAP: usize = 32;

const SELECT_COLS: &str = "id, sync_id, session_id, type, title, content, tool_name, scope,
    created_by, topic_key, normalized_hash, revision_count, duplicate_count,
    last_seen_at, created_at, updated_at, deleted_at, review_after";

/// `GetObservation` — fetch a single row by id or sync_id.
pub fn get(conn: &Connection, key: &ObservationKey) -> Result<Observation> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM observations WHERE {} AND deleted_at IS NULL",
        match key {
            ObservationKey::Id(_) => "id = ?1",
            ObservationKey::SyncId(_) => "sync_id = ?1",
        }
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare get: {e}")))?;
    let result = match key {
        ObservationKey::Id(id) => stmt.query_row(params![id], Observation::from_row),
        ObservationKey::SyncId(s) => stmt.query_row(params![s], Observation::from_row),
    };
    result.map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Error::not_found("observation"),
        other => Error::internal(format!("get: {other}")),
    })
}

/// FTS5 BM25 search over `observations_fts` for one project.
///
/// `query` is passed to FTS5 directly. Special characters that would otherwise
/// break the FTS5 grammar are quoted (FR-EC-2 / TS-22).
pub fn search(
    conn: &Connection,
    query: &str,
    type_filter: Option<&str>,
    scope_filter: Option<&str>,
    limit: i32,
) -> Result<Vec<Observation>> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let safe_query = sanitize_fts5_query(query);
    let mut sql = format!(
        "SELECT {SELECT_COLS} FROM observations
          WHERE id IN (
              SELECT rowid FROM observations_fts WHERE observations_fts MATCH ?1
              ORDER BY bm25(observations_fts) ASC LIMIT ?2
          )
          AND deleted_at IS NULL",
    );
    let mut bind: Vec<Box<dyn ToSql>> = vec![Box::new(safe_query), Box::new(limit)];
    if let Some(t) = type_filter {
        sql.push_str(" AND type = ?3");
        bind.push(Box::new(t.to_string()));
    }
    if let Some(s) = scope_filter {
        let pos = bind.len() + 1;
        sql.push_str(&format!(" AND scope = ?{pos}"));
        bind.push(Box::new(s.to_string()));
    }
    sql.push_str(" ORDER BY id DESC");
    let mut stmt = conn.prepare(&sql).map_err(|e| Error::internal(format!("prepare search: {e}")))?;
    let bind_refs: Vec<&dyn ToSql> = bind.iter().map(|b| &**b as &dyn ToSql).collect();
    let rows = stmt
        .query_map(params_from_iter(bind_refs), Observation::from_row)
        .map_err(|e| Error::internal(format!("search query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("search row: {e}")))?);
    }
    Ok(out)
}

/// Cross-project search via ATTACH on up to 32 most-recently-active projects.
///
/// Returns the merged ranked hits and an optional warning if some projects
/// were dropped (EC-10).
pub fn search_all_projects(
    main_conn: &Connection,
    other_db_paths: &[(String, std::path::PathBuf)],
    query: &str,
    limit: i32,
) -> Result<(Vec<Observation>, Option<String>)> {
    let mut warning = None;
    let mut chosen: Vec<&(String, std::path::PathBuf)> =
        other_db_paths.iter().take(ALL_PROJECTS_CAP).collect();
    if other_db_paths.len() > ALL_PROJECTS_CAP {
        warning = Some(format!(
            "search capped at {ALL_PROJECTS_CAP} projects ({} total)",
            other_db_paths.len()
        ));
    }

    // Attach.
    for (i, (_pname, path)) in chosen.iter().enumerate() {
        let alias = format!("p{i}");
        let sql = format!("ATTACH DATABASE ?1 AS {alias}");
        if let Err(e) = main_conn.execute(&sql, params![path.to_string_lossy().to_string()]) {
            warn!(?path, "attach failed: {e}");
            chosen.truncate(i);
            break;
        }
    }

    // Build a UNION ALL across each attached DB's FTS5 hit list.
    let safe_query = sanitize_fts5_query(query);
    let mut subqueries: Vec<String> = Vec::with_capacity(chosen.len() + 1);
    // The "main" connection itself is the first project's DB.
    subqueries.push(format!(
        "SELECT {SELECT_COLS} FROM observations
            WHERE id IN (SELECT rowid FROM observations_fts
                         WHERE observations_fts MATCH ?1
                         ORDER BY bm25(observations_fts) ASC LIMIT ?2)
              AND deleted_at IS NULL"
    ));
    for i in 0..chosen.len() {
        let alias = format!("p{i}");
        subqueries.push(format!(
            "SELECT {SELECT_COLS} FROM {alias}.observations
                WHERE id IN (SELECT rowid FROM {alias}.observations_fts
                             WHERE {alias}.observations_fts MATCH ?1
                             ORDER BY bm25({alias}.observations_fts) ASC LIMIT ?2)
                  AND deleted_at IS NULL"
        ));
    }
    let union = subqueries.join(" UNION ALL ");
    let sql = format!("SELECT * FROM ({union}) ORDER BY updated_at DESC LIMIT ?2");

    let mut stmt = main_conn.prepare(&sql).map_err(|e| Error::internal(format!("prepare merge: {e}")))?;
    let mut out = Vec::new();
    let mut rows = stmt
        .query(params![safe_query, limit.clamp(1, MAX_LIMIT)])
        .map_err(|e| Error::internal(format!("query merge: {e}")))?;
    while let Some(row) = rows.next().map_err(|e| Error::internal(format!("merge row: {e}")))? {
        out.push(Observation::from_row(row).map_err(|e| Error::internal(format!("decode: {e}")))?);
    }

    // Detach for a clean state.
    for i in 0..chosen.len() {
        let alias = format!("p{i}");
        let _ = main_conn.execute(&format!("DETACH DATABASE {alias}"), []);
    }

    Ok((out, warning))
}

/// `ListObservations`. Cursor-based: rows are ordered `created_at DESC, id DESC`
/// and `(last_created_at, last_id)` is the high-water mark.
pub fn list(
    conn: &Connection,
    type_filter: Option<&str>,
    scope_filter: Option<&str>,
    created_by_filter: Option<&str>,
    due_for_review: bool,
    limit: i32,
    cursor: Option<&Cursor>,
) -> Result<(Vec<Observation>, Option<Cursor>)> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut where_clauses: Vec<String> = vec!["deleted_at IS NULL".into()];
    let mut bind: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(t) = type_filter {
        where_clauses.push(format!("type = ?{}", bind.len() + 1));
        bind.push(Box::new(t.to_string()));
    }
    if let Some(s) = scope_filter {
        where_clauses.push(format!("scope = ?{}", bind.len() + 1));
        bind.push(Box::new(s.to_string()));
    }
    if let Some(c) = created_by_filter {
        where_clauses.push(format!("created_by = ?{}", bind.len() + 1));
        bind.push(Box::new(c.to_string()));
    }
    if due_for_review {
        where_clauses.push("review_after IS NOT NULL AND review_after <= datetime('now')".into());
    }
    if let Some(cur) = cursor {
        let p1 = bind.len() + 1;
        let p2 = bind.len() + 2;
        where_clauses.push(format!(
            "(created_at < ?{p1} OR (created_at = ?{p1} AND id < ?{p2}))",
        ));
        bind.push(Box::new(cur.last_created_at.clone()));
        bind.push(Box::new(cur.last_id));
    }
    // Fetch limit+1 to know if there's a next page.
    let limit_plus = limit + 1;
    let pos = bind.len() + 1;
    bind.push(Box::new(limit_plus));
    let sql = format!(
        "SELECT {SELECT_COLS} FROM observations
          WHERE {} ORDER BY created_at DESC, id DESC LIMIT ?{pos}",
        where_clauses.join(" AND ")
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| Error::internal(format!("prepare list: {e}")))?;
    let bind_refs: Vec<&dyn ToSql> = bind.iter().map(|b| &**b as &dyn ToSql).collect();
    let mut rows: Vec<Observation> = stmt
        .query_map(params_from_iter(bind_refs), Observation::from_row)
        .map_err(|e| Error::internal(format!("list query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("list rows: {e}")))?;

    let next = if rows.len() > limit as usize {
        let last = rows.pop().unwrap();
        // Use the last *included* row, not the trimmed one.
        let last_kept = rows.last().unwrap();
        let _ = last;
        Some(Cursor::new(last_kept.id, last_kept.created_at.clone()))
    } else {
        None
    };
    Ok((rows, next))
}

/// `RecentObservations` — last N rows by `created_at`.
pub fn recent(
    conn: &Connection,
    limit: i32,
    scope_filter: Option<&str>,
) -> Result<Vec<Observation>> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let (sql, bound): (String, Vec<Box<dyn ToSql>>) = if let Some(s) = scope_filter {
        (
            format!(
                "SELECT {SELECT_COLS} FROM observations
                  WHERE deleted_at IS NULL AND scope = ?1
                  ORDER BY created_at DESC LIMIT ?2"
            ),
            vec![Box::new(s.to_string()), Box::new(limit)],
        )
    } else {
        (
            format!(
                "SELECT {SELECT_COLS} FROM observations
                  WHERE deleted_at IS NULL
                  ORDER BY created_at DESC LIMIT ?1"
            ),
            vec![Box::new(limit)],
        )
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| Error::internal(format!("prepare recent: {e}")))?;
    let bind_refs: Vec<&dyn ToSql> = bound.iter().map(|b| &**b as &dyn ToSql).collect();
    stmt.query_map(params_from_iter(bind_refs), Observation::from_row)
        .map_err(|e| Error::internal(format!("recent query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("recent rows: {e}")))
}

/// Escape FTS5-special characters so a user-supplied query never crashes the parser.
///
/// Strategy: wrap the whole query in double-quotes and escape any embedded
/// double-quotes by doubling them. This forces FTS5 to treat the string as a
/// phrase, which is the safest interpretation. (TS-22.)
fn sanitize_fts5_query(q: &str) -> String {
    let escaped = q.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::{handle_save_observation_for_tests, SaveObservationInput};
    use tempfile::TempDir;

    fn make_conn() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let conn = crate::db::open_write(&path).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        (dir, conn)
    }

    fn save(conn: &mut Connection, content: &str, ty: &str) {
        let tx = conn.transaction().unwrap();
        handle_save_observation_for_tests(
            &tx,
            SaveObservationInput {
                sync_id: None,
                session_id: "s1".into(),
                r#type: ty.into(),
                title: format!("title-{content}"),
                content: content.into(),
                tool_name: None,
                scope: "project".into(),
                created_by: None,
                topic_key: None,
                dedupe_window_secs: 0, // disable dedupe so each row inserts
                max_content_chars: 50_000,
            },
        )
        .unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn sc8_search_returns_hit() {
        let (_d, mut c) = make_conn();
        save(&mut c, "the auth strategy is jwt", "decision");
        save(&mut c, "totally unrelated content", "note");
        let hits = search(&c, "auth", None, None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("auth"));
    }

    #[test]
    fn sc12_soft_deleted_invisible() {
        let (_d, mut c) = make_conn();
        save(&mut c, "alpha unique", "note");
        // Soft-delete it.
        c.execute(
            "UPDATE observations SET deleted_at = datetime('now')",
            [],
        )
        .unwrap();
        assert!(search(&c, "alpha", None, None, 10).unwrap().is_empty());
        assert!(recent(&c, 10, None).unwrap().is_empty());
        assert!(list(&c, None, None, None, false, 10, None).unwrap().0.is_empty());
    }

    #[test]
    fn sc10_pagination_three_pages() {
        let (_d, mut c) = make_conn();
        for i in 0..12 {
            save(&mut c, &format!("row-{i}"), "note");
        }
        let (p1, c1) = list(&c, None, None, None, false, 5, None).unwrap();
        assert_eq!(p1.len(), 5);
        assert!(c1.is_some());
        let (p2, c2) = list(&c, None, None, None, false, 5, c1.as_ref()).unwrap();
        assert_eq!(p2.len(), 5);
        assert!(c2.is_some());
        let (p3, c3) = list(&c, None, None, None, false, 5, c2.as_ref()).unwrap();
        assert_eq!(p3.len(), 2);
        assert!(c3.is_none());
    }

    #[test]
    fn ec7_malformed_cursor_rejected() {
        assert!(crate::cursor::Cursor::decode("@@@").is_err());
    }

    #[test]
    fn search_handles_fts5_special_chars() {
        let (_d, mut c) = make_conn();
        save(&mut c, "with quotes", "note");
        // Should not panic.
        let _ = search(&c, "weird\"chars*+", None, None, 10).unwrap();
    }
}
