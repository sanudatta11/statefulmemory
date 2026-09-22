//! Dream-lite: review-only consolidation scan (spec: MEM0 gap 1.4).
//!
//! Heuristics only — no LLM, no writes. For each live observation we look for
//! siblings in the same scope+type:
//!   1. Exact-content duplicates (same `normalized_hash`) — candidates for a
//!      dedupe merge.
//!   2. Title-overlap supersession candidates — an older row whose BM25 match
//!      on the current title is strong; the reviewer may supersede the older
//!      by the newer one.
//!
//! Everything is advisory. Applying a proposal is a separate, explicit action
//! (the write thread's existing supersession path) — this module never writes.

use statefulmemory_core::error::{Error, Result};
use tracing::debug;

use crate::models::Observation;

/// One consolidation suggestion. All ids refer to the same project DB.
#[derive(Debug, Clone, PartialEq)]
pub struct DreamProposal {
    /// Review classification: `duplicate` or `supersede`.
    pub kind: String,
    /// The observation the reviewer would KEEP (newer / canonical).
    pub keep_id: i64,
    /// The observation the reviewer would dismiss or supersede.
    pub drop_id: i64,
    /// Human-readable reason.
    pub reason: String,
    /// 0..=1 confidence (hash-duplicates are 1.0; title-similar lower).
    pub confidence: f64,
}

const TITLE_MATCH_THRESHOLD: f64 = 0.55;

/// Scan a project DB and return consolidation proposals (review-only).
pub fn dream_scan(conn: &rusqlite::Connection) -> Result<Vec<DreamProposal>> {
    let all = list_live(conn)?;
    if all.len() < 2 {
        return Ok(Vec::new());
    }

    let mut out: Vec<DreamProposal> = Vec::new();
    let mut seen_pairs: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();

    for obs in &all {
        // (a) exact-content duplicates within the same scope+type.
        if let Some(hash) = obs.normalized_hash.as_deref().filter(|h| !h.is_empty()) {
            for other in &all {
                if other.id == obs.id {
                    continue;
                }
                if other.r#type != obs.r#type || other.scope != obs.scope {
                    continue;
                }
                if other.normalized_hash.as_deref() == Some(hash) {
                    let (keep, drop, reason) = order_for_keep(obs, other, "identical content");
                    if seen_pairs.insert((keep.id, drop.id)) {
                        out.push(DreamProposal {
                            kind: "duplicate".into(),
                            keep_id: keep.id,
                            drop_id: drop.id,
                            reason,
                            confidence: 1.0,
                        });
                    }
                }
            }
        }

        // (b) title-overlap supersession candidate (BM25 self-search).
        if obs.r#type == "note" {
            // Notes are append-only-ish; skipping keeps the surface stable.
            continue;
        }
        let query = obs.title.trim();
        if query.chars().count() < 8 {
            continue;
        }
        debug!(obs_id = obs.id, "dream scan title search");
        let matches =
            match crate::read::search(conn, query, Some(&obs.r#type), Some(&obs.scope), 12) {
                Ok(m) => m,
                Err(_) => continue,
            };
        for hit in matches {
            if hit.id == obs.id {
                continue;
            }
            // Older row whose title is suspiciously close to the current one.
            if title_overlap(&hit.title, &obs.title) < TITLE_MATCH_THRESHOLD {
                continue;
            }
            let (keep, drop, reason) = order_for_keep(obs, &hit, "overlapping titles (BM25)");
            if seen_pairs.insert((keep.id, drop.id)) {
                out.push(DreamProposal {
                    kind: "supersede".into(),
                    keep_id: keep.id,
                    drop_id: drop.id,
                    reason,
                    confidence: 0.6,
                });
            }
        }
    }

    out.sort_by(|a, b| {
        (b.confidence as i32)
            .cmp(&(a.confidence as i32))
            .then(a.keep_id.cmp(&b.keep_id).then(a.drop_id.cmp(&b.drop_id)))
    });
    Ok(out)
}

