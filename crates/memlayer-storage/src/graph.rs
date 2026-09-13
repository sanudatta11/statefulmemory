//! Cue-entity graph storage for the briefing layer (spec: graph-briefing).
//!
//! All writes ride the caller's connection (the write thread in production);
//! read helpers run on read connections only.

use memlayer_core::config::{normalize_entity_name, Entity, EntityKind};
use memlayer_core::error::{Error, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};
use tracing::debug;

/// Prefix lookup on `norm_name`, ordered shortest-first (more general cues first).
pub fn entity_lookup(conn: &Connection, norm_prefix: &str, limit: usize) -> Vec<Entity> {
    let mut out = Vec::new();
    if norm_prefix.is_empty() || limit == 0 {
        return out;
    }
    let sql = "SELECT id, kind, name, norm_name FROM entities
               WHERE norm_name LIKE ?1 ESCAPE '\\'
               ORDER BY LENGTH(norm_name) ASC, id ASC LIMIT ?2";
    let Ok(mut stmt) = conn.prepare(sql) else {
        return out;
    };
    // LIKE pattern: prefix with `_` matching relaxed via escape; `%` and `_`
    // in the prefix itself are escaped so user text can't widen the match.
    let escaped = norm_prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("{escaped}%");
    let Ok(rows) = stmt.query_map(params![pattern, limit as i64], |row| {
        let kind_s: String = row.get(1)?;
        Ok(Entity {
            id: row.get(0)?,
            kind: EntityKind::parse(&kind_s).unwrap_or(EntityKind::Concept),
            name: row.get(2)?,
            norm_name: row.get(3)?,
        })
    }) else {
        return out;
    };
    for r in rows.flatten() {
        out.push(r);
    }
    out
}

/// Insert-or-merge by `norm_name`. On conflict, the newer display name wins.
/// Returns the entity row id. `now_epoch` is accepted for call-site symmetry
/// with `insert_edge` but unused: V11 `entities` carries no timestamp column.
pub fn upsert_entity(conn: &Connection, kind: &str, name: &str, now_epoch: i64) -> Result<i64> {
    let _ = now_epoch;
    let norm = normalize_entity_name(name);
    let id: i64 = conn
        .query_row(
            "INSERT INTO entities (kind, name, norm_name) VALUES (?1, ?2, ?3)
             ON CONFLICT(norm_name) DO UPDATE SET name = excluded.name
             RETURNING id",
            params![kind, name, norm],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("upsert_entity: {e}")))?;
    Ok(id)
}

/// Wire an entity to an observation. Deduped on (entity_id, observation_id, source).
pub fn insert_mention(
    conn: &Connection,
    entity_id: i64,
    observation_id: i64,
    offsets: Option<&str>,
    source: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO entity_mentions (entity_id, observation_id, offsets, source)
         SELECT ?1, ?2, ?3, ?4
         WHERE NOT EXISTS (
             SELECT 1 FROM entity_mentions
             WHERE entity_id = ?1 AND observation_id = ?2 AND source = ?4
         )",
        params![entity_id, observation_id, offsets, source],
    )
    .map_err(|e| Error::internal(format!("insert_mention: {e}")))?;
    Ok(())
}

