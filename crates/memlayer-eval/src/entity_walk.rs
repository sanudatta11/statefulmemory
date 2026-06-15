// Generated with AI Coding Rules Hub
//! Entity-walk 2-hop reranking (P5 spec-task-31).
//!
//! Targets the "scattered fact" failure mode that a single-hop entity boost
//! and BM25/dense fusion can't catch: queries like "what activities does X
//! do" or "where has Y camped" need to aggregate facts that share an
//! entity through indirect chains rather than direct keyword/vector match.
//!
//! Algorithm (locked grill 2026-06-12):
//!
//!   Hop 0: query → heuristic entity tokens (already done in retrieve_facts).
//!   Hop 1: for each query entity, fetch entity_links → fact_ids.
//!          (This is the same set the existing entity_boost uses; it gets
//!           the direct-link credit there. We don't double-count it.)
//!   Hop 2: for each hop-1 fact, fetch its OTHER entities ("co-entities").
//!          For each co-entity, fetch ITS fact_ids ("second-order facts").
//!          Each second-order fact id gets +path_increment per distinct
//!          path that surfaced it.
//!
//! Caps (spec): top-5 query entities × top-10 second-order facts per
//! co-entity. Without these caps a popular entity (e.g. "Caroline" linked
//! to 200+ facts) would explode into O(n^2) joins.
//!
//! The result is a `HashMap<fact_id, walk_score>` clamped to [0, 1.0]; it
//! becomes the new `entity_walk_boost` field on `ScoreComponents` so it
//! adds to the additive base (sem + bm25 + entity_boost + walk_boost).
//!
//! Spec link: TS-19. Plan: analysis.md §6.5, §7.2.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::params_from_iter;
use rusqlite::types::Value as SqlValue;

use crate::vec_index::open_with_vec;

/// Top-N query entities to walk from. Locked-grill cap.
const MAX_QUERY_ENTITIES: usize = 5;
/// Top-N second-order facts per co-entity. Locked-grill cap.
const MAX_SECOND_ORDER_PER_ENTITY: usize = 10;
/// Per-path additive contribution to the walk score. With a cap on path
/// count this keeps the walk_boost in a sensible [0, 1] range.
const PATH_INCREMENT: f32 = 0.1;
/// Hard ceiling on the per-fact walk boost. Multiple distinct paths
/// reinforce a fact but we don't want runaway dominance.
const WALK_BOOST_CAP: f32 = 1.0;

/// Compute 2-hop entity walk boosts keyed by fact_id.
///
/// Returns an empty map when:
///   - query_entities is empty (nothing to walk from)
///   - no query entity matches a row in the `entities` table for this
///     project (entity registry has no overlap with the question)
///   - the project has no entity_links (P2 entity layer was skipped)
///
/// Errors only on storage failures (open, prepare, query).
pub fn compute_entity_walk_boost(
    facts_db_path: &Path,
    project: &str,
    query_entities: &[String],
) -> Result<HashMap<i64, f32>> {
    if query_entities.is_empty() {
        return Ok(HashMap::new());
    }

    let conn = open_with_vec(facts_db_path)
        .context("open facts.db for entity walk")?;

    // Step 1: query entity names → entity_ids. Heuristic entity strings
    // are already lowercased; the entities table stores lowercased names.
    let cap_q: Vec<&str> = query_entities
        .iter()
        .take(MAX_QUERY_ENTITIES)
        .map(String::as_str)
        .collect();
    let placeholders = std::iter::repeat("?")
        .take(cap_q.len())
        .collect::<Vec<_>>()
        .join(",");
    let mut params: Vec<SqlValue> = Vec::with_capacity(cap_q.len() + 1);
    params.push(SqlValue::Text(project.to_string()));
    for q in &cap_q {
        params.push(SqlValue::Text((*q).to_string()));
    }

    let sql = format!(
        "SELECT id FROM entities WHERE project = ?1 AND name IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql).context("prepare entity-id lookup")?;
    let q_entity_ids: Vec<i64> = stmt
        .query_map(params_from_iter(params.iter()), |row| row.get::<_, i64>(0))
        .context("run entity-id lookup")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect entity-id lookup rows")?;

    if q_entity_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Step 2: hop-1 fact_ids per query entity. Bounded by entity_links
    // table size; in practice <100 facts per entity.
    let mut hop1_facts: HashSet<i64> = HashSet::new();
    {
        let placeholders = std::iter::repeat("?")
            .take(q_entity_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT DISTINCT fact_id FROM entity_links WHERE entity_id IN ({placeholders})"
        );
        let id_params: Vec<SqlValue> =
            q_entity_ids.iter().map(|i| SqlValue::Integer(*i)).collect();
        let mut stmt = conn.prepare(&sql).context("prepare hop1 lookup")?;
        let rows = stmt
            .query_map(params_from_iter(id_params.iter()), |row| {
                row.get::<_, i64>(0)
            })
            .context("run hop1 lookup")?;
        for r in rows {
            hop1_facts.insert(r.context("read hop1 row")?);
        }
    }

    if hop1_facts.is_empty() {
        return Ok(HashMap::new());
    }

    // Step 3: co-entities = entities that share any hop-1 fact with a
    // query entity, EXCLUDING the query entities themselves. These are
    // the bridges to second-order facts.
    let mut co_entity_ids: HashSet<i64> = HashSet::new();
    {
        let h1_placeholders = std::iter::repeat("?")
            .take(hop1_facts.len())
            .collect::<Vec<_>>()
            .join(",");
        let q_set: HashSet<i64> = q_entity_ids.iter().copied().collect();
        let sql = format!(
            "SELECT DISTINCT entity_id FROM entity_links WHERE fact_id IN ({h1_placeholders})"
        );
        let id_params: Vec<SqlValue> =
            hop1_facts.iter().map(|i| SqlValue::Integer(*i)).collect();
        let mut stmt = conn.prepare(&sql).context("prepare co-entity lookup")?;
        let rows = stmt
            .query_map(params_from_iter(id_params.iter()), |row| {
                row.get::<_, i64>(0)
            })
            .context("run co-entity lookup")?;
        for r in rows {
            let id = r.context("read co-entity row")?;
            if !q_set.contains(&id) {
                co_entity_ids.insert(id);
            }
        }
    }

    if co_entity_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Step 4: second-order facts = facts linked to co-entities, EXCLUDING
    // hop-1 facts (those already get direct entity_boost credit).
    // Per spec cap, take at most MAX_SECOND_ORDER_PER_ENTITY per co-entity.
    let mut walk_scores: HashMap<i64, f32> = HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT fact_id FROM entity_links WHERE entity_id = ?1 LIMIT ?2",
            )
            .context("prepare second-order lookup")?;
        for &ce_id in &co_entity_ids {
            let rows = stmt
                .query_map(
                    rusqlite::params![ce_id, MAX_SECOND_ORDER_PER_ENTITY as i64],
                    |row| row.get::<_, i64>(0),
                )
                .context("run second-order lookup")?;
            for r in rows {
                let fact_id = r.context("read second-order row")?;
                if hop1_facts.contains(&fact_id) {
                    continue;
                }
                let s = walk_scores.entry(fact_id).or_insert(0.0);
                *s = (*s + PATH_INCREMENT).min(WALK_BOOST_CAP);
            }
        }
    }

    Ok(walk_scores)
}
