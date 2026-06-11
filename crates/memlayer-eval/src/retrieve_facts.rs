// Generated with AI Coding Rules Hub
//! Hybrid retrieval over `facts.db` with evidence expansion (spec-task-19).
//!
//! Pipeline:
//!   1. Embed `query` (cache lookup → embedder on miss).
//!   2. Open `facts.db`. **EH-5:** hard-fail with a remediation hint if the
//!      file is missing — the runner should have called `eval extract` first.
//!   3. In parallel (`spawn_blocking`):
//!        * BM25 top-100 over `facts_fts` filtered to `project`.
//!        * Dense ANN top-100 over `facts_vec` filtered to `project` via a
//!          join back to `facts`.
//!   4. RRF-fuse the two ranked id lists (`k_const = 60`). **EH-6:** if dense
//!      ANN is empty, log and fall back to BM25-only.
//!   5. Fetch the top-`k` fact rows preserving fused order.
//!   6. For each fact, expand its `evidence_obs_id` against the project's
//!      storage DB via `retrieve::expand_evidence`. **EH-8:** orphaned
//!      evidence (zero-row lookup) yields a fact-only hit, not an error.
//!
//! Boundaries (spec-task-19 only):
//!   * No entity boost / additive scoring (owned by spec-task-19d).
//!   * No 4× over-fetch (owned by spec-task-19e). This task uses a flat 100
//!     candidate window per retriever.
//!
//! SC-10: this module never touches `memlayer-storage` migrations or schema.
//! It only opens existing storage connections read-only via
//! `ProjectRegistry::open_read_conn` to read raw observations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::params;

use memlayer_embed::{cache::EmbeddingCache, Embedder};
use memlayer_storage::ProjectRegistry;

use crate::vec_index::open_with_vec;

#[derive(Debug, thiserror::Error)]
pub enum FactsRetrieveError {
    #[error("facts.db not found at {path}. Run `eval extract --benchmark <b>` first.")]
    FactsDbMissing { path: PathBuf },
}

/// Output of a hybrid facts retrieval call.
pub struct FactsHybridResult {
    /// Top-k formatted hits in fused-rank order. Each hit is the fact's
    /// `[temporal] subject predicate object` line, optionally followed by
    /// the expanded raw-observation evidence rows joined with `\n`.
    pub hits: Vec<String>,
    /// End-to-end latency (embed + BM25 + ANN + RRF + evidence expansion).
    pub latency: Duration,
    /// Number of fact rows considered before fusion (sum of BM25 + ANN).
    pub candidates_considered: usize,
}

/// Hybrid retrieval over `facts.db` for one project (one LoCoMo conversation
/// or LME session). See module-level docs for the full pipeline.
pub async fn retrieve_facts(
    data_dir: &Path,
    facts_db_path: &Path,
    project: &str,
    query: &str,
    k: i32,
    evidence_window: u8,
    embedder: Arc<dyn Embedder>,
    cache: Arc<EmbeddingCache>,
) -> Result<FactsHybridResult> {
    let t0 = Instant::now();

    // EH-5: facts.db must already exist; the runner is responsible for the
    // extract phase (`eval extract`). Surfacing this as an error keeps the
    // failure mode observable instead of silently returning zero hits.
    if !facts_db_path.exists() {
        return Err(FactsRetrieveError::FactsDbMissing {
            path: facts_db_path.to_path_buf(),
        }
        .into());
    }

    // Storage paths must be initialised before the registry opens any DB.
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);

    // 1. Embed the query (cache → fall through to embedder on miss).
    let (mut hits_cache, _miss_idx) =
        cache.get_many(&[query]).context("query cache lookup")?;
    let query_vec: Vec<f32> = if let Some(v) = hits_cache[0].take() {
        v
    } else {
        let mut vs = embedder
            .embed(&[query])
            .context("embed query (cache miss)")?;
        let v = vs.remove(0);
        cache
            .put_many(&[(query.to_string(), v.clone())])
            .context("populate cache after query embed")?;
        v
    };

    // 3. Parallel BM25 + ANN over blocking sqlite handles.
    let bm25_path = facts_db_path.to_path_buf();
    let bm25_proj = project.to_string();
    let bm25_query = query.to_string();
    let bm25_handle = tokio::task::spawn_blocking(move || {
        bm25_top_n(&bm25_path, &bm25_proj, &bm25_query, 100)
    });

    let ann_path = facts_db_path.to_path_buf();
    let ann_proj = project.to_string();
    let ann_query_vec = query_vec.clone();
    let ann_handle = tokio::task::spawn_blocking(move || {
        ann_top_n(&ann_path, &ann_proj, &ann_query_vec, 100)
    });

    let bm25_ids = bm25_handle.await.context("join facts BM25 task")??;
    let ann_ids = ann_handle.await.context("join facts ANN task")??;
    let candidates_considered = bm25_ids.len() + ann_ids.len();

    // 4. Fuse — or fall back to BM25 if dense returned nothing (EH-6).
    let fused: Vec<u64> = if ann_ids.is_empty() {
        tracing::warn!(
            target: "memlayer_eval::retrieve_facts",
            project = %project,
            "facts ANN returned 0 results; falling back to BM25-only ranking (EH-6)"
        );
        bm25_ids
    } else {
        crate::rrf::rrf_fuse(&[bm25_ids, ann_ids], 60)
    };
    let top_k: Vec<u64> = fused.into_iter().take(k.max(0) as usize).collect();

    // 5. Fetch the top-k fact rows, preserving fused order.
    let fetched_facts = fetch_facts_by_ids(facts_db_path, &top_k)
        .context("fetch facts by id")?;

    // 6. Open the storage project DB read-only ONCE for evidence expansion.
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
                // EH-8: orphan evidence_obs_id (no rows in storage). Emit the
                // fact only — losing one fact's evidence shouldn't kill the
                // whole retrieval.
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
    })
}