/// Keep the newer row (by id tie-break on created_at), drop the other.
fn order_for_keep<'a>(
    a: &'a Observation,
    b: &'a Observation,
    basis: &str,
) -> (&'a Observation, &'a Observation, String) {
    let newer = if a.created_at >= b.created_at { a } else { b };
    let older = if a.created_at >= b.created_at { b } else { a };
    (
        newer,
        older,
        format!("{basis}: #{} supersedes #{}", newer.id, older.id),
    )
}

/// Cheap word-overlap Dice coefficient (0..=1) over tokenized titles.
fn title_overlap(a: &str, b: &str) -> f64 {
    let at: Vec<String> = a
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let bt: Vec<String> = b
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if at.is_empty() || bt.is_empty() {
        return 0.0;
    }
    let aa: std::collections::HashSet<String> = at.into_iter().collect();
    let bb: std::collections::HashSet<String> = bt.into_iter().collect();
    let inter = aa.intersection(&bb).count();
    (2.0 * inter as f64) / (aa.len() + bb.len()) as f64
}

fn list_live(conn: &rusqlite::Connection) -> Result<Vec<Observation>> {
    let ids: Vec<i64> = {
        let mut st = conn
            .prepare("SELECT id FROM observations WHERE deleted_at IS NULL ORDER BY created_at ASC, id ASC")
            .map_err(|e| Error::internal(format!("dream id list prepare: {e}")))?;
        let rows = st
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| Error::internal(format!("dream id list query: {e}")))?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| Error::internal(format!("dream id list collect: {e}")))?
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        match crate::read::get(conn, &crate::write::ObservationKey::Id(id)) {
            Ok(o) => out.push(o),
            Err(e) => debug!(id, error = %e, "dream: skip observation"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    fn conn() -> Connection {
        crate::pragmas::ensure_sqlite_vec_extension();
        let mut conn = Connection::open_in_memory().unwrap();
        crate::run_migrations(&mut conn).unwrap();
        conn
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_obs(
        conn: &Connection,
        sync_id: &str,
        scope: &str,
        obs_type: &str,
        title: &str,
        content: &str,
        created_at: &str,
        hash: Option<&str>,
    ) -> i64 {
        conn.execute(
            "INSERT OR IGNORE INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observations
                (sync_id, session_id, type, scope, title, content, created_at, normalized_hash)
             VALUES (?1, 's1', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![sync_id, obs_type, scope, title, content, created_at, hash],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn duplicate_hash_proposals() {
        let conn = conn();
        let a = insert_obs(
            &conn,
            "s-a",
            "project",
            "decision",
            "first take",
            "same body",
            "2026-01-01T00:00:00Z",
            Some("abc"),
        );
        let b = insert_obs(
            &conn,
            "s-b",
            "project",
            "decision",
            "second take",
            "same body",
            "2026-01-02T00:00:00Z",
            Some("abc"),
        );
        let props = dream_scan(&conn).unwrap();
        assert!(
            props.iter().any(|p| p.kind == "duplicate"
                && p.keep_id == b
                && p.drop_id == a
                && p.confidence == 1.0),
            "duplicate pair (keep newer #b, drop older #a) missing: {props:?}"
        );
    }

    #[test]
    fn overlapping_title_supersedes() {
        let conn = conn();
        let old = insert_obs(
            &conn,
            "s-c",
            "project",
            "decision",
            "Retry policy: backoff with jitter for db conn",
            "old body",
            "2026-01-01T00:00:00Z",
            None,
        );
        let new = insert_obs(
            &conn,
            "s-d",
            "project",
            "decision",
            "Retry policy: backoff with jitter for db conn (revised)",
            "new body",
            "2026-01-10T00:00:00Z",
            Some("def"),
        );
        let props = dream_scan(&conn).unwrap();
        assert!(
            props
                .iter()
                .any(|p| p.kind == "supersede" && p.keep_id == new && p.drop_id == old),
            "title-overlap supersede proposal missing: {props:?}"
        );
    }

    #[test]
    fn empty_db_no_proposals() {
        let conn = conn();
        assert!(dream_scan(&conn).unwrap().is_empty());
    }
}
