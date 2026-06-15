// Generated with AI Coding Rules Hub
//! Hybrid retrieval over `facts.db` with evidence expansion.
//!
//! Pipeline (spec-task-19d, additive scoring):
//!   1. Embed `query` (cache lookup → embedder on miss).
//!   2. Extract query entities via the heuristic tokenizer (no LLM at query
//!      time — latency budget) and embed each one.
//!   3. In parallel (`spawn_blocking`):
//!        * BM25 top-N over `facts_fts` filtered to `project` — returns
//!          (fact_id, raw bm25 score).
//!        * Dense ANN top-N over `facts_vec` joined to `facts` — returns
//!          (fact_id, L2 distance).
//!        * For each query entity: name match (SQL) AND vec ANN over
//!          `entities_vec` at distance ≤ 0.7. Both paths feed
//!          `entity_links` to find linked fact_ids; their union drives a
//!          per-fact boost = 0.5 per matched query entity, capped at 1.0.
//!   4. Build a score map via `scoring::build_score_map`. Combined score
//!      is `sem + bm25 + entity_boost` (each in [0, 1]).
//!   5. Sort facts by combined desc; take top-k.
//!   6. Fetch full fact rows preserving order. **EH-8:** orphaned evidence
//!      (zero-row lookup) yields a fact-only hit, not an error.
//!
//! Boundaries:
//!   * No 4× over-fetch (owned by spec-task-19e). This task uses a flat
//!     `CANDIDATES_PER_RETRIEVER` (=100) window per source.
//!   * The `rrf` module is preserved for benchmarking and future modes;
//!     this module no longer calls it.
//!
//! SC-10: this module never touches `memlayer-storage` migrations or schema.
//! It only opens existing storage connections read-only via
//! `ProjectRegistry::open_read_conn` to read raw observations.
//! Spec links: TS-21, TS-22, SC-12, SC-13. Plan: P2 §4.5.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::params;

use memlayer_embed::{cache::EmbeddingCache, Embedder};
use memlayer_extract::entities::extract_heuristic_tokens;
use memlayer_storage::ProjectRegistry;

use crate::scoring::{
    apply_quality_modifiers, build_score_map, Bm25Norm, FactMeta, QualityConfig,
};
use crate::vec_index::open_with_vec;

/// Over-fetch policy: pull `max(k * 4, 60)` candidates per retriever before
/// rescoring. Mirrors Mem0's design (audit, spec §3) — recall improves
/// roughly free at this scale because additive scoring rebalances the
/// inflated candidate pool. spec-task-19e / TS-23 pins this formula.
fn over_fetch_n(k: i32) -> i32 {
    let from_k = k.saturating_mul(4);
    from_k.max(60)
}

/// Max number of entity matches that contribute to a fact's boost. With
/// `BOOST_PER_ENTITY = 0.5`, the cap of 1.0 is hit at 2 distinct matches.
const BOOST_PER_ENTITY: f32 = 0.5;
const ENTITY_BOOST_CAP: f32 = 1.0;

/// L2 distance threshold for the entities_vec ANN match. With BGE-small's
/// L2-normalized embeddings this corresponds roughly to cosine ≥ 0.5.
/// Tunable; spec-task-19e may revisit.
const ENTITY_VEC_DISTANCE_THRESHOLD: f64 = 0.7;

/// Top-N entity matches per query entity (vec sim path).
const ENTITY_VEC_TOP_N: i32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum FactsRetrieveError {
    #[error("facts.db not found at {path}. Run `eval extract --benchmark <b>` first.")]
    FactsDbMissing { path: PathBuf },
}

/// Output of a hybrid facts retrieval call.
#[derive(Debug)]
pub struct FactsHybridResult {
    /// Top-k formatted hits in score-ranked order. Each hit is the fact's
    /// `[temporal] subject predicate object` line, optionally followed by
    /// the expanded raw-observation evidence rows joined with `\n`.
    pub hits: Vec<String>,
    /// End-to-end latency (embed + BM25 + ANN + entity boost + scoring +
    /// evidence expansion).
    pub latency: Duration,
    /// Number of fact rows considered before scoring (sum of BM25 + ANN +
    /// entity-boosted ids).
    pub candidates_considered: usize,
    /// Score delta between rank-1 and rank-2 after fusion + quality
    /// modifiers. Used by the runner to gate the LLM rerank call —
    /// when this is large the top hit is unambiguous and rerank is a
    /// waste of money. None when fewer than 2 hits were returned.
    pub top2_delta: Option<f32>,
}