/// BM25 top-N over `facts_fts` filtered to `project`. Returns fact ids in
/// BM25-ranked order (best first). Equal column weights for now — entity
/// boost is owned by spec-task-19d.
fn bm25_top_n(
    facts_db_path: &Path,
    project: &str,
    query: &str,
    n: i32,
) -> Result<Vec<u64>> {
    let conn = open_with_vec(facts_db_path).context("open facts.db for BM25")?;
    let fts_query = crate::retrieve::tokenize(query);
    let limit = n.clamp(1, 1000);

    let mut stmt = conn
        .prepare(
            "SELECT f.id
               FROM facts_fts
               JOIN facts f ON f.id = facts_fts.rowid
              WHERE facts_fts MATCH ?1
                AND f.project = ?2
              ORDER BY bm25(facts_fts, 1.0, 1.0, 1.0, 1.0) ASC
              LIMIT ?3",
        )
        .context("prepare facts BM25 SELECT")?;
    let rows = stmt
        .query_map(params![fts_query, project, limit], |row| {
            let id: i64 = row.get(0)?;
            Ok(id as u64)
        })
        .context("run facts BM25 SELECT")?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r.context("read facts BM25 id row")?);
    }
    Ok(ids)
}

/// Dense ANN top-N against `facts_vec` filtered to `project` via a join back
/// to `facts`. `vec0`'s `MATCH` operator on a `float[N]` column expects an
/// `N*4`-byte little-endian BLOB.
fn ann_top_n(
    facts_db_path: &Path,
    project: &str,
    query_vec: &[f32],
    n: i32,
) -> Result<Vec<u64>> {
    let conn = open_with_vec(facts_db_path).context("open facts.db for ANN")?;
    let bytes: Vec<u8> = query_vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    let limit = n.clamp(1, 1000);

    // facts_vec.rowid maps directly to facts.id (no separate id_map for
    // the eval-side facts schema — see migrations_eval/V1__facts.sql).
    // We over-fetch from the vec table by `k` and post-filter to `project`,
    // because vec0 doesn't accept arbitrary WHERE predicates alongside MATCH.
    let mut stmt = conn
        .prepare(
            "SELECT f.id
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
            let id: i64 = row.get(0)?;
            Ok(id as u64)
        })
        .context("run facts ANN SELECT")?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r.context("read facts ANN id row")?);
    }
    Ok(ids)
}

/// Fetch the canonical fact rows for `ids`, preserving the input order.
/// Returns `(id, subject, predicate, object, temporal, evidence_obs_id)`.
/// Caller does not need to keep the connection alive — all data is owned.
fn fetch_facts_by_ids(
    facts_db_path: &Path,
    ids: &[u64],
) -> Result<Vec<(i64, String, String, String, Option<String>, i64)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = open_with_vec(facts_db_path).context("open facts.db for fetch")?;

    // Build "?,?,?,…" placeholder list. SQLite's bound-parameter limit is
    // 32766 by default; our k is tiny so we never approach it.
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

    // Rebuild in fused order. Skip ids that vanished between fusion and the
    // SELECT-back; the eval harness shouldn't crash on a stale snapshot.
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if let Some(row) = by_id.remove(&(id as i64)) {
            out.push(row);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts_db::FactsDb;
    use rusqlite::params;
    use tempfile::TempDir;

    /// `fetch_facts_by_ids` must preserve the order of its input id slice
    /// regardless of how SQLite returns the underlying `IN (...)` rows. This
    /// guards the contract relied on by RRF-fused output.
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

        // Request rows out of insertion order. SQLite's IN (...) makes no
        // ordering promise, so this exercises the HashMap rebuild.
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

    /// Missing ids are silently dropped (no error, no placeholder rows).
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

    /// Empty id slice returns empty without opening the DB.
    #[test]
    fn fetch_facts_by_ids_empty_input() {
        // Path doesn't even need to exist — the early return guards open.
        let rows = fetch_facts_by_ids(Path::new("/nonexistent/facts.db"), &[]).unwrap();
        assert!(rows.is_empty());
    }

    /// EH-5: `retrieve_facts` must hard-fail with `FactsDbMissing` when the
    /// facts.db file is absent. This path doesn't need an embedder so we use
    /// a stub.
    #[tokio::test]
    async fn retrieve_facts_eh5_missing_db() {
        struct StubEmbedder;
        impl Embedder for StubEmbedder {
            fn embed(&self, _texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
                Ok(vec![vec![0.0; 384]])
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
