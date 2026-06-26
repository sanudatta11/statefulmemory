//! Read path: Get / Search / List / Recent observations.
//!
//! Spec sections: FR5, SC-8/9/10/12, EC-7/8/10.
//!
//! All read queries filter `deleted_at IS NULL` so soft-deleted rows are
//! invisible across the board (SC-12). FTS5 search uses BM25 ranking.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};
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
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare search: {e}")))?;
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

/// Dense (cosine) top-K against the V4 vec0 vtable. Returns observations
/// in ascending distance (most similar first). Empty result on:
/// - empty `observations_vec` (no embeddings landed yet, falling-back
///   caller treats as a hint to use BM25 only),
/// - query vector dim != 384 (caller's bug; we surface as
///   `InvalidArgument` so it's loud),
/// - any sqlite-vec error (logged, returned as Internal).
///
/// Spec: retrieval-promotion SC-3, P8.
pub fn search_dense(
    conn: &Connection,
    q_vec: &[f32],
    limit: i64,
) -> Result<Vec<Observation>> {
    if q_vec.len() != 384 {
        return Err(Error::invalid(format!(
            "search_dense expects 384-dim query vector, got {}",
            q_vec.len()
        )));
    }
    let limit = limit.clamp(1, MAX_LIMIT as i64);

    // Detect whether this DB is running in int8-quantized mode (Spec 1a).
    // A project is considered "quantized-only" if every meta row has a
    // non-null quantized_blob. Mixed-mode (some quantized, some vec0) falls
    // back to the vec0 path — the quantized rows will have no vec0 rowid and
    // simply won't appear.
    let maybe_all_quantized: bool = conn
        .query_row(
            "SELECT COUNT(*) = 0 FROM observation_embedding_meta \
             WHERE (quantized IS NULL OR quantized = 0) AND observation_id IS NOT NULL",
            [],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false);

    let has_quantized: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM observation_embedding_meta WHERE quantized = 1",
            [],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false);

    if has_quantized && maybe_all_quantized {
        // Brute-force cosine similarity over int8 BLOBs.
        return search_dense_quantized(conn, q_vec, limit);
    }

    let mut blob = Vec::with_capacity(q_vec.len() * 4);
    for f in q_vec {
        blob.extend_from_slice(&f.to_le_bytes());
    }

    // vec0 MATCH ?1 expects the query as a blob; the `k=?2` pseudo-column
    // is how sqlite-vec asks for top-K. Distance is exposed as the
    // `distance` column on the vtable rows.
    let sql = format!(
        "SELECT {SELECT_COLS} FROM observations o
         JOIN observations_vec v ON o.id = v.rowid
         WHERE v.embedding MATCH ?1 AND k = ?2
           AND o.deleted_at IS NULL
         ORDER BY v.distance ASC"
    );

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            // Most common cause: V4 hasn't run yet (no observations_vec
            // table). The hybrid caller treats Ok(empty) as a fall-back
            // signal, so surface that here too.
            tracing::warn!(error = %e, "search_dense prepare failed (vec table missing?) — returning empty");
            return Ok(Vec::new());
        }
    };
    let rows = stmt
        .query_map(params![blob, limit], Observation::from_row)
        .map_err(|e| Error::internal(format!("search_dense query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("search_dense row: {e}")))?);
    }
    Ok(out)
}

/// Brute-force cosine top-K over int8 quantized embeddings stored in
/// `observation_embedding_meta.quantized_blob + scale` (Spec 1a).
fn search_dense_quantized(
    conn: &Connection,
    q_vec: &[f32],
    limit: i64,
) -> Result<Vec<Observation>> {
    use memlayer_embed::quantize::int8_to_f32;

    // Load all quantized rows into memory. At 384 bytes per row, 10K obs ≈
    // 3.8 MB — trivially fits in RAM.
    let mut candidates: Vec<(i64, f32)> = Vec::new(); // (obs_id, cosine_sim)

    let mut stmt = conn
        .prepare(
            "SELECT observation_id, quantized_blob, scale \
             FROM observation_embedding_meta \
             WHERE quantized = 1 AND quantized_blob IS NOT NULL AND scale IS NOT NULL",
        )
        .map_err(|e| Error::internal(format!("search_dense_quantized prepare: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            let obs_id: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            let scale: f32 = row.get(2)?;
            Ok((obs_id, blob, scale))
        })
        .map_err(|e| Error::internal(format!("search_dense_quantized query: {e}")))?;

    for r in rows {
        let (obs_id, blob, scale) = r.map_err(|e| Error::internal(format!("search_dense_quantized row: {e}")))?;
        if blob.len() != 384 {
            continue; // skip malformed rows
        }
        let int8: Vec<i8> = blob.iter().map(|&x| x as i8).collect();
        let f32_vec = int8_to_f32(&int8, scale);
        let sim = cosine_similarity(q_vec, &f32_vec);
        candidates.push((obs_id, sim));
    }

    // Sort by similarity descending, take top-K.
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(limit as usize);

    // Fetch Observation rows for the top-K ids.
    let mut out = Vec::with_capacity(candidates.len());
    for (obs_id, _sim) in candidates {
        let obs: Option<Observation> = conn
            .query_row(
                &format!("SELECT {SELECT_COLS} FROM observations WHERE id = ?1 AND deleted_at IS NULL"),
                params![obs_id],
                Observation::from_row,
            )
            .optional()
            .map_err(|e| Error::internal(format!("search_dense_quantized fetch: {e}")))?;
        if let Some(o) = obs {
            out.push(o);
        }
    }
    Ok(out)
}

/// Cosine similarity between two equal-length f32 slices.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
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

    let mut stmt = main_conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare merge: {e}")))?;
    let mut out = Vec::new();
    let mut rows = stmt
        .query(params![safe_query, limit.clamp(1, MAX_LIMIT)])
        .map_err(|e| Error::internal(format!("query merge: {e}")))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| Error::internal(format!("merge row: {e}")))?
    {
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
    session_id_filter: Option<&str>,
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
    if let Some(sid) = session_id_filter {
        where_clauses.push(format!("session_id = ?{}", bind.len() + 1));
        bind.push(Box::new(sid.to_string()));
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
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare list: {e}")))?;
    let bind_refs: Vec<&dyn ToSql> = bind.iter().map(|b| &**b as &dyn ToSql).collect();
    let row_iter = stmt
        .query_map(params_from_iter(bind_refs), Observation::from_row)
        .map_err(|e| Error::internal(format!("list query: {e}")))?;
    let mut rows: Vec<Observation> = Vec::new();
    for r in row_iter {
        rows.push(r.map_err(|e| Error::internal(format!("list rows: {e}")))?);
    }

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
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare recent: {e}")))?;
    let bind_refs: Vec<&dyn ToSql> = bound.iter().map(|b| &**b as &dyn ToSql).collect();
    let row_iter = stmt
        .query_map(params_from_iter(bind_refs), Observation::from_row)
        .map_err(|e| Error::internal(format!("recent query: {e}")))?;
    let mut out = Vec::new();
    for r in row_iter {
        out.push(r.map_err(|e| Error::internal(format!("recent rows: {e}")))?);
    }
    Ok(out)
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

/// One row of the active-topic summary used by `Context` (FR12.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTopic {
    pub topic_key: String,
    pub scope: String,
    pub latest_title: String,
    pub updated_at: String,
}

/// `Context` (FR12.3): last `limit` active observations + active topic keys
/// with their latest title + updated_at.
///
/// "Active" means `deleted_at IS NULL`. The topic block returns up to 20
/// rows ordered by most-recent activity in any scope.
pub fn recent_active(
    conn: &Connection,
    limit: i32,
) -> Result<(Vec<Observation>, Vec<ActiveTopic>)> {
    let recents = recent(conn, limit, None)?;
    let mut stmt = conn
        .prepare(
            "SELECT topic_key, scope, title, updated_at FROM observations
              WHERE topic_key IS NOT NULL AND deleted_at IS NULL
              GROUP BY topic_key
              ORDER BY MAX(updated_at) DESC
              LIMIT 20",
        )
        .map_err(|e| Error::internal(format!("prepare context topic: {e}")))?;
    let topic_iter = stmt
        .query_map([], |row| {
            Ok(ActiveTopic {
                topic_key: row.get(0)?,
                scope: row.get(1)?,
                latest_title: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .map_err(|e| Error::internal(format!("context topic query: {e}")))?;
    let mut topics = Vec::new();
    for r in topic_iter {
        topics.push(r.map_err(|e| Error::internal(format!("context topic row: {e}")))?);
    }
    Ok((recents, topics))
}

/// `Timeline` (FR12.4): chronological neighbors of one observation.
///
/// Returns `(before, anchor, after)` where:
/// - `before`  = up to `before_n` rows with `created_at < anchor.created_at`,
///               sorted DESC (closest-older-first).
/// - `anchor`  = the observation identified by `key`.
/// - `after`   = up to `after_n` rows with `created_at > anchor.created_at`,
///               sorted ASC (closest-newer-first).
///
/// Soft-deleted rows are excluded; the anchor itself must not be soft-deleted.
pub fn timeline(
    conn: &Connection,
    key: &ObservationKey,
    before_n: i32,
    after_n: i32,
) -> Result<(Vec<Observation>, Observation, Vec<Observation>)> {
    let anchor = get(conn, key)?;
    let before = if before_n > 0 {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM observations
                  WHERE deleted_at IS NULL
                    AND (created_at < ?1 OR (created_at = ?1 AND id < ?2))
                  ORDER BY created_at DESC, id DESC
                  LIMIT ?3"
            ))
            .map_err(|e| Error::internal(format!("prepare timeline before: {e}")))?;
        let rows = stmt
            .query_map(
                params![anchor.created_at, anchor.id, before_n],
                Observation::from_row,
            )
            .map_err(|e| Error::internal(format!("timeline before query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| Error::internal(format!("timeline before row: {e}")))?);
        }
        out
    } else {
        Vec::new()
    };
    let after = if after_n > 0 {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM observations
                  WHERE deleted_at IS NULL
                    AND (created_at > ?1 OR (created_at = ?1 AND id > ?2))
                  ORDER BY created_at ASC, id ASC
                  LIMIT ?3"
            ))
            .map_err(|e| Error::internal(format!("prepare timeline after: {e}")))?;
        let rows = stmt
            .query_map(
                params![anchor.created_at, anchor.id, after_n],
                Observation::from_row,
            )
            .map_err(|e| Error::internal(format!("timeline after query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| Error::internal(format!("timeline after row: {e}")))?);
        }
        out
    } else {
        Vec::new()
    };
    Ok((before, anchor, after))
}

/// One entry in an observation's supersession history.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub observation: Observation,
    /// The id of the newer observation that superseded this one (`None` = still active).
    pub superseded_by_id: Option<i64>,
}

/// Walk the superseded_by_id chain for `anchor_id` and return the full
/// lineage ordered oldest → newest.
///
/// The walk has two legs:
/// 1. Walk **backward** from `anchor_id` toward the original root by
///    following `superseded_by_id` links in the *other* direction: find all
///    rows that point *at* the anchor (and transitively at their
///    predecessors) — this finds older revisions the caller may not know.
/// 2. Walk **forward** from `anchor_id` following `superseded_by_id` until
///    it is NULL — this finds newer revisions.
///
/// Both legs are implemented with a single bidirectional recursive CTE.
/// Soft-deleted rows are included (they are part of the history).
pub fn history_chain(conn: &Connection, anchor_id: i64) -> Result<Vec<HistoryEntry>> {
    // The CTE first seeds with the anchor row, then expands in both
    // directions:
    //   - forward: current_id → rows.superseded_by_id
    //   - backward: rows where superseded_by_id = current_id
    // We track direction so the final result can be sorted by creation time.
    let sql = format!(
        "WITH RECURSIVE chain(id) AS (
            SELECT ?1
            UNION
            -- walk forward: follow superseded_by_id link
            SELECT o.superseded_by_id
            FROM   observations o
            JOIN   chain c ON o.id = c.id
            WHERE  o.superseded_by_id IS NOT NULL
            UNION
            -- walk backward: find predecessor that this row superseded
            SELECT o.id
            FROM   observations o
            JOIN   chain c ON o.superseded_by_id = c.id
        )
        SELECT {cols}, superseded_by_id
        FROM   observations
        WHERE  id IN (SELECT id FROM chain)
        ORDER  BY created_at ASC, id ASC",
        cols = SELECT_COLS,
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("history_chain prepare: {e}")))?;
    let rows = stmt
        .query_map(params![anchor_id], |row| {
            // SELECT_COLS has 18 columns (indices 0-17); superseded_by_id is column 18.
            let obs = Observation::from_row(row)?;
            let superseded_by_id: Option<i64> = row.get(18)?;
            Ok(HistoryEntry { observation: obs, superseded_by_id })
        })
        .map_err(|e| Error::internal(format!("history_chain query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("history_chain row: {e}")))?);
    }
    Ok(out)
}

/// Dense (cosine) fan-out search across multiple per-project DBs. Used by the
/// daemon's `--all-projects --mode hybrid` path to add a dense lane alongside
/// the existing global-DB BM25 lane.
///
/// Opens each project's DB read-only, registers the sqlite-vec extension,
/// calls [`search_dense`], and tags each result with the project name.
/// Capped at `ALL_PROJECTS_CAP` (32) projects; extras are silently skipped.
///
/// Errors opening individual project DBs are logged at warn level and skipped
/// (partial results preferred over hard failure).
pub fn search_dense_multi(
    project_db_paths: &[(String, std::path::PathBuf)],
    q_vec: &[f32],
    per_project_limit: i64,
) -> Result<Vec<(String, Observation)>> {
    crate::pragmas::ensure_sqlite_vec_extension();
    let capped = project_db_paths.iter().take(ALL_PROJECTS_CAP);
    let mut out: Vec<(String, Observation)> = Vec::new();
    for (project_name, db_path) in capped {
        if !db_path.exists() {
            continue;
        }
        let conn = match crate::db::open_read(db_path) {
            Ok(c) => c,
            Err(e) => {
                warn!(project = %project_name, error = %e, "search_dense_multi: skipping project (open failed)");
                continue;
            }
        };
        match search_dense(&conn, q_vec, per_project_limit) {
            Ok(hits) => {
                for obs in hits {
                    out.push((project_name.clone(), obs));
                }
            }
            Err(e) => {
                warn!(project = %project_name, error = %e, "search_dense_multi: dense search failed for project");
            }
        }
    }
    Ok(out)
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
        c.execute("UPDATE observations SET deleted_at = datetime('now')", [])
            .unwrap();
        assert!(search(&c, "alpha", None, None, 10).unwrap().is_empty());
        assert!(recent(&c, 10, None).unwrap().is_empty());
        assert!(list(&c, None, None, None, false, 10, None, None)
            .unwrap()
            .0
            .is_empty());
    }

    #[test]
    fn sc10_pagination_three_pages() {
        let (_d, mut c) = make_conn();
        for i in 0..12 {
            save(&mut c, &format!("row-{i}"), "note");
        }
        let (p1, c1) = list(&c, None, None, None, false, 5, None, None).unwrap();
        assert_eq!(p1.len(), 5);
        assert!(c1.is_some());
        let (p2, c2) = list(&c, None, None, None, false, 5, c1.as_ref(), None).unwrap();
        assert_eq!(p2.len(), 5);
        assert!(c2.is_some());
        let (p3, c3) = list(&c, None, None, None, false, 5, c2.as_ref(), None).unwrap();
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

    // ---------------------------------------------------------------------
    // FR12.3 / FR12.4 — Context + Timeline (spec2-t3)
    // ---------------------------------------------------------------------

    #[test]
    fn context_returns_recent_active() {
        let (_d, mut c) = make_conn();
        // Insert observations with topic keys + a soft-deleted one to verify
        // it's filtered.
        for i in 0..5 {
            let tx = c.transaction().unwrap();
            crate::write::handle_save_observation_for_tests(
                &tx,
                SaveObservationInput {
                    sync_id: None,
                    session_id: "s1".into(),
                    r#type: "note".into(),
                    title: format!("title-{i}"),
                    content: format!("content-{i}"),
                    tool_name: None,
                    scope: "project".into(),
                    created_by: None,
                    topic_key: Some(format!("note/topic-{i}")),
                    dedupe_window_secs: 0,
                    max_content_chars: 50_000,
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        // Soft-delete the most recent one.
        c.execute(
            "UPDATE observations SET deleted_at = datetime('now')
              WHERE title = 'title-4'",
            [],
        )
        .unwrap();

        let (recents, topics) = recent_active(&c, 10).unwrap();
        // 4 active recents (the deleted one is excluded).
        assert_eq!(recents.len(), 4);
        // 4 active topics (the deleted topic is also excluded — its only row
        // is soft-deleted and the GROUP BY filter `deleted_at IS NULL`
        // removes it).
        assert_eq!(topics.len(), 4);
        // Topics ordered by most-recent activity: topic-3 first.
        assert_eq!(topics[0].topic_key, "note/topic-3");
    }

    #[test]
    fn timeline_before_after_neighbors() {
        let (_d, mut c) = make_conn();
        // Insert 5 observations with distinguishable content. SQLite's
        // datetime('now') is per-second, so we manually stamp created_at to
        // guarantee ordering inside this test.
        for i in 0..5 {
            let tx = c.transaction().unwrap();
            crate::write::handle_save_observation_for_tests(
                &tx,
                SaveObservationInput {
                    sync_id: None,
                    session_id: "s1".into(),
                    r#type: "note".into(),
                    title: format!("t{i}"),
                    content: format!("body-{i}"),
                    tool_name: None,
                    scope: "project".into(),
                    created_by: None,
                    topic_key: None,
                    dedupe_window_secs: 0,
                    max_content_chars: 50_000,
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        // Stamp created_at so ordering is deterministic.
        for i in 0..5 {
            c.execute(
                "UPDATE observations SET created_at = ?1 WHERE title = ?2",
                params![format!("2026-01-0{}T00:00:00Z", i + 1), format!("t{i}")],
            )
            .unwrap();
        }
        // Anchor on the middle row (t2).
        let id: i64 = c
            .query_row("SELECT id FROM observations WHERE title = 't2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let (before, anchor, after) = timeline(&c, &ObservationKey::Id(id), 5, 5).unwrap();
        assert_eq!(anchor.title, "t2");
        // Closest-older-first: t1, t0.
        assert_eq!(
            before.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(),
            vec!["t1", "t0"]
        );
        // Closest-newer-first: t3, t4.
        assert_eq!(
            after.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(),
            vec!["t3", "t4"]
        );
    }

    #[test]
    fn timeline_zero_before_returns_empty_before() {
        let (_d, mut c) = make_conn();
        save(&mut c, "only", "note");
        let id: i64 = c
            .query_row("SELECT id FROM observations LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let (before, _anchor, after) = timeline(&c, &ObservationKey::Id(id), 0, 0).unwrap();
        assert!(before.is_empty());
        assert!(after.is_empty());
    }

    fn unit_vector(seed: u8) -> Vec<f32> {
        // Deterministic, non-zero 384-dim vector. Most positions are 0;
        // a few non-zero values keyed off the seed make distinct rows
        // distinguishable under cosine.
        let mut v = vec![0.0f32; 384];
        v[seed as usize % 384] = 1.0;
        v[(seed as usize + 17) % 384] = 0.5;
        v
    }

    fn insert_embedding(conn: &Connection, obs_id: i64, vec: &[f32], model: &str) {
        let mut blob = Vec::with_capacity(vec.len() * 4);
        for f in vec {
            blob.extend_from_slice(&f.to_le_bytes());
        }
        conn.execute(
            "INSERT OR REPLACE INTO observations_vec(rowid, embedding) VALUES (?1, ?2)",
            rusqlite::params![obs_id, blob],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO observation_embedding_meta \
                (observation_id, model, dim, created_at) \
             VALUES (?1, ?2, 384, datetime('now'))",
            rusqlite::params![obs_id, model],
        )
        .unwrap();
    }

    #[test]
    fn search_dense_returns_cosine_top_k() {
        // Spec retrieval-promotion SC-3: dense search returns observations
        // ordered by ascending cosine distance.
        let (_d, mut c) = make_conn();
        save(&mut c, "first body alpha", "note");
        save(&mut c, "second body beta", "note");
        save(&mut c, "third body gamma", "note");

        // Map observation rows to their ids in insertion order.
        let mut ids: Vec<i64> = Vec::new();
        let mut stmt = c
            .prepare("SELECT id FROM observations ORDER BY id ASC")
            .unwrap();
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap();
        for r in rows {
            ids.push(r.unwrap());
        }
        drop(stmt);
        assert_eq!(ids.len(), 3);

        // Seed three distinct embeddings; query == row[0]'s embedding so
        // we expect ids[0] to come back first (distance 0).
        let q = unit_vector(0);
        insert_embedding(&c, ids[0], &q, "bge-small-en-v1.5");
        insert_embedding(&c, ids[1], &unit_vector(8), "bge-small-en-v1.5");
        insert_embedding(&c, ids[2], &unit_vector(64), "bge-small-en-v1.5");

        let hits = search_dense(&c, &q, 3).unwrap();
        assert_eq!(hits.len(), 3, "expected 3 dense hits");
        assert_eq!(hits[0].id, ids[0], "exact-match row must rank first");
    }

    #[test]
    fn search_dense_returns_empty_when_no_embeddings() {
        // Empty observations_vec — the hybrid caller treats Ok(empty) as
        // "fall back to BM25 only".
        let (_d, c) = make_conn();
        let q = unit_vector(1);
        let hits = search_dense(&c, &q, 5).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn search_dense_rejects_wrong_dimension() {
        let (_d, c) = make_conn();
        let bad = vec![0.0_f32; 16];
        let err = search_dense(&c, &bad, 5).unwrap_err();
        assert!(format!("{err}").contains("384"));
    }

    #[test]
    fn search_dense_excludes_soft_deleted() {
        let (_d, mut c) = make_conn();
        save(&mut c, "live row", "note");
        save(&mut c, "dead row", "note");
        let ids: Vec<i64> = c
            .prepare("SELECT id FROM observations ORDER BY id ASC")
            .unwrap()
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let q = unit_vector(2);
        insert_embedding(&c, ids[0], &q, "bge-small-en-v1.5");
        insert_embedding(&c, ids[1], &q, "bge-small-en-v1.5");
        c.execute(
            "UPDATE observations SET deleted_at = datetime('now') WHERE id = ?1",
            rusqlite::params![ids[1]],
        )
        .unwrap();

        let hits = search_dense(&c, &q, 5).unwrap();
        assert_eq!(hits.len(), 1, "soft-deleted row must not surface");
        assert_eq!(hits[0].id, ids[0]);
    }

    // ---------------------------------------------------------------------------
    // history_chain tests
    // ---------------------------------------------------------------------------

    /// Build a v1→v2→v3 supersession chain by directly writing the DB links.
    fn make_chain_3() -> (TempDir, Connection, i64, i64, i64) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let mut conn = crate::db::open_write(&path).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        // Insert three rows with distinct content but same type/scope.
        for content in ["v1-body", "v2-body", "v3-body"] {
            let tx = conn.transaction().unwrap();
            handle_save_observation_for_tests(
                &tx,
                SaveObservationInput {
                    sync_id: None,
                    session_id: "s1".into(),
                    r#type: "decision".into(),
                    title: format!("title-{content}"),
                    content: content.into(),
                    tool_name: None,
                    scope: "project".into(),
                    created_by: None,
                    topic_key: None,
                    dedupe_window_secs: 0,
                    max_content_chars: 50_000,
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        // Fetch ids in order.
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM observations ORDER BY id ASC")
            .unwrap()
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let (v1, v2, v3) = (ids[0], ids[1], ids[2]);
        // Wire supersession: v1 superseded_by v2, v2 superseded_by v3.
        conn.execute(
            "UPDATE observations SET superseded_by_id = ?1, deleted_at = datetime('now'), delete_reason = 'superseded' WHERE id = ?2",
            rusqlite::params![v2, v1],
        ).unwrap();
        conn.execute(
            "UPDATE observations SET superseded_by_id = ?1, deleted_at = datetime('now'), delete_reason = 'superseded' WHERE id = ?2",
            rusqlite::params![v3, v2],
        ).unwrap();
        (dir, conn, v1, v2, v3)
    }

    #[test]
    fn history_chain_anchor_on_oldest_returns_all_three() {
        let (_d, conn, v1, _v2, v3) = make_chain_3();
        let chain = history_chain(&conn, v1).unwrap();
        assert_eq!(chain.len(), 3, "chain: {chain:?}");
        // First entry is the oldest (anchor); last is the newest (active).
        assert_eq!(chain[0].observation.id, v1);
        assert_eq!(chain[2].observation.id, v3);
        // The newest entry has no superseded_by_id.
        assert!(chain[2].superseded_by_id.is_none());
    }

    #[test]
    fn history_chain_anchor_on_newest_returns_all_three() {
        let (_d, conn, v1, _v2, v3) = make_chain_3();
        let chain = history_chain(&conn, v3).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].observation.id, v1);
        assert_eq!(chain[2].observation.id, v3);
    }

    #[test]
    fn history_chain_anchor_on_middle_returns_all_three() {
        let (_d, conn, v1, v2, v3) = make_chain_3();
        let chain = history_chain(&conn, v2).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].observation.id, v1);
        assert_eq!(chain[2].observation.id, v3);
    }

    #[test]
    fn history_chain_single_obs_returns_one_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let mut conn = crate::db::open_write(&path).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        handle_save_observation_for_tests(
            &tx,
            SaveObservationInput {
                sync_id: None,
                session_id: "s1".into(),
                r#type: "note".into(),
                title: "solo-title".into(),
                content: "solo".into(),
                tool_name: None,
                scope: "project".into(),
                created_by: None,
                topic_key: None,
                dedupe_window_secs: 0,
                max_content_chars: 50_000,
            },
        )
        .unwrap();
        tx.commit().unwrap();
        let id: i64 = conn
            .query_row("SELECT id FROM observations LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let chain = history_chain(&conn, id).unwrap();
        assert_eq!(chain.len(), 1);
        assert!(chain[0].superseded_by_id.is_none());
    }

    #[test]
    fn history_chain_includes_soft_deleted() {
        let (_d, conn, v1, _v2, _v3) = make_chain_3();
        // v1 is soft-deleted; it should still appear in the chain.
        let chain = history_chain(&conn, v1).unwrap();
        let has_deleted = chain.iter().any(|e| e.observation.deleted_at.is_some());
        assert!(has_deleted, "chain must include soft-deleted entries");
    }

    // ---------------------------------------------------------------------------
    // int8 quantize round-trip (Spec 1a)
    // ---------------------------------------------------------------------------

    fn insert_quantized(conn: &Connection, obs_id: i64, vec: &[f32], model: &str) {
        use memlayer_embed::quantize::{calibrate_scale, f32_to_int8};
        let scale = calibrate_scale(&[vec]);
        let int8: Vec<i8> = f32_to_int8(vec, scale);
        let blob: Vec<u8> = int8.iter().map(|&x| x as u8).collect();
        conn.execute(
            "INSERT OR REPLACE INTO observation_embedding_meta \
                (observation_id, model, dim, created_at, quantized, quantized_blob, scale) \
             VALUES (?1, ?2, 384, datetime('now'), 1, ?3, ?4)",
            rusqlite::params![obs_id, model, blob, scale],
        )
        .unwrap();
    }

    #[test]
    fn search_dense_quantized_returns_closest_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("q.db");
        let mut conn = crate::db::open_write(&path).unwrap();
        conn.execute("INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')", []).unwrap();
        for content in ["alpha vec", "beta vec"] {
            let tx = conn.transaction().unwrap();
            handle_save_observation_for_tests(
                &tx,
                crate::write::SaveObservationInput {
                    sync_id: None,
                    session_id: "s1".into(),
                    r#type: "note".into(),
                    title: format!("t-{content}"),
                    content: content.into(),
                    tool_name: None,
                    scope: "project".into(),
                    created_by: None,
                    topic_key: None,
                    dedupe_window_secs: 0,
                    max_content_chars: 50_000,
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM observations ORDER BY id ASC").unwrap()
            .query_map([], |r| r.get::<_, i64>(0)).unwrap()
            .map(|r| r.unwrap())
            .collect();

        // Seed two distinct 384-dim vectors.
        let q = unit_vector(0);
        let other = unit_vector(99);
        insert_quantized(&conn, ids[0], &q, "bge-small-en-v1.5");
        insert_quantized(&conn, ids[1], &other, "bge-small-en-v1.5");

        // Query with the exact vector for ids[0] — should rank first.
        let hits = search_dense(&conn, &q, 2).unwrap();
        assert_eq!(hits.len(), 2, "expected 2 quantized hits");
        assert_eq!(hits[0].id, ids[0], "exact-match row should rank first (cosine sim ≈ 1.0)");
    }

    #[test]
    fn search_dense_quantized_excludes_soft_deleted() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("q2.db");
        let mut conn = crate::db::open_write(&path).unwrap();
        conn.execute("INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')", []).unwrap();
        for content in ["live", "dead"] {
            let tx = conn.transaction().unwrap();
            handle_save_observation_for_tests(
                &tx,
                crate::write::SaveObservationInput {
                    sync_id: None, session_id: "s1".into(), r#type: "note".into(),
                    title: format!("t-{content}"), content: content.into(),
                    tool_name: None, scope: "project".into(), created_by: None,
                    topic_key: None, dedupe_window_secs: 0, max_content_chars: 50_000,
                },
            ).unwrap();
            tx.commit().unwrap();
        }
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM observations ORDER BY id ASC").unwrap()
            .query_map([], |r| r.get::<_, i64>(0)).unwrap()
            .map(|r| r.unwrap()).collect();
        let q = unit_vector(5);
        insert_quantized(&conn, ids[0], &q, "bge-small-en-v1.5");
        insert_quantized(&conn, ids[1], &q, "bge-small-en-v1.5");
        conn.execute(
            "UPDATE observations SET deleted_at = datetime('now') WHERE id = ?1",
            rusqlite::params![ids[1]],
        ).unwrap();
        let hits = search_dense(&conn, &q, 5).unwrap();
        assert_eq!(hits.len(), 1, "soft-deleted row must not surface in quantized search");
        assert_eq!(hits[0].id, ids[0]);
    }
}