/// Hybrid retrieval over `facts.db` for one project. See module-level docs
/// for the full pipeline.
pub async fn retrieve_facts(
    data_dir: &Path,
    facts_db_path: &Path,
    project: &str,
    query: &str,
    k: i32,
    evidence_window: u8,
    embedder: Arc<dyn Embedder>,
    cache: Arc<EmbeddingCache>,
    decay_lambda: f64,
) -> Result<FactsHybridResult> {
    let t0 = Instant::now();

    // EH-5: facts.db must already exist; the runner is responsible for the
    // extract phase (`eval extract`). Surface this as an error so the
    // failure mode is observable instead of silently zero-hit.
    if !facts_db_path.exists() {
        return Err(FactsRetrieveError::FactsDbMissing {
            path: facts_db_path.to_path_buf(),
        }
        .into());
    }

    // Storage paths must be initialised before the registry opens any DB.
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);

    // 1. Embed the query.
    let query_vec = embed_with_cache(query, &embedder, &cache)
        .context("embed query")?;

    // 2. Heuristic entity extraction over the query. No LLM here — query
    //    latency budget rules out Haiku at retrieval time.
    let query_entities: Vec<String> = extract_heuristic_tokens(query);

    // 3. Embed each query entity (cached). 1-3 entities is the typical
    //    case so this stays cheap. We dedupe inside the heuristic, so the
    //    list is already unique.
    let mut query_entity_vecs: Vec<(String, Vec<f32>)> =
        Vec::with_capacity(query_entities.len());
    for ent in &query_entities {
        let v = embed_with_cache(ent, &embedder, &cache)
            .with_context(|| format!("embed query entity '{ent}'"))?;
        query_entity_vecs.push((ent.clone(), v));
    }

    // 4. Parallel BM25 + dense ANN. Over-fetch per spec-task-19e / TS-23.
    let n = over_fetch_n(k);
    let bm25_path = facts_db_path.to_path_buf();
    let bm25_proj = project.to_string();
    let bm25_query = query.to_string();
    let bm25_handle = tokio::task::spawn_blocking(move || {
        bm25_top_n(&bm25_path, &bm25_proj, &bm25_query, n)
    });

    let ann_path = facts_db_path.to_path_buf();
    let ann_proj = project.to_string();
    let ann_query_vec = query_vec.clone();
    let ann_handle = tokio::task::spawn_blocking(move || {
        ann_top_n(&ann_path, &ann_proj, &ann_query_vec, n)
    });

    let bm25_hits = bm25_handle.await.context("join facts BM25 task")??;
    let dense_hits = ann_handle.await.context("join facts ANN task")??;

    // 5. Entity boost — both paths (name match + vec sim) feed entity_links.
    let entity_boost_map = {
        let path = facts_db_path.to_path_buf();
        let proj = project.to_string();
        let names = query_entities.clone();
        let entity_vecs = query_entity_vecs.clone();
        tokio::task::spawn_blocking(move || {
            compute_entity_boost(&path, &proj, &names, &entity_vecs)
        })
        .await
        .context("join entity boost task")??
    };

    let candidates_considered =
        bm25_hits.len() + dense_hits.len() + entity_boost_map.len();

    // EH-6: if dense ANN is empty AND BM25 is empty AND no entity matches,
    // there's nothing to score. Return empty cleanly.
    if bm25_hits.is_empty() && dense_hits.is_empty() && entity_boost_map.is_empty() {
        return Ok(FactsHybridResult {
            hits: Vec::new(),
            latency: t0.elapsed(),
            candidates_considered: 0,
            top2_delta: None,
        });
    }
    if dense_hits.is_empty() {
        tracing::warn!(
            target: "memlayer_eval::retrieve_facts",
            project = %project,
            "facts ANN returned 0 results; relying on BM25 + entity boost only (EH-6)"
        );
    }

    // 6. Build the additive score map and rank.
    //    P5 spec-task-28: BM25 normalization is now an adaptive sigmoid
    //    (Mem0 formula) keyed off query token count. P5 spec-task-27/29/30:
    //    salience multiplier (0.3 floor) + time-decay (λ from config) +
    //    contradiction penalty (0.5× both, +0.25 to higher-salience).
    //    P5 spec-task-31: entity-walk 2-hop adds entity_walk_boost to
    //    the additive base — surfaces facts connected via shared
    //    co-entities even when no direct BM25/dense/boost hit fires.
    let query_token_count = query.split_whitespace().count();
    let mut scored = build_score_map(
        &bm25_hits,
        &dense_hits,
        &entity_boost_map,
        Bm25Norm::AdaptiveSigmoid { query_token_count },
    );

    // Entity-walk 2-hop boost: fold into the score map BEFORE quality
    // modifiers so salience/decay/contradiction multiply through the
    // walk contribution as well.
    let walk_boosts = crate::entity_walk::compute_entity_walk_boost(
        facts_db_path,
        project,
        &query_entities,
    )
    .context("entity-walk 2-hop boost")?;
    if !walk_boosts.is_empty() {
        for (fact_id, boost) in &walk_boosts {
            scored.entry(*fact_id).or_default().entity_walk_boost = *boost;
        }
        tracing::debug!(
            target: "memlayer_eval::retrieve_facts",
            project = %project,
            walk_facts = walk_boosts.len(),
            "entity-walk 2-hop populated boosts"
        );
    }

    // Fetch fact metadata for every scored id so the quality modifiers
    // have salience + temporal + (subject, predicate, object) to work
    // with. Single SELECT keyed by IN-list.
    let scored_ids: Vec<i64> = scored.keys().copied().collect();
    let meta_by_id = fetch_meta_for_ids(facts_db_path, &scored_ids)
        .context("fetch fact meta for scoring")?;
    let now_unix = chrono::Utc::now().timestamp();
    let quality_cfg = QualityConfig {
        decay_lambda,
        now_unix,
        ..QualityConfig::with_now(now_unix)
    };
    apply_quality_modifiers(&mut scored, &meta_by_id, &quality_cfg);

    let mut ranked: Vec<(i64, f32)> = scored
        .iter()
        .map(|(id, sc)| (*id, sc.combined()))
        .collect();
    // Stable secondary sort by id keeps output deterministic on score ties.
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });

    // Top-2 score delta — surfaced for the rerank-on-ambiguity gate
    // (P5 spec-task-31): the runner skips the LLM rerank call when
    // this delta is large (clear winner).
    let top2_delta = if ranked.len() >= 2 {
        Some((ranked[0].1 - ranked[1].1).max(0.0))
    } else {
        None
    };

    let k_usize = k.max(0) as usize;
    let top_k_ids: Vec<u64> = ranked
        .into_iter()
        .take(k_usize)
        .map(|(id, _)| id as u64)
        .collect();

    // 7. Fetch the top-k fact rows preserving rank order.
    let fetched_facts = fetch_facts_by_ids(facts_db_path, &top_k_ids)
        .context("fetch facts by id")?;

    // 8. Open the storage project DB read-only ONCE for evidence expansion.
    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project_state = registry
        .get_or_open(project)
        .with_context(|| format!("open project '{project}' for evidence expansion"))?;
    let storage_conn = project_state
        .open_read_conn()
        .context("open storage read connection")?;

    let mut formatted: Vec<String> = Vec::with_capacity(fetched_facts.len());
    for (id, subject, predicate, object, temporal, evidence_obs_id) in fetched_facts {
        let fact_line = if let Some(t) = temporal {
            format!("[{t}] {subject} {predicate} {object}")
        } else {
            format!("{subject} {predicate} {object}")
        };

        let evidence = match crate::retrieve::expand_evidence(
            &storage_conn,
            evidence_obs_id,
            evidence_window,
        ) {
            Ok(rows) if !rows.is_empty() => rows
                .into_iter()
                .map(|(t, c)| format!("  - {t}: {c}"))
                .collect::<Vec<_>>()
                .join("\n"),
            Ok(_) => {
                // EH-8: orphan evidence_obs_id (no rows in storage). Emit
                // the fact only — losing one fact's evidence shouldn't
                // kill the whole retrieval.
                tracing::warn!(
                    target: "memlayer_eval::retrieve_facts",
                    project = %project,
                    evidence_obs_id = %evidence_obs_id,
                    fact_id = %id,
                    "fact has no expandable evidence; including fact-only (EH-8)"
                );
                String::new()
            }
            Err(e) => {
                tracing::warn!(
                    target: "memlayer_eval::retrieve_facts",
                    project = %project,
                    evidence_obs_id = %evidence_obs_id,
                    fact_id = %id,
                    error = %e,
                    "expand_evidence failed; including fact-only"
                );
                String::new()
            }
        };

        let combined = if evidence.is_empty() {
            fact_line
        } else {
            format!("{fact_line}\n{evidence}")
        };
        formatted.push(combined);
    }

    Ok(FactsHybridResult {
        hits: formatted,
        latency: t0.elapsed(),
        candidates_considered,
        top2_delta,
    })
}

