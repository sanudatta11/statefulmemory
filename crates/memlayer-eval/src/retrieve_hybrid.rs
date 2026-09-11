// Generated with AI Coding Rules Hub
//! Hybrid retrieval: BM25 + dense ANN (sqlite-vec) fused via RRF.
//!
//! Spec links: TS-5 (semantic synonym retrieval), EH-6 (ANN-empty fallback),
//! SC-5 (storage layer untouched). Plan §3.2, §3.5.
//!
//! Pipeline:
//!   1. Embed `query` (cache lookup → embedder on miss).
//!   2. Lazy-bootstrap the project's sidecar `<data_dir>/vec/<project>.vec.db`
//!      if it is missing or out-of-sync with the project DB. Embeddings flow
//!      through the shared `EmbeddingCache` so re-runs are free.
//!   3. Run BM25 top-100 (over `observations_fts`) and dense ANN top-100
//!      (over `observations_vec`) in parallel via `spawn_blocking` — both are
//!      sync sqlite calls and we want the embedder thread to overlap with
//!      tokenization work.
//!   4. Fuse the two ranked id-lists with `rrf::rrf_fuse(.., k_const=60)`.
//!   5. SELECT title+content for the fused ids, preserving fused order.
//!
//! **EH-6:** if dense ANN returns 0 ids (e.g. the sidecar is empty or the
//! query embedding has no cosine neighbours), fall back to the BM25 ranking
//! alone instead of failing. We log a warning so eval runs can spot the
//! degenerate case.
//!
//! **Lazy embedding bootstrap** is intentionally cheap-checked: if the
//! sidecar's `observations_vec` row count already matches the project DB's
//! non-deleted observation count, we skip the rebuild. This keeps LoCoMo /
//! LongMemEval re-runs fast. BEAM-scale (1M+) is meant to use an offline
//! `eval prepare-vec` (P4) and never hits the lazy path here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::params;

use memlayer_core::paths;
use memlayer_embed::{cache::EmbeddingCache, Embedder};
use memlayer_storage::ProjectRegistry;

use crate::vec_index::{bootstrap_vec_index, open_with_vec};

/// Output of a hybrid retrieval call.
pub struct HybridResult {
    /// Top-k hits as `"<title>\n<content>"` strings, in fused-rank order.
    pub hits: Vec<String>,
    /// End-to-end retrieval latency (embed + BM25 + ANN + RRF + SELECT-back).
    pub latency: Duration,
}

/// Hybrid retrieval over `project`.
///
/// See module-level doc for the full pipeline. Returns up to `k` formatted
/// hits in fused-rank order plus the end-to-end latency.
pub async fn retrieve_hybrid(
    data_dir: &Path,
    project: &str,
    query: &str,
    k: i32,
    embedder: Arc<dyn Embedder>,
    cache: Arc<EmbeddingCache>,
) -> Result<HybridResult> {
    let t0 = Instant::now();

    // Storage paths must be initialised before the registry opens any DB.
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);
    paths::ensure_dirs(data_dir).ok();

    // 1. Embed the query (cache → fall through to embedder on miss).
    let (mut hits, _miss_idx) = cache.get_many(&[query]).context("query cache lookup")?;
    let query_vec: Vec<f32> = if let Some(v) = hits[0].take() {
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

    // 2. Ensure the sidecar vec DB has embeddings for every live observation.
    //    Synchronous: the cost only materialises on a fresh project, and the
    //    embedder is borrowed (not Cloned/Arc'd) for clarity.
    ensure_vec_index(data_dir, project, embedder.as_ref(), cache.as_ref())
        .context("ensure sidecar vec index is populated")?;

    // 3. Parallel BM25 + ANN over blocking sqlite handles.
    let bm25_data_dir = data_dir.to_path_buf();
    let bm25_project = project.to_string();
    let bm25_query = query.to_string();
    let bm25_handle = tokio::task::spawn_blocking(move || {
        bm25_top_n(&bm25_data_dir, &bm25_project, &bm25_query, 100)
    });

    let ann_data_dir = data_dir.to_path_buf();
    let ann_project = project.to_string();
    let ann_query_vec = query_vec.clone();
    let ann_handle = tokio::task::spawn_blocking(move || {
        ann_top_n(&ann_data_dir, &ann_project, &ann_query_vec, 100)
    });

    let bm25_ids = bm25_handle.await.context("join bm25 task")??;
    let ann_ids = ann_handle.await.context("join ann task")??;

    // 4. Fuse — or fall back to BM25 if dense returned nothing (EH-6).
    let fused: Vec<u64> = if ann_ids.is_empty() {
        tracing::warn!(
            target: "memlayer_eval::retrieve_hybrid",
            project = %project,
            "dense ANN returned 0 results; falling back to BM25-only ranking (EH-6)"
        );
        bm25_ids
    } else {
        crate::rrf::rrf_fuse(&[bm25_ids, ann_ids], 60)
    };
    let top_k: Vec<u64> = fused.into_iter().take(k.max(0) as usize).collect();

    // 5. SELECT title+content in fused order.
    let hits = fetch_observations(project, &top_k).context("fetch observations by id")?;

    Ok(HybridResult {
        hits,
        latency: t0.elapsed(),
    })
}