/// Increment-style edge write: existing (from,to,relation) rows accumulate
/// weight (weight = weight + new) and refresh `last_seen`; new pairs insert at
/// the given weight. Self-edges are silently skipped (logged at debug).
// Signature fixed by task spec (8 args incl. conn); callers are graph
// writers only, so the arity churn is acceptable.
#[allow(clippy::too_many_arguments)]
pub fn insert_edge(
    conn: &Connection,
    from_entity: i64,
    to_entity: i64,
    relation: &str,
    weight: f64,
    first_seen: i64,
    last_seen: i64,
    src_observation_id: Option<i64>,
) -> Result<()> {
    if from_entity == to_entity {
        debug!(
            from = from_entity,
            relation, "insert_edge: skipped self-edge"
        );
        return Ok(());
    }
    // V11 schema has no UNIQUE(from,to,relation) index, so conflict-target
    // upsert isn't available; do read-then-write (write thread serializes).
    let existing: Option<(f64, Option<i64>)> = conn
        .query_row(
            "SELECT weight, first_seen FROM entity_edges
             WHERE from_entity = ?1 AND to_entity = ?2 AND relation = ?3",
            params![from_entity, to_entity, relation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| Error::internal(format!("insert_edge select: {e}")))?;
    match existing {
        Some((_w, first)) => {
            let new_first = first.map_or(first_seen, |f| f.min(first_seen));
            conn.execute(
                "UPDATE entity_edges SET weight = weight + ?4, first_seen = ?5, last_seen = ?6
                 WHERE from_entity = ?1 AND to_entity = ?2 AND relation = ?3",
                params![
                    from_entity,
                    to_entity,
                    relation,
                    weight,
                    new_first,
                    last_seen
                ],
            )
            .map_err(|e| Error::internal(format!("insert_edge update: {e}")))?;
        }
        None => {
            conn.execute(
                "INSERT INTO entity_edges
                     (from_entity, to_entity, relation, weight, first_seen, last_seen, src_observation_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![from_entity, to_entity, relation, weight, first_seen, last_seen, src_observation_id],
            )
            .map_err(|e| Error::internal(format!("insert_edge insert: {e}")))?;
        }
    }
    Ok(())
}

/// BFS neighborhood via recursive CTE. Returns `(entity_id, hop_distance)`
/// pairs for the seed (`hop 0`) and entities within `hops` hops, traversing
/// only edges whose relation is in `edge_types`, and never expanding *into*
/// entities whose mention count exceeds `degree_cap`.
pub fn neighbors(
    conn: &Connection,
    entity_id: i64,
    hops: u8,
    edge_types: &[&str],
    degree_cap: usize,
) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    if edge_types.is_empty() || hops == 0 {
        return out;
    }
    // Explicit placeholder indexing: ?1 seed, ?2 hops, ?3.. edge types, then
    // degree cap — avoids SQLite's bare-`?` auto-indexing colliding with
    // explicit indices when binding a dynamic IN list alongside scalars.
    let placeholders: String = edge_types
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 3))
        .collect::<Vec<_>>()
        .join(",");
    let cap_idx = 3 + edge_types.len();
    let sql = format!(
        "WITH RECURSIVE walk(entity_id, hop) AS (
            SELECT ?1, 0
            UNION
            SELECT e.to_entity, walk.hop + 1
            FROM walk
            JOIN entity_edges e ON e.from_entity = walk.entity_id
            WHERE walk.hop < ?2
              AND e.relation IN ({placeholders})
              AND (SELECT COUNT(*) FROM entity_mentions m WHERE m.entity_id = e.to_entity) <= ?{cap_idx}
         )
         SELECT DISTINCT entity_id, hop FROM walk ORDER BY hop ASC, entity_id ASC"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            debug!(error = %e, "neighbors prepare failed");
            return out;
        }
    };
    let hops_i = i64::from(hops);
    let cap_i = degree_cap as i64;
    let mut bound: Vec<&dyn ToSql> = vec![&entity_id, &hops_i];
    for t in edge_types {
        bound.push(t);
    }
    bound.push(&cap_i);
    let Ok(rows) = stmt.query_map(params_from_iter(bound), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return out;
    };
    for r in rows.flatten() {
        out.push(r);
    }
    out
}