/// Embed `text`, hitting the on-disk cache before the model.
fn embed_with_cache(
    text: &str,
    embedder: &Arc<dyn Embedder>,
    cache: &Arc<EmbeddingCache>,
) -> Result<Vec<f32>> {
    let (mut hits_cache, _miss_idx) = cache.get_many(&[text]).context("cache lookup")?;
    if let Some(v) = hits_cache[0].take() {
        return Ok(v);
    }
    let mut vs = embedder
        .embed(&[text])
        .context("embed (cache miss)")?;
    let v = vs.remove(0);
    cache
        .put_many(&[(text.to_string(), v.clone())])
        .context("populate cache after embed")?;
    Ok(v)
}

/// BM25 top-N over `facts_fts` filtered to `project`. Returns
/// `(fact_id, raw_bm25_score)` in BM25-ranked order (best first).
fn bm25_top_n(
    facts_db_path: &Path,
    project: &str,
    query: &str,
    n: i32,
) -> Result<Vec<(i64, f64)>> {
    let conn = open_with_vec(facts_db_path).context("open facts.db for BM25")?;
    let fts_query = crate::retrieve::tokenize(query);
    let limit = n.clamp(1, 1000);

    let mut stmt = conn
        .prepare(
            "SELECT f.id, bm25(facts_fts, 5.0, 1.0, 3.0, 0.5)
               FROM facts_fts
               JOIN facts f ON f.id = facts_fts.rowid
              WHERE facts_fts MATCH ?1
                AND f.project = ?2
              ORDER BY bm25(facts_fts, 5.0, 1.0, 3.0, 0.5) ASC
              LIMIT ?3",
        )
        .context("prepare facts BM25 SELECT")?;
    let rows = stmt
        .query_map(params![fts_query, project, limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })
        .context("run facts BM25 SELECT")?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.context("read facts BM25 row")?);
    }
    Ok(out)
}