/// BM25 top-N over `<data_dir>/projects/<project>.db`. Returns observation
/// rowids (cast to `u64`) in BM25-ranked order. Mirrors the field-weighted
/// SELECT used by `retrieve::retrieve` so TS-1 stays unaffected.
fn bm25_top_n(data_dir: &Path, project: &str, query: &str, n: i32) -> Result<Vec<u64>> {
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);
    paths::ensure_dirs(data_dir).ok();

    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project_state = registry
        .get_or_open(project)
        .with_context(|| format!("open project '{project}' for BM25 retrieval"))?;
    let conn = project_state
        .open_read_conn()
        .context("open read connection for BM25")?;

    let fts_query = crate::retrieve::tokenize(query);
    let limit = n.clamp(1, 1000);

    let mut stmt = conn
        .prepare(
            "SELECT o.id
               FROM observations_fts f
               JOIN observations o ON o.id = f.rowid
              WHERE f.observations_fts MATCH ?1
                AND o.deleted_at IS NULL
              ORDER BY bm25(observations_fts, 5.0, 1.0, 0.5, 0.5, 2.0) ASC
              LIMIT ?2",
        )
        .context("prepare BM25 id-only SELECT")?;
    let rows = stmt
        .query_map(params![fts_query, limit], |row| {
            let id: i64 = row.get(0)?;
            Ok(id as u64)
        })
        .context("run BM25 id-only SELECT")?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r.context("read BM25 id row")?);
    }
    Ok(ids)
}

/// Dense ANN top-N against the sidecar `<data_dir>/vec/<project>.vec.db`.
/// Returns observation rowids in ascending L2 distance (closest first).
///
/// `vec0`'s `MATCH` operator on a `float[N]` column accepts the query as
/// a little-endian byte BLOB of length `N*4`.
fn ann_top_n(
    data_dir: &Path,
    project: &str,
    query_vec: &[f32],
    n: i32,
) -> Result<Vec<u64>> {
    let path = data_dir.join("vec").join(format!("{project}.vec.db"));
    if !path.exists() {
        // Fresh project, sidecar never got built — let EH-6 handle it.
        return Ok(Vec::new());
    }
    let conn = open_with_vec(&path).context("open sidecar vec DB for ANN")?;
    bootstrap_vec_index(&conn, "observations", 384)
        .context("bootstrap observations_vec for ANN")?;

    // sqlite-vec's vec0 MATCH expects an N*4 little-endian byte blob.
    let bytes: Vec<u8> = query_vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    let limit = n.clamp(1, 1000);

    let mut stmt = conn
        .prepare(
            "SELECT id.observation_id
               FROM observations_vec v
               JOIN observations_id_map id ON id.vec_rowid = v.rowid
              WHERE v.embedding MATCH ?1
                AND k = ?2
              ORDER BY distance ASC",
        )
        .context("prepare ANN SELECT")?;
    let rows = stmt
        .query_map(params![bytes, limit], |row| {
            let id: i64 = row.get(0)?;
            Ok(id as u64)
        })
        .context("run ANN SELECT")?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r.context("read ANN id row")?);
    }
    Ok(ids)
}