/// Distinct observation ids co-mentioning the given entity set, most-referenced
/// first. Empty input → empty output.
pub fn observations_for_entities(conn: &Connection, entity_ids: &[i64], limit: usize) -> Vec<i64> {
    let mut out = Vec::new();
    if entity_ids.is_empty() || limit == 0 {
        return out;
    }
    let placeholders: String = entity_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT observation_id, COUNT(*) AS refs
         FROM entity_mentions
         WHERE entity_id IN ({placeholders})
         GROUP BY observation_id
         ORDER BY refs DESC, observation_id ASC
         LIMIT ?"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            debug!(error = %e, "observations_for_entities prepare failed");
            return out;
        }
    };
    let limit_i = limit as i64;
    let mut bound: Vec<&dyn ToSql> = entity_ids.iter().map(|id| id as &dyn ToSql).collect();
    bound.push(&limit_i);
    let Ok(rows) = stmt.query_map(params_from_iter(bound), |row| row.get::<_, i64>(0)) else {
        return out;
    };
    for r in rows.flatten() {
        out.push(r);
    }
    out
}

/// Look up one entity by primary key.
pub fn get_entity_by_id(conn: &Connection, id: i64) -> Result<memlayer_core::Entity> {
    conn.query_row(
        "SELECT id, kind, name, norm_name FROM entities WHERE id = ?1",
        [id],
        entity_from_row,
    )
    .map_err(|e| Error::internal(format!("get_entity_by_id: {e}")))
}

/// Batch entity fetch by primary key. Order = input order, deduped.
pub fn entities_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<memlayer_core::Entity>> {
    let mut out = Vec::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql =
        format!("SELECT id, kind, name, norm_name FROM entities WHERE id IN ({placeholders})");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("entities_by_ids prepare: {e}")))?;
    let bound: Vec<&dyn ToSql> = ids.iter().map(|id| id as &dyn ToSql).collect();
    let rows = stmt
        .query_map(params_from_iter(bound), entity_from_row)
        .map_err(|e| Error::internal(format!("entities_by_ids query: {e}")))?;
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("entities_by_ids row: {e}")))?);
    }
    Ok(out)
}

/// Edges among the given entity set, filtered by relation, most recent first.
pub struct GraphEdge {
    pub from_id: i64,
    pub to_id: i64,
    pub relation: String,
    pub weight: f64,
    pub first_seen: i64,
    pub last_seen: i64,
    pub src_observation_id: Option<i64>,
}

pub fn edges_for_entities(
    conn: &Connection,
    entity_ids: &[i64],
    edge_types: &[&str],
    limit: usize,
) -> Result<Vec<GraphEdge>> {
    let mut out = Vec::new();
    if entity_ids.is_empty() || edge_types.is_empty() || limit == 0 {
        return Ok(out);
    }
    let n = entity_ids.len();
    let id_ph: Vec<String> = (1..=n).map(|i| format!("?{i}")).collect();
    let rel_ph: Vec<String> = (n * 2 + 1..=n * 2 + edge_types.len())
        .map(|i| format!("?{i}"))
        .collect();
    let limit_idx = n * 2 + edge_types.len() + 1;
    let sql = format!(
        "SELECT from_entity, to_entity, relation, weight, first_seen, last_seen, src_observation_id
         FROM entity_edges
         WHERE from_entity IN ({}) AND to_entity IN ({})
           AND relation IN ({})
         ORDER BY last_seen DESC, id DESC
         LIMIT ?{limit_idx}",
        id_ph.join(","),
        id_ph.join(","),
        rel_ph.join(","),
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("edges_for_entities prepare: {e}")))?;
    let mut bound: Vec<&dyn ToSql> = Vec::new();
    for id in entity_ids {
        bound.push(id as &dyn ToSql);
    }
    for id in entity_ids {
        bound.push(id as &dyn ToSql);
    }
    for rel in edge_types {
        bound.push(rel as &dyn ToSql);
    }
    let limit_local = (limit as i64).max(1);
    bound.push(&limit_local);
    let rows = stmt
        .query_map(params_from_iter(bound), |row| {
            Ok(GraphEdge {
                from_id: row.get(0)?,
                to_id: row.get(1)?,
                relation: row.get(2)?,
                weight: row.get(3)?,
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
                src_observation_id: row.get(6)?,
            })
        })
        .map_err(|e| Error::internal(format!("edges_for_entities query: {e}")))?;
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("edges_for_entities row: {e}")))?);
    }
    Ok(out)
}