/// Dense ANN top-N against `facts_vec` filtered to `project`. Returns
/// `(fact_id, distance)` ordered by distance asc. vec0's MATCH on a
/// `float[N]` column expects an `N*4`-byte little-endian BLOB.
fn ann_top_n(
    facts_db_path: &Path,
    project: &str,
    query_vec: &[f32],
    n: i32,
) -> Result<Vec<(i64, f64)>> {
    let conn = open_with_vec(facts_db_path).context("open facts.db for ANN")?;
    let bytes: Vec<u8> = query_vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    let limit = n.clamp(1, 1000);

    let mut stmt = conn
        .prepare(
            "SELECT f.id, distance
               FROM facts_vec v
               JOIN facts f ON f.id = v.rowid
              WHERE v.embedding MATCH ?1
                AND k = ?2
                AND f.project = ?3
              ORDER BY distance ASC",
        )
        .context("prepare facts ANN SELECT")?;
    let rows = stmt
        .query_map(params![bytes, limit, project], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })
        .context("run facts ANN SELECT")?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.context("read facts ANN row")?);
    }
    Ok(out)
}

/// Compute the per-fact entity boost from query entities.
///
/// Two parallel paths feed `entity_links`:
///   1. **Name match** (SQL): exact `(project, name)` lookup against
///      `entities`. Cheap, deterministic, and covers direct mentions.
///   2. **Vector match** (ANN): top-N over `entities_vec` at distance
///      ≤ `ENTITY_VEC_DISTANCE_THRESHOLD`. Catches synonyms / paraphrases.
///
/// For each fact, we count the number of *distinct query entities* (by
/// query-entity name, not by matched entity row) that resolve to a link.
/// The boost is `BOOST_PER_ENTITY * count`, capped at `ENTITY_BOOST_CAP`.
fn compute_entity_boost(
    facts_db_path: &Path,
    project: &str,
    query_entity_names: &[String],
    query_entity_vecs: &[(String, Vec<f32>)],
) -> Result<HashMap<i64, f32>> {
    if query_entity_names.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = open_with_vec(facts_db_path).context("open facts.db for entity boost")?;

    // For each query entity name, collect the set of fact_ids it links to
    // (via either name match or vec ANN). We accumulate per-query-entity
    // sets so we can compute "distinct query entities matching" per fact
    // — the boost cap is on that count, not on the row count.
    let mut per_query_entity_links: HashMap<String, HashSet<i64>> = HashMap::new();

    // Path 1: exact name match.
    {
        let mut stmt = conn
            .prepare(
                "SELECT el.fact_id
                   FROM entities e
                   JOIN entity_links el ON el.entity_id = e.id
                  WHERE e.project = ?1 AND e.name = ?2",
            )
            .context("prepare entity name-match SELECT")?;
        for name in query_entity_names {
            let rows = stmt
                .query_map(params![project, name], |r| r.get::<_, i64>(0))
                .context("run entity name-match SELECT")?;
            let entry = per_query_entity_links
                .entry(name.clone())
                .or_default();
            for r in rows {
                let fact_id = r.context("read entity name-match row")?;
                entry.insert(fact_id);
            }
        }
    }

    // Path 2: entities_vec ANN. For each query entity vec, top-N matches
    // at distance ≤ threshold, then JOIN entity_links. We prepare per-query
    // because vec0 binds a single query vector per statement execution.
    {
        let mut stmt = conn
            .prepare(
                "SELECT el.fact_id, ev.distance
                   FROM entities_vec ev
                   JOIN entities e ON e.id = ev.rowid
                   JOIN entity_links el ON el.entity_id = e.id
                  WHERE ev.embedding MATCH ?1
                    AND k = ?2
                    AND e.project = ?3
                    AND ev.distance <= ?4
                  ORDER BY ev.distance ASC",
            )
            .context("prepare entity vec ANN SELECT")?;
        for (name, vec) in query_entity_vecs {
            let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
            let rows = stmt
                .query_map(
                    params![
                        bytes,
                        ENTITY_VEC_TOP_N,
                        project,
                        ENTITY_VEC_DISTANCE_THRESHOLD,
                    ],
                    |r| Ok(r.get::<_, i64>(0)?),
                )
                .context("run entity vec ANN SELECT")?;
            let entry = per_query_entity_links
                .entry(name.clone())
                .or_default();
            for r in rows {
                let fact_id = r.context("read entity vec ANN row")?;
                entry.insert(fact_id);
            }
        }
    }

    // Aggregate: for each fact, count the number of distinct query entities
    // that matched it (across both paths).
    let mut per_fact_count: HashMap<i64, u32> = HashMap::new();
    for fact_set in per_query_entity_links.values() {
        for fact_id in fact_set {
            *per_fact_count.entry(*fact_id).or_insert(0) += 1;
        }
    }

    // Convert counts to capped boost values.
    let mut out: HashMap<i64, f32> = HashMap::with_capacity(per_fact_count.len());
    for (fact_id, count) in per_fact_count {
        let boost = (BOOST_PER_ENTITY * count as f32).min(ENTITY_BOOST_CAP);
        out.insert(fact_id, boost);
    }
    Ok(out)
}