/// SELECT `title || '\n' || content` for `ids`, preserving the input order.
/// SQLite's `IN (...)` returns rows in arbitrary order, so we hash + rebuild.
fn fetch_observations(project: &str, ids: &[u64]) -> Result<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project_state = registry
        .get_or_open(project)
        .with_context(|| format!("open project '{project}' for fetch_observations"))?;
    let conn = project_state
        .open_read_conn()
        .context("open read connection for fetch_observations")?;

    // Build "?,?,?,..." placeholder list. SQLite's bound-parameter limit is
    // 32766 by default; our k is tiny so we never approach it.
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT id, title, content FROM observations \
         WHERE id IN ({placeholders}) AND deleted_at IS NULL"
    );

    let mut stmt = conn
        .prepare(&sql)
        .context("prepare fetch_observations SELECT")?;
    let id_params: Vec<rusqlite::types::Value> = ids
        .iter()
        .map(|i| rusqlite::types::Value::Integer(*i as i64))
        .collect();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(id_params.iter()), |row| {
            let id: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let content: String = row.get(2)?;
            Ok((id as u64, format!("{title}\n{content}")))
        })
        .context("run fetch_observations SELECT")?;

    let mut by_id: HashMap<u64, String> = HashMap::with_capacity(ids.len());
    for r in rows {
        let (id, formatted) = r.context("read fetch_observations row")?;
        by_id.insert(id, formatted);
    }

    // Rebuild in fused order. Skip ids that vanished (e.g. a row soft-deleted
    // between BM25/ANN and the SELECT-back); RRF over a stale snapshot
    // shouldn't crash the retrieval call.
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// Lazily build the sidecar `observations_vec` index for `project`.
///
/// Cheap-check: if `count(observations_vec) == count(observations WHERE
/// deleted_at IS NULL)` we treat the sidecar as up to date and return
/// immediately. Otherwise we rebuild the rows that are missing — embedding
/// `"<title> <content>"` per observation, routing all texts through the
/// shared `EmbeddingCache` so a previous run's vectors are reused for free.
///
/// The rebuild path is intentionally simple: it wipes `observations_vec` +
/// `observations_id_map` and re-inserts every row. Eval datasets are tiny
/// (LoCoMo is ~3k obs/run total), and BEAM-scale users go through the
/// offline `prepare-vec` tool instead.
fn ensure_vec_index(
    data_dir: &Path,
    project: &str,
    embedder: &dyn Embedder,
    cache: &EmbeddingCache,
) -> Result<()> {
    let vec_path = data_dir.join("vec").join(format!("{project}.vec.db"));
    let vec_conn = open_with_vec(&vec_path).context("open sidecar for ensure_vec_index")?;
    bootstrap_vec_index(&vec_conn, "observations", 384)
        .context("bootstrap observations_vec for ensure_vec_index")?;

    // Read live observations from the project DB.
    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project_state = registry
        .get_or_open(project)
        .with_context(|| format!("open project '{project}' for ensure_vec_index"))?;
    let proj_conn = project_state
        .open_read_conn()
        .context("open read connection for ensure_vec_index")?;

    let live_count: i64 = proj_conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .context("count live observations")?;

    let vec_count: i64 = vec_conn
        .query_row("SELECT COUNT(*) FROM observations_vec", [], |row| row.get(0))
        .context("count observations_vec")?;

    if vec_count == live_count && live_count > 0 {
        return Ok(());
    }

    // Rebuild. Wipe-and-replace keeps the rebuild simple — eval-scale only.
    vec_conn
        .execute("DELETE FROM observations_vec", [])
        .context("clear observations_vec for rebuild")?;
    vec_conn
        .execute("DELETE FROM observations_id_map", [])
        .context("clear observations_id_map for rebuild")?;

    // Snapshot all live rows once — embedder + cache calls don't need a live
    // sqlite handle and the iteration order must be stable for rowid mapping.
    let mut stmt = proj_conn
        .prepare(
            "SELECT id, title, content FROM observations \
              WHERE deleted_at IS NULL \
              ORDER BY id ASC",
        )
        .context("prepare select live observations")?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let content: String = row.get(2)?;
            Ok((id, title, content))
        })?
        .collect::<rusqlite::Result<_>>()
        .context("collect live observations")?;

    if rows.is_empty() {
        return Ok(());
    }

    // Build the embedding source strings and cache-lookup keys.
    let texts: Vec<String> = rows
        .iter()
        .map(|(_, t, c)| format!("{t} {c}"))
        .collect();
    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let (mut hit_vecs, miss_indices) =
        cache.get_many(&text_refs).context("cache get_many for obs")?;

    // Embed misses (single batch) and stitch back into the hits vector.
    if !miss_indices.is_empty() {
        let miss_texts: Vec<&str> = miss_indices.iter().map(|&i| text_refs[i]).collect();
        let miss_vecs = embedder
            .embed(&miss_texts)
            .context("embed observation batch (cache miss)")?;
        let mut put_buf: Vec<(String, Vec<f32>)> = Vec::with_capacity(miss_indices.len());
        for (k, &i) in miss_indices.iter().enumerate() {
            let v = miss_vecs[k].clone();
            put_buf.push((texts[i].clone(), v.clone()));
            hit_vecs[i] = Some(v);
        }
        cache
            .put_many(&put_buf)
            .context("populate cache after obs embed")?;
    }

    // Insert (rowid, embedding) and (vec_rowid, observation_id) in one tx.
    let tx = vec_conn
        .unchecked_transaction()
        .context("begin sidecar rebuild transaction")?;
    {
        let mut ins_vec = tx
            .prepare(
                "INSERT INTO observations_vec(rowid, embedding) VALUES (?1, ?2)",
            )
            .context("prepare insert observations_vec")?;
        let mut ins_map = tx
            .prepare(
                "INSERT INTO observations_id_map(vec_rowid, observation_id) \
                 VALUES (?1, ?2)",
            )
            .context("prepare insert observations_id_map")?;

        for (rowid_minus_one, ((obs_id, _, _), vec_opt)) in
            rows.iter().zip(hit_vecs.iter()).enumerate()
        {
            let rowid = (rowid_minus_one + 1) as i64;
            let v = vec_opt
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing embedding for obs id {}", obs_id))?;
            let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
            ins_vec
                .execute(params![rowid, bytes])
                .with_context(|| format!("insert observations_vec rowid={rowid}"))?;
            ins_map
                .execute(params![rowid, obs_id])
                .with_context(|| {
                    format!("insert observations_id_map rowid={rowid} obs_id={obs_id}")
                })?;
        }
    }
    tx.commit().context("commit sidecar rebuild transaction")?;

    Ok(())
}