fn entity_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<memlayer_core::Entity> {
    Ok(memlayer_core::Entity {
        id: row.get(0)?,
        kind: memlayer_core::config::EntityKind::parse(&row.get::<_, String>(1)?)
            .unwrap_or(memlayer_core::config::EntityKind::Concept),
        name: row.get(2)?,
        norm_name: row.get(3)?,
    })
}

/// Aggregate counts for CLI display.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphStats {
    pub entities_by_kind: Vec<(String, i64)>,
    pub total_entities: i64,
    pub total_mentions: i64,
    pub edges_by_relation: Vec<(String, i64)>,
    pub total_edges: i64,
}

impl std::fmt::Display for GraphStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "entities: {}", self.total_entities)?;
        for (kind, n) in &self.entities_by_kind {
            writeln!(f, "  {kind}: {n}")?;
        }
        writeln!(f, "mentions: {}", self.total_mentions)?;
        writeln!(f, "edges: {}", self.total_edges)?;
        for (rel, n) in &self.edges_by_relation {
            writeln!(f, "  {rel}: {n}")?;
        }
        Ok(())
    }
}

pub fn stats(conn: &Connection) -> Result<GraphStats> {
    let mut out = GraphStats::default();

    let mut stmt = conn
        .prepare("SELECT kind, COUNT(*) FROM entities GROUP BY kind ORDER BY kind ASC")
        .map_err(|e| Error::internal(format!("graph stats kinds prepare: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| Error::internal(format!("graph stats kinds query: {e}")))?;
    for r in rows {
        out.entities_by_kind
            .push(r.map_err(|e| Error::internal(format!("graph stats kinds row: {e}")))?);
    }
    out.total_entities = out.entities_by_kind.iter().map(|(_, n)| *n).sum();

    out.total_mentions = conn
        .query_row("SELECT COUNT(*) FROM entity_mentions", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("graph stats mentions: {e}")))?;

    let mut stmt = conn
        .prepare(
            "SELECT relation, COUNT(*) FROM entity_edges GROUP BY relation ORDER BY relation ASC",
        )
        .map_err(|e| Error::internal(format!("graph stats edges prepare: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| Error::internal(format!("graph stats edges query: {e}")))?;
    for r in rows {
        out.edges_by_relation
            .push(r.map_err(|e| Error::internal(format!("graph stats edges row: {e}")))?);
    }
    out.total_edges = out.edges_by_relation.iter().map(|(_, n)| *n).sum();

    Ok(out)
}

/// One entity per `observation_anchors` row: `file` for the path part, `symbol`
/// for the symbol part when present. Mentions wired with `source='anchor'`.
/// Returns the number of NEW entity rows created; idempotent.
pub fn backfill_from_anchors(conn: &Connection) -> Result<usize> {
    let mut stmt = conn
        .prepare(
            "SELECT a.observation_id, a.path, a.symbol, o.created_at
             FROM observation_anchors a
             JOIN observations o ON o.id = a.observation_id
             ORDER BY a.id ASC",
        )
        .map_err(|e| Error::internal(format!("backfill prepare: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| Error::internal(format!("backfill query: {e}")))?;
    let mut anchor_rows = Vec::new();
    for r in rows {
        anchor_rows.push(r.map_err(|e| Error::internal(format!("backfill row: {e}")))?);
    }

    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("backfill count: {e}")))?;

    for (observation_id, path, symbol, created_at) in anchor_rows {
        let now_epoch = created_at
            .as_deref()
            .and_then(epoch_from_sqlite_datetime)
            .unwrap_or(0);
        // Dedupe path/symbol entities across anchors of the same observation so
        // a second anchor for the same file does not double-write mentions.
        let mut seen: std::collections::HashSet<(i64, &'static str)> = Default::default();

        let file_id = upsert_entity(conn, "file", &path, now_epoch)?;
        if seen.insert((file_id, "file")) {
            insert_mention(conn, file_id, observation_id, None, "anchor")?;
        }
        if let Some(sym) = symbol.as_deref().filter(|s| !s.is_empty()) {
            let sym_id = upsert_entity(conn, "symbol", sym, now_epoch)?;
            if seen.insert((sym_id, "symbol")) {
                insert_mention(conn, sym_id, observation_id, None, "anchor")?;
            }
        }
    }

    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("backfill count: {e}")))?;
    Ok((after - before).max(0) as usize)
}

/// SQLite `datetime('now')` format → unix epoch seconds. Naive parse: the
/// format is fixed by SQLite (`YYYY-MM-DD HH:MM:SS`); fallback to 0 on
/// malformed input rather than failing the backfill.
fn epoch_from_sqlite_datetime(s: &str) -> Option<i64> {
    let (date, time) = s.split_once(' ')?;
    let mut dp = date.split('-');
    let y: i32 = dp.next()?.parse().ok()?;
    let mo: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let mut tp = time.split(':');
    let h: u32 = tp.next()?.parse().ok()?;
    let mi: u32 = tp.next()?.parse().ok()?;
    let sec_str = tp.next()?;
    let sec: f64 = sec_str.parse().ok()?;
    let days = days_from_civil(y, mo, d);
    Some(days * 86_400 + (h as i64) * 3600 + (mi as i64) * 60 + sec as i64)
}

/// Days since 1970-01-01 from a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(if m <= 2 { y - 1 } else { y });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        crate::pragmas::ensure_sqlite_vec_extension();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::run_migrations(&mut conn).unwrap();
        conn
    }

    fn seed_obs(conn: &Connection, id: i64) {
        let session_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = 's1')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if !session_exists {
            conn.execute(
                "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
                [],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO observations (id, sync_id, session_id, type, title, content)
             VALUES (?1, ?2, 's1', 'decision', 't', 'c')",
            params![id, format!("sync{id}")],
        )
        .unwrap();
    }

    #[test]
    fn upsert_entity_dedupes_by_norm_name() {
        let conn = mem_conn();
        let a = upsert_entity(&conn, "file", "src/Auth.rs", 100).unwrap();
        let b = upsert_entity(&conn, "file", "src/auth.rs", 200).unwrap();
        assert_eq!(a, b);
        let all = entity_lookup(&conn, "src/auth.rs", 10);
        assert_eq!(all.len(), 1);
        // newer display name wins on conflict
        assert_eq!(all[0].name, "src/auth.rs");
        assert_eq!(all[0].kind, EntityKind::File);
    }

    #[test]
    fn insert_mention_dedupes_on_entity_obs_source() {
        let conn = mem_conn();
        seed_obs(&conn, 1);
        let eid = upsert_entity(&conn, "file", "f.rs", 0).unwrap();
        insert_mention(&conn, eid, 1, Some("[0,5]"), "anchor").unwrap();
        // same (entity, obs, source): skipped even with different offsets
        insert_mention(&conn, eid, 1, Some("[9,9]"), "anchor").unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // same entity+obs, different source: allowed
        insert_mention(&conn, eid, 1, None, "backtick").unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn edge_weight_bumps_and_self_edge_skipped() {
        let conn = mem_conn();
        let a = upsert_entity(&conn, "concept", "alpha", 0).unwrap();
        let b = upsert_entity(&conn, "concept", "beta", 0).unwrap();
        insert_edge(&conn, a, b, "mentions", 1.0, 10, 10, Some(1)).unwrap();
        insert_edge(&conn, a, b, "mentions", 2.5, 20, 20, Some(2)).unwrap();
        let (w, first, last): (f64, i64, i64) = conn
            .query_row(
                "SELECT weight, first_seen, last_seen FROM entity_edges
                 WHERE from_entity = ?1 AND to_entity = ?2 AND relation = 'mentions'",
                params![a, b],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(w, 3.5);
        assert_eq!(first, 10);
        assert_eq!(last, 20);
        // self-edge silently skipped
        insert_edge(&conn, a, a, "mentions", 1.0, 10, 10, None).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn neighbors_respects_hops_edge_types_and_degree_cap() {
        let conn = mem_conn();
        // chain: a ->(mentions) b ->(mentions) c ->(mentions) d, plus a ->(co_occurs) d
        let a = upsert_entity(&conn, "concept", "a", 0).unwrap();
        let b = upsert_entity(&conn, "concept", "b", 0).unwrap();
        let c = upsert_entity(&conn, "concept", "c", 0).unwrap();
        let d = upsert_entity(&conn, "concept", "d", 0).unwrap();
        for (f, t, rel) in [
            (a, b, "mentions"),
            (b, c, "mentions"),
            (c, d, "mentions"),
            (a, d, "co_occurs"),
        ] {
            insert_edge(&conn, f, t, rel, 1.0, 0, 0, None).unwrap();
        }
        // hops=1: only a, b (co_occurs filtered out)
        let r = neighbors(&conn, a, 1, &["mentions"], 256);
        assert_eq!(r, vec![(a, 0), (b, 1)]);
        // hops=2: reaches c; co_occurs shortcut to d still filtered
        let r = neighbors(&conn, a, 2, &["mentions"], 256);
        assert_eq!(r, vec![(a, 0), (b, 1), (c, 2)]);
        // edge_types filter allows co_occurs: d reachable at hop 1
        let r = neighbors(&conn, a, 2, &["mentions", "co_occurs"], 256);
        assert_eq!(r, vec![(a, 0), (b, 1), (d, 1), (c, 2)]);
        // degree cap: give c more mentions than cap → c unreachable, d also cut off
        seed_obs(&conn, 1);
        for obs in 1..=3 {
            insert_mention(&conn, c, obs, None, "anchor").unwrap();
        }
        let r = neighbors(&conn, a, 3, &["mentions"], 2);
        assert_eq!(r, vec![(a, 0), (b, 1)]);
    }

    #[test]
    fn observations_for_entities_ranks_by_comention_and_empty_input() {
        let conn = mem_conn();
        seed_obs(&conn, 1);
        seed_obs(&conn, 2);
        seed_obs(&conn, 3);
        let a = upsert_entity(&conn, "concept", "a", 0).unwrap();
        let b = upsert_entity(&conn, "concept", "b", 0).unwrap();
        // obs 1 mentions both a and b (refs=2), obs 2 mentions a (refs=1)
        insert_mention(&conn, a, 1, None, "token").unwrap();
        insert_mention(&conn, b, 1, None, "token").unwrap();
        insert_mention(&conn, a, 2, None, "token").unwrap();
        assert_eq!(observations_for_entities(&conn, &[a, b], 10), vec![1, 2]);
        // empty input → empty, no error
        assert!(observations_for_entities(&conn, &[], 10).is_empty());
        // limit respected
        assert_eq!(observations_for_entities(&conn, &[a, b], 1), vec![1]);
        let _ = conn;
    }

    #[test]
    fn backfill_from_anchors_creates_entities_and_is_idempotent() {
        let conn = mem_conn();
        seed_obs(&conn, 1);
        seed_obs(&conn, 2);
        let anchor = crate::anchor::Anchor::parse("src/auth.rs::validate").unwrap();
        crate::anchor::insert_anchors(&conn, 1, std::slice::from_ref(&anchor)).unwrap();
        let anchor2 = crate::anchor::Anchor::parse("src/db.rs").unwrap();
        crate::anchor::insert_anchors(&conn, 2, &[anchor2]).unwrap();

        let created = backfill_from_anchors(&conn).unwrap();
        assert_eq!(created, 3); // src/auth.rs + validate + src/db.rs
        let mention_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mention_count, 3);
        let kinds = stats(&conn).unwrap();
        assert_eq!(
            kinds.entities_by_kind,
            vec![("file".to_string(), 2), ("symbol".to_string(), 1)]
        );

        // second run: no new entities or mentions
        let created = backfill_from_anchors(&conn).unwrap();
        assert_eq!(created, 0);
        let mention_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mention_count, 3);
        let _ = anchor;
    }
}