/// Fetch the canonical fact rows for `ids`, preserving the input order.
/// Returns `(id, subject, predicate, object, temporal, evidence_obs_id)`.
fn fetch_facts_by_ids(
    facts_db_path: &Path,
    ids: &[u64],
) -> Result<Vec<(i64, String, String, String, Option<String>, i64)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = open_with_vec(facts_db_path).context("open facts.db for fetch")?;

    let placeholders = std::iter::repeat("?")
        .take(ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, subject, predicate, object, temporal, evidence_obs_id \
           FROM facts \
          WHERE id IN ({placeholders})"
    );

    let mut stmt = conn.prepare(&sql).context("prepare facts SELECT")?;
    let id_params: Vec<rusqlite::types::Value> = ids
        .iter()
        .map(|i| rusqlite::types::Value::Integer(*i as i64))
        .collect();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(id_params.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .context("run facts SELECT")?;

    let mut by_id: HashMap<i64, (i64, String, String, String, Option<String>, i64)> =
        HashMap::with_capacity(ids.len());
    for r in rows {
        let row = r.context("read facts row")?;
        by_id.insert(row.0, row);
    }

    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if let Some(row) = by_id.remove(&(id as i64)) {
            out.push(row);
        }
    }
    Ok(out)
}

/// Fetch (salience, temporal, subject, predicate, object) for every id
/// in the score map. Used by P5 quality modifiers (spec-task-27/29/30).
/// Single SELECT — facts.db is local SQLite, the IN-list is bounded by
/// over-fetch (~60 ids).
fn fetch_meta_for_ids(
    facts_db_path: &Path,
    ids: &[i64],
) -> Result<HashMap<i64, FactMeta>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = open_with_vec(facts_db_path).context("open facts.db for meta fetch")?;
    let placeholders = std::iter::repeat("?")
        .take(ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, salience, temporal, subject, predicate, object \
           FROM facts \
          WHERE id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql).context("prepare meta SELECT")?;
    let id_params: Vec<rusqlite::types::Value> = ids
        .iter()
        .map(|i| rusqlite::types::Value::Integer(*i))
        .collect();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(id_params.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .context("run meta SELECT")?;
    let mut out: HashMap<i64, FactMeta> = HashMap::with_capacity(ids.len());
    for r in rows {
        let (id, salience, temporal, subject, predicate, object) =
            r.context("read meta row")?;
        let temporal_unix = temporal.as_deref().and_then(parse_temporal_to_unix);
        out.insert(
            id,
            FactMeta {
                salience: salience as f32,
                temporal_unix,
                subject,
                predicate,
                object,
            },
        );
    }
    Ok(out)
}

/// Parse the extractor's temporal text into a unix timestamp. Tolerant
/// to several formats the prompt has produced in practice:
///   - `"2023-05-25"` → 2023-05-25 00:00 UTC
///   - `"[2023-05-25]"` → same, brackets stripped
///   - `"2023-05"` → 2023-05-01 00:00 UTC
///   - `"2023"` → 2023-01-01 00:00 UTC
/// Anything we can't parse returns None (decay disabled for that fact).
fn parse_temporal_to_unix(raw: &str) -> Option<i64> {
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    let s = raw
        .trim()
        .trim_matches(|c: char| c == '[' || c == ']')
        .trim();
    if s.is_empty() {
        return None;
    }
    // Full date.
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(NaiveDateTime::new(d, NaiveTime::MIN).and_utc().timestamp());
    }
    // Year-month.
    if let Ok(d) = NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d") {
        return Some(NaiveDateTime::new(d, NaiveTime::MIN).and_utc().timestamp());
    }
    // Year only.
    if let Ok(year) = s.parse::<i32>() {
        if (1900..3000).contains(&year) {
            if let Some(d) = NaiveDate::from_ymd_opt(year, 1, 1) {
                return Some(NaiveDateTime::new(d, NaiveTime::MIN).and_utc().timestamp());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts_db::FactsDb;
    use rusqlite::params;
    use tempfile::TempDir;

    #[test]
    fn ts23_over_fetch_uses_4x_with_floor_60() {
        // k=10 -> 4*10=40 -> clamped up to 60.
        assert_eq!(over_fetch_n(10), 60);
        // k=20 -> 4*20=80 -> 80 (above floor).
        assert_eq!(over_fetch_n(20), 80);
        // k=1 -> 4 -> floor 60.
        assert_eq!(over_fetch_n(1), 60);
        // k=100 -> 400.
        assert_eq!(over_fetch_n(100), 400);
        // k=0 -> 0 -> floor 60.
        assert_eq!(over_fetch_n(0), 60);
    }

    /// `fetch_facts_by_ids` must preserve the order of its input id slice
    /// regardless of how SQLite returns the underlying `IN (...)` rows.
    #[test]
    fn fetch_facts_by_ids_preserves_input_order() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("facts.db");
        let db = FactsDb::open(&path).expect("open facts.db");

        for (subj, pred, obj, temporal, obs_id) in [
            ("Alice", "knows", "Bob", Some("2024-01-01"), 10_i64),
            ("Carol", "met", "Dave", None, 20_i64),
            ("Eve", "saw", "Frank", Some("2024-02-02"), 30_i64),
        ] {
            db.conn
                .execute(
                    "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, temporal, salience) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params!["proj", obs_id, subj, pred, obj, temporal, 1.0_f64],
                )
                .unwrap();
        }
        drop(db);

        let ids = vec![3_u64, 1, 2];
        let rows = fetch_facts_by_ids(&path, &ids).expect("fetch");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, 3);
        assert_eq!(rows[0].1, "Eve");
        assert_eq!(rows[0].4.as_deref(), Some("2024-02-02"));
        assert_eq!(rows[0].5, 30);
        assert_eq!(rows[1].0, 1);
        assert_eq!(rows[1].1, "Alice");
        assert_eq!(rows[2].0, 2);
        assert_eq!(rows[2].1, "Carol");
        assert_eq!(rows[2].4, None);
    }

    #[test]
    fn fetch_facts_by_ids_skips_unknown_ids() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("facts.db");
        let db = FactsDb::open(&path).expect("open facts.db");
        db.conn
            .execute(
                "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, temporal, salience) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params!["proj", 1_i64, "S", "P", "O", Option::<&str>::None, 1.0_f64],
            )
            .unwrap();
        drop(db);

        let rows = fetch_facts_by_ids(&path, &[42_u64, 1, 99]).expect("fetch");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 1);
    }

    #[test]
    fn fetch_facts_by_ids_empty_input() {
        let rows = fetch_facts_by_ids(Path::new("/nonexistent/facts.db"), &[]).unwrap();
        assert!(rows.is_empty());
    }

    /// EH-5: `retrieve_facts` must hard-fail with `FactsDbMissing` when the
    /// facts.db file is absent.
    #[tokio::test]
    async fn retrieve_facts_eh5_missing_db() {
        struct StubEmbedder;
        impl Embedder for StubEmbedder {
            fn embed(&self, _texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
                Ok(vec![vec![0.0; 384]])
            }
            fn dim(&self) -> usize {
                384
            }
        }

        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let facts_db_path = data_dir.join("missing").join("facts.db");
        let cache = Arc::new(EmbeddingCache::open(&data_dir).unwrap());
        let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder);

        let err = retrieve_facts(
            &data_dir,
            &facts_db_path,
            "proj",
            "anything",
            5,
            1,
            embedder,
            cache,
            0.005,
        )
        .await
        .expect_err("must reject missing facts.db");

        let downcast = err
            .downcast_ref::<FactsRetrieveError>()
            .expect("FactsRetrieveError variant");
        match downcast {
            FactsRetrieveError::FactsDbMissing { path } => {
                assert_eq!(path, &facts_db_path);
            }
        }
    }
}
