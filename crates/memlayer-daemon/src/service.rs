//! `MemlayerService`: implements the tonic-generated `Memlayer` trait.
//!
//! Each handler:
//!  1. Resolves the project via `ProjectRegistry::get_or_open`.
//!  2. Validates the input.
//!  3. Dispatches to the storage layer (write thread for mutations,
//!     direct read connection for queries).
//!  4. Maps the `memlayer_core::Error` to a `tonic::Status`.
//!
//! Out-of-spec methods (anything Spec 1 doesn't claim) return
//! `Status::unimplemented(...)` with a stable message that Spec 2's CLI can
//! detect.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tonic::{Request, Response, Status};
use tracing::{debug, instrument};

use memlayer_core::error::{Error, Result};
use memlayer_proto::{
    memlayer_server::Memlayer,
    Cursor as ProtoCursor,
    *,
};
use memlayer_storage::{
    cursor::Cursor as StorageCursor,
    diskmon::DiskMonitor,
    facts as facts_q,
    models::{Observation, Prompt, Session},
    projects_admin,
    prompts as prompts_q,
    read as read_q,
    sessions as sessions_q,
    stats as stats_q,
    write::{
        ObservationKey, ObservationPatch, PromptKey, SaveObservationInput, WriteRequest,
    },
    GlobalDb, ProjectRegistry, ProjectState,
};

use memlayer_embed::Embedder;

use crate::admin_guard;
use crate::error_map::map;
use crate::tokens::{TokenMeta, TokenStore};

/// Shared daemon state injected into the service.
#[derive(Clone)]
pub struct DaemonState {
    pub registry: Arc<ProjectRegistry>,
    pub disk_monitor: DiskMonitor,
    pub token_store: Option<Arc<TokenStore>>,
    /// Counted once per RPC entry, decremented on exit.
    pub in_flight: Arc<AtomicU64>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Triggered by `Shutdown` RPC (admin only).
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Max content length per SaveObservation (EC-2). Mirrors Config.
    pub max_content_chars: usize,
    /// Dedupe window (PRD §5.8). Mirrors Config.
    pub dedupe_window: Duration,
    /// Per-project export serialization (FR3 / EC-7). The outer
    /// `parking_lot::Mutex` guards the registry; each per-project entry is a
    /// `tokio::sync::Mutex` so handlers can `.lock().await` across `.await`
    /// points (manifest IO, write-thread reply) without holding the outer lock.
    pub export_mutexes:
        Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// Per-project last sync error string (set by SyncExport / SyncImport handlers).
    pub last_sync_errors: Arc<Mutex<HashMap<String, String>>>,
    /// Per-project last export timestamp (RFC3339), set by SyncExport.
    pub last_export_at: Arc<Mutex<HashMap<String, String>>>,
    /// Cross-project mirror DB (`~/.memlayer/global.sqlite`). The daemon
    /// upserts every successful save into this DB so `--all-projects`
    /// search has a single BM25-ranked corpus to query. Mirror failures
    /// are logged via tracing and DO NOT fail the per-project save.
    pub global_db: Option<Arc<Mutex<GlobalDb>>>,
    /// Async embed worker pool. `None` if BGE-small failed to load at
    /// startup; the daemon then runs in BM25-only mode (search/context
    /// callers see embeddings as "missing" and fall back to BM25).
    pub embed_pool: Option<crate::embed_worker::EmbedWorkerPool>,
    /// Shared BGE-small embedder used by both the embed worker pool and
    /// the daemon's hybrid query path. Same `Arc` instance, so the model
    /// weights are loaded once and reused. `None` mirrors `embed_pool` —
    /// daemon falls back to BM25-only.
    pub query_embedder: Option<std::sync::Arc<memlayer_embed::BgeSmallEmbedder>>,
    /// Async extract worker pool. Always Some after daemon startup; the
    /// pool itself is config-gated per task (cfg.extract.enabled), so a
    /// disabled extract config means workers wake up and immediately
    /// return without an LLM call (SC-7).
    pub extract_pool: Option<crate::extract_worker::ExtractWorkerPool>,
    /// Shared Claude client used by both the extract worker pool and
    /// the daemon's rerank path. Same `Arc` instance, so configuration
    /// (proxy strip, environment) is consistent across the two callers.
    pub claude_client: std::sync::Arc<dyn memlayer_extract::claude_cli::ClaudeClient>,
    /// Async resolution worker for `conflicts_with` pairs. `try_queue`
    /// never blocks save.
    pub resolve_pool: Option<crate::resolve_worker::ResolveWorkerPool>,
    /// Async anchor verification worker. `try_queue` never blocks save.
    pub verify_pool: Option<crate::verify_worker::VerifyWorkerPool>,
}

#[derive(Clone)]
pub struct MemlayerService {
    pub state: Arc<DaemonState>,
}

impl MemlayerService {
    pub fn new(state: Arc<DaemonState>) -> Self {
        Self { state }
    }

    /// Wrap every RPC body so we always update `in_flight` and return read-only
    /// errors when the disk monitor is tripped (writes only).
    fn enter_rpc(&self) -> RpcGuard {
        self.state.in_flight.fetch_add(1, Ordering::Relaxed);
        RpcGuard {
            counter: self.state.in_flight.clone(),
        }
    }

    fn check_writeable(&self) -> Result<()> {
        if self.state.disk_monitor.read_only() {
            return Err(Error::ResourceExhausted(
                "disk full — memlayer is in read-only mode".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn open_project(&self, name: &str) -> Result<Arc<ProjectState>> {
        if name.trim().is_empty() {
            return Err(Error::invalid("project_name is required"));
        }
        self.state.registry.get_or_open(name)
    }

    pub(crate) fn resolved_search_mode(&self, wire: Option<&str>, project_name: &str) -> memlayer_retrieval::hybrid::HybridMode {
        let cfg = memlayer_core::config::load_resolved(Some(project_name));
        memlayer_retrieval::hybrid::HybridMode::parse_wire_or_default(wire, &cfg.search.mode)
    }

    /// Wire `rerank` if set; otherwise honor `search.rerank` config (model role).
    pub(crate) fn resolved_rerank(
        &self,
        wire: Option<&str>,
        project_name: &str,
    ) -> Option<String> {
        if let Some(m) = wire.map(str::trim).filter(|s| !s.is_empty()) {
            return Some(m.to_string());
        }
        let cfg = memlayer_core::config::load_resolved(Some(project_name));
        if cfg.search.rerank {
            Some(cfg.rerank.model.as_lowercase().to_string())
        } else {
            None
        }
    }

    /// Run hybrid retrieval: BM25 top-30 + dense top-30, RRF-fused, then
    /// re-hydrated to full Observations and trimmed to `limit`. Falls
    /// back to BM25-only when:
    /// - the daemon has no embedder loaded (BM25-only mode), or
    /// - `BgeSmallEmbedder::embed(query)` errors, or
    /// - the per-project `observations_vec` table is missing/empty.
    ///
    /// Spec: retrieval-promotion SC-3, SC-11, P8.
    pub(crate) fn hybrid_search(
        &self,
        conn: &rusqlite::Connection,
        query: &str,
        type_filter: Option<&str>,
        scope_filter: Option<&str>,
        limit: i32,
        project_name: &str,
    ) -> Result<Vec<Observation>> {
        const RRF_DEPTH: i32 = 30;
        const RRF_K: u32 = 60;

        let cfg = memlayer_core::config::load_resolved(Some(project_name));
        let decay_lambda = cfg.search.decay_lambda;

        let bm25_hits = read_q::search(conn, query, type_filter, scope_filter, RRF_DEPTH)?;
        let fact_ids = fact_parent_ids(conn, query, RRF_DEPTH as i64);

        // Embed the query. If the daemon has no embedder (cold-start
        // fallback or candle init failure), or embedding errors, fall
        // back to BM25 (+ facts) only.
        let dense_hits = match &self.state.query_embedder {
            None => {
                tracing::info!(
                    "hybrid requested but no query embedder loaded — using BM25 (+ facts if any)",
                );
                Vec::new()
            }
            Some(embedder) => match embedder.embed(&[query]) {
                Ok(mut vs) => match vs.pop() {
                    Some(q_vec) => match read_q::search_dense(conn, &q_vec, RRF_DEPTH as i64) {
                        Ok(h) => h,
                        Err(e) => {
                            tracing::warn!(error = %e, "dense search failed — using BM25 (+ facts)");
                            Vec::new()
                        }
                    },
                    None => {
                        tracing::warn!("embedder returned no vectors for query — using BM25 (+ facts)");
                        Vec::new()
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "query embed failed — using BM25 (+ facts)");
                    Vec::new()
                }
            },
        };

        if dense_hits.is_empty() && fact_ids.is_empty() {
            let mut hits: Vec<Observation> =
                bm25_hits.into_iter().take(limit as usize).collect();
            if decay_lambda > 0.0 {
                apply_time_decay(&mut hits, decay_lambda);
            }
            return Ok(hits);
        }

        // Apply the same type/scope filter to dense hits that BM25 honored.
        // search_dense doesn't accept the filter today; cheap to filter here.
        let dense_hits: Vec<Observation> = dense_hits
            .into_iter()
            .filter(|o| {
                if let Some(t) = type_filter {
                    if o.r#type != t {
                        return false;
                    }
                }
                if let Some(s) = scope_filter {
                    if o.scope != s {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Build id-rank lists for RRF (BM25 + dense + fact parents).
        let bm25_ids: Vec<u64> = bm25_hits.iter().map(|o| o.id as u64).collect();
        let dense_ids: Vec<u64> = dense_hits.iter().map(|o| o.id as u64).collect();
        let fused = memlayer_retrieval::facts_fuse::fuse_observation_lists_scored(
            &[bm25_ids, dense_ids, fact_ids],
            RRF_K,
        );

        // Re-hydrate by id, preferring observations we already have in
        // hand. This avoids an extra round-trip to the DB for the common
        // case where the same observations rank in both lists.
        let mut by_id: std::collections::HashMap<i64, Observation> =
            std::collections::HashMap::new();
        for o in bm25_hits.into_iter().chain(dense_hits) {
            by_id.entry(o.id).or_insert(o);
        }

        let mut scored: Vec<(Observation, f64)> = fused
            .into_iter()
            .filter_map(|(id, score)| {
                let oid = id as i64;
                let o = if let Some(o) = by_id.remove(&oid) {
                    o
                } else {
                    read_q::get(conn, &ObservationKey::Id(oid)).ok()?
                };
                Some((o, score))
            })
            .collect();

        if decay_lambda > 0.0 {
            let now = chrono::Utc::now();
            for (o, score) in &mut scored {
                let age = age_days_since(&o.created_at, now);
                *score *= (-decay_lambda * age).exp();
            }
            scored.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        Ok(scored
            .into_iter()
            .map(|(o, _)| o)
            .take(limit as usize)
            .collect())
    }

    /// LLM rerank wrapper around `memlayer_retrieval::rerank::ClaudeReranker`.
    /// Hard 5s timeout (SC-5); on timeout or error, the un-reranked input
    /// list is returned with a tracing warning. `model` is the wire string
    /// Roles inherit the invoking agent's current model; a concrete id is
    /// passed through to the agent CLI.
    async fn rerank_hits(
        &self,
        model: &str,
        query: &str,
        hits: Vec<Observation>,
    ) -> Vec<Observation> {
        if hits.len() <= 1 {
            return hits;
        }
        let model_id = model.trim();
        if model_id.is_empty() {
            return hits;
        }
        let reranker = memlayer_retrieval::rerank::ClaudeReranker::with_model_id(
            self.state.claude_client.clone(),
            model_id,
        );
        let candidates: Vec<String> = hits
            .iter()
            .map(|o| {
                let body = if o.content.len() > 800 {
                    o.content.chars().take(800).collect()
                } else {
                    o.content.clone()
                };
                format!("{}\n{}", o.title, body)
            })
            .collect();

        let top_k = hits.len();
        let fut = async {
            use memlayer_retrieval::rerank::Reranker;
            reranker.rerank(&candidates, query, top_k).await
        };

        let rerank_result = tokio::time::timeout(std::time::Duration::from_secs(5), fut).await;

        let reordered: Vec<String> = match rerank_result {
            Ok(Ok((reranked, _dur))) => reranked,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, model, "rerank failed — returning hybrid order");
                return hits;
            }
            Err(_) => {
                tracing::warn!(model, "rerank timed out (5s) — returning hybrid order");
                return hits;
            }
        };

        // Map reranked candidate strings back to observations by stable
        // string match (the reranker preserves the candidate text, just
        // reorders). Build an index on candidates so duplicates are
        // handled by first-occurrence.
        let mut idx: std::collections::HashMap<String, usize> =
            std::collections::HashMap::with_capacity(candidates.len());
        for (i, c) in candidates.iter().enumerate() {
            idx.entry(c.clone()).or_insert(i);
        }
        let mut out: Vec<Observation> = Vec::with_capacity(reordered.len());
        let mut consumed = vec![false; hits.len()];
        for c in &reordered {
            if let Some(&i) = idx.get(c) {
                if !consumed[i] {
                    consumed[i] = true;
                    out.push(hits[i].clone());
                }
            }
        }
        // Append any candidates the reranker dropped, preserving original
        // hybrid order. (`ClaudeReranker` should return all top_k, but we
        // belt-and-suspenders so callers always get a full result.)
        for (i, o) in hits.into_iter().enumerate() {
            if !consumed[i] {
                out.push(o);
            }
        }
        out
    }
}

struct RpcGuard {
    counter: Arc<AtomicU64>,
}

impl Drop for RpcGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Await a write-thread reply and convert to `Status` in one shot.
///
/// The write thread sends `Result<T, memlayer_core::Error>` over a
/// `oneshot` channel. Two failure modes need flattening:
/// - `oneshot::RecvError` (channel closed → write thread panicked).
/// - The domain `Error` returned by the handler.
#[allow(clippy::result_large_err)]
async fn await_write_reply<T>(
    rx: tokio::sync::oneshot::Receiver<Result<T>>,
) -> std::result::Result<T, Status> {
    let inner = rx
        .await
        .map_err(|_| Status::internal("write thread crashed"))?;
    map(inner)
}

// ---------------------------------------------------------------------------
// Conversions: storage models ↔ proto messages
// ---------------------------------------------------------------------------

fn obs_to_proto(o: Observation) -> memlayer_proto::Observation {
    memlayer_proto::Observation {
        id: o.id,
        sync_id: o.sync_id,
        session_id: o.session_id,
        r#type: o.r#type,
        title: o.title,
        content: o.content,
        tool_name: o.tool_name,
        scope: o.scope,
        created_by: o.created_by,
        topic_key: o.topic_key,
        normalized_hash: o.normalized_hash,
        revision_count: o.revision_count,
        duplicate_count: o.duplicate_count,
        last_seen_at: o.last_seen_at,
        created_at: o.created_at,
        updated_at: o.updated_at,
        deleted_at: o.deleted_at,
        review_after: o.review_after,
        project_name: None,
        code_anchor: o.code_anchor,
        supersedes_ids: o.superseded_ids,
        superseded_count: o.superseded_count,
        verify_state: Some(o.verify_state).filter(|s| !s.is_empty()),
    }
}

fn filter_context_observations(
    observations: &mut Vec<memlayer_proto::Observation>,
    project_name: &str,
    include_stale: bool,
) {
    let cfg = memlayer_core::config::load_resolved(Some(project_name));
    observations.retain(|o| {
        let state = o.verify_state.as_deref().unwrap_or("unanchored");
        crate::context_filter::context_allows(state, cfg.verify.serve_stale, include_stale)
    });
}

/// Age in days since `created_at` (RFC3339 or SQLite `datetime('now')` form).
fn age_days_since(created_at: &str, now: chrono::DateTime<chrono::Utc>) -> f64 {
    let parsed = chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S")
                .map(|ndt| ndt.and_utc())
        })
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(created_at, "%Y-%m-%dT%H:%M:%S")
                .map(|ndt| ndt.and_utc())
        });
    match parsed {
        Ok(dt) => {
            let secs = (now - dt).num_seconds().max(0) as f64;
            secs / 86_400.0
        }
        Err(_) => 0.0,
    }
}

fn apply_time_decay(hits: &mut Vec<Observation>, decay_lambda: f64) {
    if decay_lambda <= 0.0 || hits.len() < 2 {
        return;
    }
    let now = chrono::Utc::now();
    let mut scored: Vec<(Observation, f64)> = hits
        .drain(..)
        .enumerate()
        .map(|(i, o)| {
            let base = 1.0 / (1.0 + i as f64);
            let age = age_days_since(&o.created_at, now);
            (o, base * (-decay_lambda * age).exp())
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    *hits = scored.into_iter().map(|(o, _)| o).collect();
}

/// Expand context hits with same-session neighbors; never exceed `limit`.
fn expand_evidence_window(
    conn: &rusqlite::Connection,
    hits: Vec<Observation>,
    window: u32,
    limit: i32,
) -> Vec<Observation> {
    if window == 0 {
        return hits.into_iter().take(limit as usize).collect();
    }
    let limit = limit.max(1) as usize;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for seed in hits {
        if out.len() >= limit {
            break;
        }
        let neighbors = read_q::neighbors_in_session(conn, seed.id, window).unwrap_or_default();
        // Prefer seed first, then neighbors by id order.
        let mut batch = vec![seed];
        for n in neighbors {
            if n.id != batch[0].id {
                batch.push(n);
            }
        }
        for o in batch {
            if seen.insert(o.id) {
                out.push(o);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    out
}

/// Apply per-type quota, stale filter, then token budget to context hits.
fn finalize_context_hits(
    conn: &rusqlite::Connection,
    hits: Vec<Observation>,
    project_name: &str,
    include_stale: bool,
    max_per_type: u32,
    max_tokens: u32,
) -> (Vec<memlayer_proto::Observation>, i32) {
    let hits = crate::token_budget::apply_max_per_type(hits, max_per_type);
    let mut recent = observations_to_proto(conn, hits);
    filter_context_observations(&mut recent, project_name, include_stale);
    let (recent, tokens_used) =
        crate::token_budget::pack_proto_by_token_budget(recent, max_tokens);
    (recent, tokens_used as i32)
}

/// Parse and stamp anchors for a just-saved observation. Returns an optional
/// warning when the directory is not a git repo (save still succeeds).
#[allow(clippy::result_large_err)]
async fn stamp_observation_anchors(
    project: &memlayer_storage::ProjectState,
    repo: Option<&std::path::Path>,
    observation_id: i64,
    raw: &[String],
) -> std::result::Result<Option<String>, Status> {
    use memlayer_core::git;
    use memlayer_storage::anchor::{digest_slice, Anchor, VerifyState};
    use memlayer_storage::write::WriteRequest;

    let mut parsed: Vec<Anchor> = Vec::new();
    for s in raw {
        match Anchor::parse(s.trim()) {
            Ok(a) => parsed.push(a),
            Err(e) => tracing::warn!(anchor = %s, error = %e, "skipping invalid --anchor"),
        }
    }
    if parsed.is_empty() {
        return Ok(None);
    }

    let Some(repo) = repo.filter(|p| git::is_repo(p)) else {
        let anchors = parsed.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::Custom {
            f: Box::new(move |conn| {
                memlayer_storage::anchor::insert_anchors(conn, observation_id, &anchors)?;
                memlayer_storage::anchor::set_verify_state(
                    conn,
                    observation_id,
                    VerifyState::Unanchored,
                    None,
                )?;
                Ok(())
            }),
            reply: tx,
        }))?;
        await_write_reply(rx).await?;
        return Ok(Some(
            "not a git repo; anchors saved as unanchored".into(),
        ));
    };

    let head = match git::head_sha(repo) {
        Ok(h) => h,
        Err(_) => {
            return Ok(Some(
                "could not read HEAD; anchors saved as unanchored".into(),
            ));
        }
    };

    for a in &mut parsed {
        a.anchor_commit = Some(head.clone());
        let path = a.path.replace('\\', "/");
        if let Ok(Some(text)) = git::file_at_commit(repo, &head, &path) {
            a.content_digest = Some(digest_slice(&text, a));
        }
    }

    let anchors = parsed;
    let head_for_write = head.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    map(project.write.send(WriteRequest::Custom {
        f: Box::new(move |conn| {
            memlayer_storage::anchor::insert_anchors(conn, observation_id, &anchors)?;
            memlayer_storage::anchor::set_verify_state(
                conn,
                observation_id,
                VerifyState::Verified,
                Some(&head_for_write),
            )?;
            if let Some(first) = anchors.first() {
                conn.execute(
                    "UPDATE observations SET code_anchor = ?2 WHERE id = ?1",
                    rusqlite::params![observation_id, first.to_canonical_string()],
                )
                .map_err(|e| memlayer_core::Error::internal(format!("code_anchor: {e}")))?;
            }
            Ok(())
        }),
        reply: tx,
    }))?;
    await_write_reply(rx).await?;
    Ok(None)
}

/// Batch-fill `supersedes_ids` for search/context/recent results (one query).
fn attach_supersedes(
    conn: &rusqlite::Connection,
    observations: &mut [memlayer_proto::Observation],
) {
    let ids: Vec<i64> = observations.iter().map(|o| o.id).collect();
    let Ok(map) = read_q::supersedes_ids_for(conn, &ids) else {
        return;
    };
    for o in observations.iter_mut() {
        if let Some(ids) = map.get(&o.id) {
            o.supersedes_ids = ids.clone();
        }
    }
}

fn observations_to_proto(
    conn: &rusqlite::Connection,
    hits: Vec<Observation>,
) -> Vec<memlayer_proto::Observation> {
    let mut out: Vec<_> = hits.into_iter().map(obs_to_proto).collect();
    attach_supersedes(conn, &mut out);
    out
}

fn fact_to_proto(f: facts_q::Fact) -> memlayer_proto::Fact {
    memlayer_proto::Fact {
        id: f.id,
        obs_id: f.obs_id,
        subject: f.subject,
        predicate: f.predicate,
        object: f.object,
        temporal: f.temporal,
        salience: f.salience,
        superseded_by: f.superseded_by,
        extracted_by: f.extracted_by,
        extracted_at: f.extracted_at,
    }
}

fn session_to_proto(s: Session) -> memlayer_proto::Session {
    memlayer_proto::Session {
        id: s.id,
        directory: s.directory,
        started_at: s.started_at,
        ended_at: s.ended_at,
        summary: s.summary,
    }
}

fn prompt_to_proto(p: Prompt) -> memlayer_proto::Prompt {
    memlayer_proto::Prompt {
        id: p.id,
        sync_id: p.sync_id,
        session_id: p.session_id,
        content: p.content,
        created_at: p.created_at,
    }
}

fn fact_parent_ids(conn: &rusqlite::Connection, query: &str, limit: i64) -> Vec<u64> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
        .unwrap_or(0);
    if n == 0 {
        return Vec::new();
    }
    match facts_q::search_facts(conn, query, limit) {
        Ok(facts) => {
            let mut ids = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for f in facts {
                if seen.insert(f.obs_id) {
                    ids.push(f.obs_id as u64);
                }
            }
            ids
        }
        Err(e) => {
            tracing::debug!(error = %e, "facts search skipped");
            Vec::new()
        }
    }
}

fn proto_cursor(c: StorageCursor) -> Result<ProtoCursor> {
    Ok(ProtoCursor { token: c.encode()? })
}

fn parse_cursor(c: &Option<ProtoCursor>) -> Result<Option<StorageCursor>> {
    Ok(match c {
        Some(c) => Some(StorageCursor::decode(&c.token)?),
        None => None,
    })
}

// ---------------------------------------------------------------------------
// Memlayer trait implementation
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl Memlayer for MemlayerService {
    // ---- Observation lifecycle ----

    #[instrument(skip(self, req), fields(rpc="SaveObservation"))]
    async fn save_observation(
        &self,
        req: Request<SaveObservationRequest>,
    ) -> Result<Response<SaveObservationResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let scope = if r.scope.is_empty() {
            "project".into()
        } else {
            r.scope.clone()
        };
        let mut topic_key = r.topic_key.clone().filter(|s| !s.trim().is_empty());
        if topic_key.is_none() {
            if let Ok(conn) = project.open_read_conn() {
                if let Ok(k) =
                    crate::suggest_topic_key::suggest(&conn, &r.r#type, &r.title, &scope)
                {
                    topic_key = Some(k);
                }
            }
        }
        let input = SaveObservationInput {
            sync_id: r.sync_id,
            session_id: r.session_id,
            r#type: r.r#type,
            title: r.title,
            content: r.content,
            tool_name: r.tool_name,
            scope,
            created_by: r.created_by,
            topic_key,
            code_anchor: r.code_anchor.clone().or_else(|| r.anchors.first().cloned()),
            dedupe_window_secs: self.state.dedupe_window.as_secs(),
            max_content_chars: self.state.max_content_chars,
                    skip_supersede: false,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::SaveObservation { input, reply: tx }))?;
        let obs = rx.await.map_err(|_| Status::internal("write thread crashed"))?;
        let mut obs = map(obs)?;
        let mut warnings_pending: Vec<String> = Vec::new();

        // Stamp multi-anchors + digests. Never fail the save if git / stamping fails.
        let mut anchor_strings: Vec<String> = r.anchors.clone();
        if anchor_strings.is_empty() {
            if let Some(c) = r.code_anchor.clone().filter(|s| !s.trim().is_empty()) {
                anchor_strings.push(c);
            }
        }
        if !anchor_strings.is_empty() {
            let repo = self
                .state
                .registry
                .get_repo_path(&r.project_name)
                .ok()
                .flatten()
                .or_else(|| {
                    let cwd = std::env::current_dir().ok()?;
                    if memlayer_core::git::is_repo(&cwd) {
                        Some(cwd)
                    } else {
                        None
                    }
                });
            match stamp_observation_anchors(
                &project,
                repo.as_deref(),
                obs.id,
                &anchor_strings,
            )
            .await
            {
                Ok(Some(w)) => warnings_pending.push(w),
                Ok(None) => {
                    if let Ok(conn) = project.open_read_conn() {
                        if let Ok(fresh) = read_q::get(&conn, &ObservationKey::Id(obs.id)) {
                            obs = fresh;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, obs_id = obs.id, "anchor stamp failed");
                    warnings_pending.push(format!("anchor_stamp_failed: {}", e.message()));
                }
            }
        }

        // Mirror into the global DB so --all-projects search has a single
        // BM25-ranked view across every memlayer-tracked repo. Failure here
        // MUST NOT fail the per-project save (the source of truth has
        // already committed). Errors are logged via tracing only.
        if let Some(global) = &self.state.global_db {
            let mut guard = global.lock();
            if let Err(e) = guard.upsert_observation(&r.project_name, &obs) {
                tracing::warn!(
                    error = %e,
                    project = %r.project_name,
                    obs_id = obs.id,
                    "global mirror upsert failed (per-project save still committed)",
                );
            }
        }

        // Queue async embed (and extract, if enabled) work after the
        // synchronous save commits — never on the critical path (SC-1).
        // Both pools `try_send`; full queue / disconnected pool drops the
        // task silently and logs.
        let mut warnings: Vec<String> = warnings_pending;
        if let Some(pool) = &self.state.embed_pool {
            match pool.try_queue(crate::embed_worker::EmbedTask {
                project_name: r.project_name.clone(),
                obs_id: obs.id,
                title: obs.title.clone(),
                content: obs.content.clone(),
            }) {
                crate::embed_worker::QueueResult::Dropped => warnings.push("embed_dropped".into()),
                crate::embed_worker::QueueResult::Disconnected => {
                    warnings.push("embed_dropped".into())
                }
                crate::embed_worker::QueueResult::Queued => {}
            }
        }
        // Extract is opt-in: only queue when this project's resolved
        // config has extract.enabled = true. SC-7 guarantees zero LLM
        // calls otherwise.
        if let Some(pool) = &self.state.extract_pool {
            if crate::extract_worker::resolved_model_for(&r.project_name).is_some() {
                match pool.try_queue(crate::extract_worker::ExtractTask {
                    project_name: r.project_name.clone(),
                    obs_id: obs.id,
                    title: obs.title.clone(),
                    content: obs.content.clone(),
                    session_id: Some(obs.session_id.clone()),
                }) {
                    crate::extract_worker::QueueResult::Dropped => {
                        warnings.push("extract_dropped".into())
                    }
                    crate::extract_worker::QueueResult::Disconnected => {
                        warnings.push("extract_dropped".into())
                    }
                    crate::extract_worker::QueueResult::Queued => {}
                }
            }
        }

        if let Some(pool) = &self.state.resolve_pool {
            if let Ok(conn) = project.open_read_conn() {
                if let Ok(rels) =
                    memlayer_storage::get_relations_for_observation(&conn, obs.id)
                {
                    for rel in rels.into_iter().filter(|r| r.relation_type == "conflicts_with")
                    {
                        let old_id = if rel.source_id == obs.id {
                            rel.target_id
                        } else {
                            rel.source_id
                        };
                        let _ = pool.try_queue(crate::resolve_worker::ResolveJob {
                            project: r.project_name.clone(),
                            old_id,
                            new_id: obs.id,
                        });
                    }
                }
            }
        }

        // Pass superseded observations (if any) in similar_observations so the
        // CLI can surface "Superseded observation #N: <title>" to the user.
        let superseded: Vec<memlayer_proto::Observation> = obs
            .superseded_ids
            .iter()
            .map(|&sid| memlayer_proto::Observation {
                id: sid,
                ..Default::default()
            })
            .collect();
        Ok(Response::new(SaveObservationResponse {
            observation: Some(obs_to_proto(obs)),
            similar_observations: superseded,
            warnings,
        }))
    }

    #[instrument(skip(self, req), fields(rpc="GetObservation"))]
    async fn get_observation(
        &self,
        req: Request<GetObservationRequest>,
    ) -> Result<Response<GetObservationResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let key = match r.key {
            Some(get_observation_request::Key::Id(id)) => ObservationKey::Id(id),
            Some(get_observation_request::Key::SyncId(s)) => ObservationKey::SyncId(s),
            None => return Err(Status::invalid_argument("missing observation key")),
        };
        let conn = map(project.open_read_conn())?;
        let obs = map(read_q::get(&conn, &key))?;
        Ok(Response::new(GetObservationResponse {
            observation: Some(obs_to_proto(obs)),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="UpdateObservation"))]
    async fn update_observation(
        &self,
        req: Request<UpdateObservationRequest>,
    ) -> Result<Response<UpdateObservationResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let key = match r.key {
            Some(update_observation_request::Key::Id(id)) => ObservationKey::Id(id),
            Some(update_observation_request::Key::SyncId(s)) => ObservationKey::SyncId(s),
            None => return Err(Status::invalid_argument("missing observation key")),
        };
        let patch = ObservationPatch {
            title: r.title,
            content: r.content,
            topic_key: r.topic_key,
            scope: r.scope,
            r#type: r.r#type,
            code_anchor: r.code_anchor,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::UpdateObservation { key, patch, reply: tx }))?;
        let obs = await_write_reply(rx).await?;
        Ok(Response::new(UpdateObservationResponse {
            observation: Some(obs_to_proto(obs)),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="DeleteObservation"))]
    async fn delete_observation(
        &self,
        req: Request<DeleteObservationRequest>,
    ) -> Result<Response<DeleteObservationResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let key = match r.key {
            Some(delete_observation_request::Key::Id(id)) => ObservationKey::Id(id),
            Some(delete_observation_request::Key::SyncId(s)) => ObservationKey::SyncId(s),
            None => return Err(Status::invalid_argument("missing observation key")),
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let req = if r.hard {
            WriteRequest::HardDeleteObservation { key, reply: tx }
        } else {
            WriteRequest::SoftDeleteObservation { key, reply: tx }
        };
        map(project.write.send(req))?;
        await_write_reply(rx).await?;
        Ok(Response::new(DeleteObservationResponse {}))
    }

    #[instrument(skip(self, req), fields(rpc="SearchObservations"))]
    async fn search_observations(
        &self,
        req: Request<SearchObservationsRequest>,
    ) -> Result<Response<SearchObservationsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let limit = if r.limit == 0 { read_q::DEFAULT_LIMIT } else { r.limit };
        let conn = map(project.open_read_conn())?;
        if r.all_projects {
            let mode = self.resolved_search_mode(r.mode.as_deref(), &r.project_name);

            if mode == memlayer_retrieval::hybrid::HybridMode::Hybrid {
                // Hybrid cross-project: BM25 via global DB + dense fan-out to
                // per-project DBs, then RRF-fused on (project, source_id) key.
                if let Some(embedder) = &self.state.query_embedder {
                    let q_vec = match embedder.embed(&[r.query.as_str()]) {
                        Ok(mut vs) => vs.pop(),
                        Err(e) => {
                            tracing::warn!(error=%e, "hybrid all-projects: query embed failed; falling back to BM25");
                            None
                        }
                    };
                    if let Some(q_vec) = q_vec {
                        const RRF_DEPTH: i64 = 30;
                        const RRF_K: u32 = 60;
                        // BM25 ids from the global mirror.
                        let bm25_keys: Vec<(String, i64)> = if let Some(global) = &self.state.global_db {
                            let guard = global.lock();
                            match guard.search(&r.query, RRF_DEPTH) {
                                Ok(hits) => hits.into_iter().map(|h| (h.project.clone(), h.source_id)).collect(),
                                Err(_) => vec![],
                            }
                        } else { vec![] };

                        // Dense ids from per-project fan-out.
                        let projects = map(ProjectRegistry::list_known_on_disk())?;
                        let project_paths: Vec<(String, std::path::PathBuf)> = projects
                            .into_iter()
                            .map(|(name, _)| {
                                let path = memlayer_core::paths::project_db_path(&name);
                                (name, path)
                            })
                            .collect();
                        let dense_hits = map(read_q::search_dense_multi(&project_paths, &q_vec, RRF_DEPTH))?;
                        let dense_keys: Vec<(String, i64)> = dense_hits.iter()
                            .map(|(proj, obs)| (proj.clone(), obs.id))
                            .collect();

                        let fused_keys = memlayer_retrieval::rrf::rrf_fuse_keyed(
                            &[bm25_keys, dense_keys],
                            RRF_K,
                        );

                        // Build a lookup from (project, id) to Observation.
                        let mut obs_map: std::collections::HashMap<(String, i64), memlayer_proto::Observation> =
                            std::collections::HashMap::new();
                        if let Some(global) = &self.state.global_db {
                            let guard = global.lock();
                            if let Ok(bm25_obs) = guard.search(&r.query, RRF_DEPTH) {
                                for h in bm25_obs {
                                    let key = (h.project.clone(), h.source_id);
                                    obs_map.entry(key).or_insert_with(|| memlayer_proto::Observation {
                                        id: h.source_id, sync_id: String::new(), session_id: String::new(),
                                        r#type: h.r#type, title: h.title, content: h.content,
                                        tool_name: None, scope: "project".into(), created_by: None,
                                        topic_key: h.topic_key, normalized_hash: None,
                                        revision_count: 1, duplicate_count: 1, last_seen_at: None,
                                        created_at: h.created_at.clone(), updated_at: h.created_at,
                                        deleted_at: None, review_after: None, project_name: Some(h.project),
                                        code_anchor: None,
                                        supersedes_ids: vec![],
                                        superseded_count: 0,
                                    verify_state: None,
                                    });
                                }
                            }
                        }
                        for (proj, obs) in dense_hits {
                            let key = (proj.clone(), obs.id);
                            obs_map.entry(key).or_insert_with(|| {
                                let mut p = obs_to_proto(obs);
                                p.project_name = Some(proj);
                                p
                            });
                        }

                        let observations: Vec<memlayer_proto::Observation> = fused_keys
                            .into_iter()
                            .take(limit as usize)
                            .filter_map(|k| obs_map.remove(&k))
                            .collect();

                        return Ok(Response::new(SearchObservationsResponse { observations, warning: None, tokens_used: None }));
                    }
                }
                // Fall through to BM25-only if no embedder.
            }

            // Prefer the cross-project global mirror DB when available — one
            // BM25-ranked corpus instead of per-project fan-out.
            if let Some(global) = &self.state.global_db {
                let guard = global.lock();
                let hits = map(guard.search(&r.query, limit as i64))?;
                let observations: Vec<memlayer_proto::Observation> = hits
                    .into_iter()
                    .map(|h| memlayer_proto::Observation {
                        id: h.source_id,
                        sync_id: String::new(),
                        session_id: String::new(),
                        r#type: h.r#type,
                        title: h.title,
                        content: h.content,
                        tool_name: None,
                        scope: "project".into(),
                        created_by: None,
                        topic_key: h.topic_key,
                        normalized_hash: None,
                        revision_count: 1,
                        duplicate_count: 1,
                        last_seen_at: None,
                        created_at: h.created_at.clone(),
                        updated_at: h.created_at,
                        deleted_at: None,
                        review_after: None,
                        project_name: Some(h.project),
                        code_anchor: None,
                        supersedes_ids: vec![],
                        superseded_count: 0,
                    verify_state: None,
                    })
                    .collect();
                return Ok(Response::new(SearchObservationsResponse {
                    observations,
                    warning: None,
                    tokens_used: None,
                }));
            }

            // Fallback: per-project fan-out (used only when global DB
            // failed to open at startup).
            let projects = map(ProjectRegistry::list_known_on_disk())?;
            let other_paths: Vec<(String, std::path::PathBuf)> = projects
                .into_iter()
                .filter(|(name, _)| name != &project.normalized)
                .map(|(name, _)| (name.clone(), memlayer_core::paths::project_db_path(&name)))
                .collect();
            let (hits, warning) = map(read_q::search_all_projects(
                &conn,
                &other_paths,
                &r.query,
                limit,
            ))?;
            return Ok(Response::new(SearchObservationsResponse {
                observations: hits.into_iter().map(obs_to_proto).collect(),
                warning,
                tokens_used: None,
            }));
        }
        let hits = if self.resolved_search_mode(r.mode.as_deref(), &r.project_name)
            == memlayer_retrieval::hybrid::HybridMode::Hybrid
        {
            map(self.hybrid_search(
                &conn,
                &r.query,
                r.r#type.as_deref(),
                r.scope.as_deref(),
                limit,
                &r.project_name,
            ))?
        } else {
            map(read_q::search(
                &conn,
                &r.query,
                r.r#type.as_deref(),
                r.scope.as_deref(),
                limit,
            ))?
        };
        // Optional LLM rerank (SC-5). Wire field wins; else search.rerank config.
        let hits = match self.resolved_rerank(r.rerank.as_deref(), &r.project_name) {
            Some(model) => self.rerank_hits(&model, &r.query, hits).await,
            None => hits,
        };
        let cfg = memlayer_core::config::load_resolved(Some(&r.project_name));
        let hits = crate::token_budget::apply_max_per_type(hits, cfg.search.max_per_type);
        let max_tokens = r.max_tokens.unwrap_or(0).max(0) as u32;
        let (hits, tokens_used) = crate::token_budget::pack_by_token_budget(hits, max_tokens);
        Ok(Response::new(SearchObservationsResponse {
            observations: observations_to_proto(&conn, hits),
            warning: None,
            tokens_used: Some(tokens_used as i32),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="ListObservations"))]
    async fn list_observations(
        &self,
        req: Request<ListObservationsRequest>,
    ) -> Result<Response<ListObservationsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let cur = map(parse_cursor(&r.cursor))?;
        let limit = if r.limit == 0 { read_q::DEFAULT_LIMIT } else { r.limit };
        let conn = map(project.open_read_conn())?;
        let (rows, next) = map(read_q::list(
            &conn,
            r.r#type.as_deref(),
            r.scope.as_deref(),
            r.created_by.as_deref(),
            r.due_for_review,
            limit,
            cur.as_ref(),
            r.session_id.as_deref(),
        ))?;
        let next_cursor = match next {
            Some(c) => Some(map(proto_cursor(c))?),
            None => None,
        };
        Ok(Response::new(ListObservationsResponse {
            observations: rows.into_iter().map(obs_to_proto).collect(),
            next_cursor,
        }))
    }

    #[instrument(skip(self, req), fields(rpc="RecentObservations"))]
    async fn recent_observations(
        &self,
        req: Request<RecentObservationsRequest>,
    ) -> Result<Response<RecentObservationsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let limit = if r.limit == 0 { read_q::DEFAULT_LIMIT } else { r.limit };
        let conn = map(project.open_read_conn())?;
        let rows = map(read_q::recent(&conn, limit, r.scope.as_deref()))?;
        Ok(Response::new(RecentObservationsResponse {
            observations: observations_to_proto(&conn, rows),
        }))
    }

    // ---- Context / Timeline / SuggestTopicKey / CapturePassive ----
    // FR12.1–FR12.6, wired in spec2-t3.

    #[instrument(skip(self, req), fields(rpc = "Context"))]
    async fn context(
        &self,
        req: Request<ContextRequest>,
    ) -> Result<Response<ContextResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let limit = if r.recent_limit <= 0 { 10 } else { r.recent_limit };
        let conn = map(project.open_read_conn())?;

        let cfg = memlayer_core::config::load_resolved(Some(&r.project_name));
        let evidence_window = cfg.search.evidence_window;
        let max_per_type = cfg.search.max_per_type;
        let max_tokens = r.max_tokens.unwrap_or(0).max(0) as u32;

        if let Some(anchor) = r.anchor.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            let hits = map(read_q::search_by_anchor(&conn, anchor, limit))?;
            let hits = expand_evidence_window(&conn, hits, evidence_window, limit);
            let (recent, tokens_used) = finalize_context_hits(
                &conn,
                hits,
                &r.project_name,
                r.include_stale,
                max_per_type,
                max_tokens,
            );
            let snapshot = ContextSnapshot {
                recent_observations: recent,
                active_topics: vec![],
            };
            return Ok(Response::new(ContextResponse {
                snapshot: Some(snapshot),
                tokens_used: Some(tokens_used),
            }));
        }

        // When the caller provides a query, retrieve for that query using the
        // resolved search mode (config default hybrid). Empty query keeps the
        // recent + active-topics briefing.
        let recents = match r.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => {
                let hits = if self.resolved_search_mode(r.mode.as_deref(), &r.project_name)
                    == memlayer_retrieval::hybrid::HybridMode::Hybrid
                {
                    map(self.hybrid_search(&conn, q, None, None, limit, &r.project_name))?
                } else {
                    map(read_q::search(&conn, q, None, None, limit))?
                };
                let hits = match self.resolved_rerank(r.rerank.as_deref(), &r.project_name) {
                    Some(model) => self.rerank_hits(&model, q, hits).await,
                    None => hits,
                };
                expand_evidence_window(&conn, hits, evidence_window, limit)
            }
            _ => {
                let (recents, topics) = map(read_q::recent_active(&conn, limit))?;
                let recents = expand_evidence_window(&conn, recents, evidence_window, limit);
                let (recent, tokens_used) = finalize_context_hits(
                    &conn,
                    recents,
                    &r.project_name,
                    r.include_stale,
                    max_per_type,
                    max_tokens,
                );
                let snapshot = ContextSnapshot {
                    recent_observations: recent,
                    active_topics: topics
                        .into_iter()
                        .map(|t| TopicSummary {
                            topic_key: t.topic_key,
                            scope: t.scope,
                            latest_title: t.latest_title,
                            updated_at: t.updated_at,
                        })
                        .collect(),
                };
                return Ok(Response::new(ContextResponse {
                    snapshot: Some(snapshot),
                    tokens_used: Some(tokens_used),
                }));
            }
        };
        let (recent, tokens_used) = finalize_context_hits(
            &conn,
            recents,
            &r.project_name,
            r.include_stale,
            max_per_type,
            max_tokens,
        );
        let snapshot = ContextSnapshot {
            recent_observations: recent,
            active_topics: vec![],
        };
        Ok(Response::new(ContextResponse {
            snapshot: Some(snapshot),
            tokens_used: Some(tokens_used),
        }))
    }

    #[instrument(skip(self, req), fields(rpc = "Timeline"))]
    async fn timeline(
        &self,
        req: Request<TimelineRequest>,
    ) -> Result<Response<TimelineResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let key = match r.anchor {
            Some(timeline_request::Anchor::Id(id)) => ObservationKey::Id(id),
            Some(timeline_request::Anchor::SyncId(s)) => ObservationKey::SyncId(s),
            None => return Err(Status::invalid_argument("missing timeline anchor")),
        };
        let conn = map(project.open_read_conn())?;
        let (before, anchor, after) =
            map(read_q::timeline(&conn, &key, r.before, r.after))?;
        Ok(Response::new(TimelineResponse {
            before: before.into_iter().map(obs_to_proto).collect(),
            anchor: Some(obs_to_proto(anchor)),
            after: after.into_iter().map(obs_to_proto).collect(),
        }))
    }

    #[instrument(skip(self, req), fields(rpc = "SuggestTopicKey"))]
    async fn suggest_topic_key(
        &self,
        req: Request<SuggestTopicKeyRequest>,
    ) -> Result<Response<SuggestTopicKeyResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let scope = if r.scope.is_empty() { "project".to_string() } else { r.scope };
        let conn = map(project.open_read_conn())?;
        let key = map(crate::suggest_topic_key::suggest(
            &conn, &r.r#type, &r.title, &scope,
        ))?;
        Ok(Response::new(SuggestTopicKeyResponse { topic_key: key }))
    }

    #[instrument(skip(self, req), fields(rpc = "CapturePassive"))]
    async fn capture_passive(
        &self,
        req: Request<CapturePassiveRequest>,
    ) -> Result<Response<CapturePassiveResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        // CapturePassive is pure markdown parsing — no project DB access
        // needed, but we still validate the project name to give consistent
        // error shape across the surface.
        let _ = map(self.open_project(&r.project_name))?;
        let snippets = crate::capture_passive::extract_key_learnings(&r.text);
        Ok(Response::new(CapturePassiveResponse { snippets }))
    }

    #[instrument(skip(self, req), fields(rpc = "Decide"))]
    async fn decide(
        &self,
        req: Request<DecideRequest>,
    ) -> Result<Response<DecideResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let inner = req.into_inner();
        let resp = crate::decide::handle(self, inner).await?;
        Ok(Response::new(resp))
    }

    /// `obs facts <id>` — return atomic facts attached to an observation.
    async fn get_facts(
        &self,
        req: Request<GetFactsRequest>,
    ) -> Result<Response<GetFactsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let conn = map(project.open_read_conn())?;
        let rows = map(facts_q::facts_for_obs(&conn, r.observation_id))?;
        let facts = rows.into_iter().map(fact_to_proto).collect();
        Ok(Response::new(GetFactsResponse { facts }))
    }

    /// `obs history <id>` — return the supersession chain for an observation.
    async fn get_observation_history(
        &self,
        req: Request<GetObservationHistoryRequest>,
    ) -> Result<Response<GetObservationHistoryResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let conn = map(project.open_read_conn())?;
        let entries = map(read_q::history_chain(&conn, r.observation_id))?;
        let proto_entries = entries
            .into_iter()
            .map(|e| memlayer_proto::ObservationHistoryEntry {
                observation: Some(obs_to_proto(e.observation)),
                superseded_by_id: e.superseded_by_id,
            })
            .collect();
        Ok(Response::new(GetObservationHistoryResponse {
            entries: proto_entries,
        }))
    }

    /// Bulk fact re-extraction (Spec 1c). Enqueues observations with no
    /// facts (or all, when only_missing=false) into the extract worker pool.
    async fn reextract_observations(
        &self,
        req: Request<ReextractObservationsRequest>,
    ) -> Result<Response<ReextractObservationsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;

        // Guard: extract must be enabled for this project.
        let cfg = memlayer_core::config::load_resolved(Some(&r.project_name));
        if !cfg.extract.enabled {
            return Err(Status::failed_precondition(
                "extract is disabled for this project; run: memlayer config set extract.enabled true",
            ));
        }

        let extract_pool = match &self.state.extract_pool {
            Some(p) => p,
            None => return Err(Status::failed_precondition("extract worker pool is not running")),
        };

        let conn = map(project.open_read_conn())?;
        let only_missing = r.only_missing;
        let since_clause = if r.since.as_deref().filter(|s| !s.is_empty()).is_some() {
            "AND created_at >= ?1"
        } else {
            ""
        };
        let facts_clause = if only_missing {
            "AND NOT EXISTS (SELECT 1 FROM facts WHERE obs_id = observations.id)"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, title, content, session_id FROM observations \
             WHERE deleted_at IS NULL {since_clause} {facts_clause}"
        );
        let since_val = r.since.as_deref().unwrap_or("").to_string();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| Status::internal(format!("reextract prepare: {e}")))?;

        // Use a consistent closure so both branches have the same type.
        let rows: Vec<(i64, String, String, Option<String>)> = {
            let res: Vec<rusqlite::Result<(i64, String, String, Option<String>)>> =
                if since_val.is_empty() {
                    stmt.query_map([], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })
                    .map_err(|e| Status::internal(format!("reextract query: {e}")))?
                    .collect()
                } else {
                    stmt.query_map(rusqlite::params![since_val], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })
                    .map_err(|e| Status::internal(format!("reextract query: {e}")))?
                    .collect()
                };
            res.into_iter()
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| Status::internal(format!("reextract row: {e}")))?
        };

        let mut queued: i64 = 0;
        let mut skipped: i64 = 0;
        for (obs_id, title, content, session_id) in rows {
            use crate::extract_worker::{ExtractTask, QueueResult};
            match extract_pool.try_queue(ExtractTask {
                project_name: r.project_name.clone(),
                obs_id,
                title,
                content,
                session_id,
            }) {
                QueueResult::Queued => queued += 1,
                _ => skipped += 1,
            }
        }
        Ok(Response::new(ReextractObservationsResponse { queued, skipped }))
    }

    /// Bulk re-embedding (Spec 1d). Enqueues observations without embeddings
    /// (or all, when force=true) into the embed worker pool.
    async fn reindex_observations(
        &self,
        req: Request<ReindexObservationsRequest>,
    ) -> Result<Response<ReindexObservationsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;

        let embed_pool = match &self.state.embed_pool {
            Some(p) => p,
            None => return Err(Status::failed_precondition(
                "embed worker pool is not running (BGE-small failed to load at startup)",
            )),
        };

        let mut cleared: i64 = 0;
        if r.force {
            // Wipe existing embeddings for this project first.
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            map(project.write.send(
                memlayer_storage::write::WriteRequest::Custom {
                    f: Box::new(|conn| {
                        conn.execute("DELETE FROM observations_vec", [])
                            .map_err(|e| memlayer_core::error::Error::internal(format!("delete vec: {e}")))?;
                        conn.execute("DELETE FROM observation_embedding_meta", [])
                            .map_err(|e| memlayer_core::error::Error::internal(format!("delete meta: {e}")))?;
                        Ok(())
                    }),
                    reply: reply_tx,
                },
            ))?;
            let del_result = reply_rx
                .await
                .map_err(|_| Status::internal("write thread crashed during reindex clear"))?;
            map(del_result)?;
            cleared = -1; // actual row count isn't returned; -1 signals "all cleared"
        }

        let conn = map(project.open_read_conn())?;
        let missing_clause = if r.force {
            ""
        } else {
            "AND id NOT IN (SELECT observation_id FROM observation_embedding_meta)"
        };
        let sql = format!(
            "SELECT id, title, content FROM observations \
             WHERE deleted_at IS NULL {missing_clause}"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| Status::internal(format!("reindex prepare: {e}")))?;
        let rows: Vec<(i64, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| Status::internal(format!("reindex query: {e}")))?
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| Status::internal(format!("reindex row: {e}")))?;

        let mut queued: i64 = 0;
        let mut skipped: i64 = 0;
        for (obs_id, title, content) in rows {
            use crate::embed_worker::{EmbedTask, QueueResult};
            match embed_pool.try_queue(EmbedTask {
                project_name: r.project_name.clone(),
                obs_id,
                title,
                content,
            }) {
                QueueResult::Queued => queued += 1,
                _ => skipped += 1,
            }
        }
        Ok(Response::new(ReindexObservationsResponse { queued, skipped, cleared }))
    }

    /// Re-verify code anchors against the project's git repo.
    #[instrument(skip(self, req), fields(rpc = "VerifyAnchors"))]
    async fn verify_anchors(
        &self,
        req: Request<VerifyAnchorsRequest>,
    ) -> Result<Response<VerifyAnchorsResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;

        let repo = map(self.state.registry.get_repo_path(&r.project_name))?
            .filter(|p| memlayer_core::git::is_repo(p))
            .or_else(|| {
                let cwd = std::env::current_dir().ok()?;
                if memlayer_core::git::is_repo(&cwd) {
                    Some(cwd)
                } else {
                    None
                }
            });
        let Some(repo) = repo else {
            return Ok(Response::new(VerifyAnchorsResponse::default()));
        };
        let head = match memlayer_core::git::head_sha(&repo) {
            Ok(h) => h,
            Err(_) => {
                return Ok(Response::new(VerifyAnchorsResponse::default()));
            }
        };

        let conn = map(project.open_read_conn())?;
        let pairs = match r.observation_id {
            Some(id) => {
                let anchors = map(memlayer_storage::anchor::anchors_for(&conn, id))?;
                anchors.into_iter().map(|a| (id, a)).collect::<Vec<_>>()
            }
            None => {
                let listed = map(memlayer_storage::anchor::list_anchored(&conn, 10_000))?;
                listed
                    .into_iter()
                    .flat_map(|(id, anchors)| anchors.into_iter().map(move |a| (id, a)))
                    .collect()
            }
        };
        let titles: std::collections::HashMap<i64, String> = {
            let ids: Vec<i64> = pairs.iter().map(|(id, _)| *id).collect();
            let mut map = std::collections::HashMap::new();
            for id in ids {
                if map.contains_key(&id) {
                    continue;
                }
                if let Ok(obs) = read_q::get(&conn, &ObservationKey::Id(id)) {
                    map.insert(id, obs.title);
                }
            }
            map
        };
        drop(conn);

        let verdicts = crate::verify::verify_anchors(&repo, &head, &pairs);
        map(crate::verify_worker::apply_verdicts_with_head(
            &project, &verdicts, &head,
        ))?;

        let mut resp = VerifyAnchorsResponse::default();
        for v in &verdicts {
            resp.changed_ids.push(v.observation_id);
            match v.state {
                memlayer_storage::VerifyState::Verified => resp.verified += 1,
                memlayer_storage::VerifyState::Stale => resp.stale += 1,
                memlayer_storage::VerifyState::Invalidated => resp.invalidated += 1,
                memlayer_storage::VerifyState::Unprovable => resp.unprovable += 1,
                memlayer_storage::VerifyState::Unanchored => resp.unanchored += 1,
            }
            resp.results.push(VerifyResult {
                id: v.observation_id,
                state: v.state.as_str().to_string(),
                title: titles.get(&v.observation_id).cloned().unwrap_or_default(),
            });
        }
        Ok(Response::new(resp))
    }

    // ---- Sessions ----

    #[instrument(skip(self, req), fields(rpc="StartSession"))]
    async fn start_session(
        &self,
        req: Request<StartSessionRequest>,
    ) -> Result<Response<StartSessionResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        if r.id.trim().is_empty() {
            return Err(Status::invalid_argument("session id is required"));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::UpsertSession {
            id: r.id.clone(),
            directory: r.directory,
            reply: tx,
        }))?;
        let session = await_write_reply(rx).await?;
        // Build context snapshot using a read connection.
        let conn = map(project.open_read_conn())?;
        let (recent, topics) = map(sessions_q::build_context_snapshot(&conn, 10))?;
        let snapshot = ContextSnapshot {
            recent_observations: recent.into_iter().map(obs_to_proto).collect(),
            active_topics: topics
                .into_iter()
                .map(|(topic_key, scope, latest_title, updated_at)| TopicSummary {
                    topic_key,
                    scope,
                    latest_title,
                    updated_at,
                })
                .collect(),
        };
        Ok(Response::new(StartSessionResponse {
            session: Some(session_to_proto(session)),
            context: Some(snapshot),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="EndSession"))]
    async fn end_session(
        &self,
        req: Request<EndSessionRequest>,
    ) -> Result<Response<EndSessionResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::EndSession {
            id: r.id,
            summary: r.summary,
            reply: tx,
        }))?;
        let s = await_write_reply(rx).await?;
        Ok(Response::new(EndSessionResponse {
            session: Some(session_to_proto(s)),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="SaveSessionSummary"))]
    async fn save_session_summary(
        &self,
        req: Request<SaveSessionSummaryRequest>,
    ) -> Result<Response<SaveSessionSummaryResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::SaveSessionSummary {
            id: r.id,
            summary: r.summary,
            reply: tx,
        }))?;
        let s = await_write_reply(rx).await?;
        Ok(Response::new(SaveSessionSummaryResponse {
            session: Some(session_to_proto(s)),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="GetSession"))]
    async fn get_session(
        &self,
        req: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let conn = map(project.open_read_conn())?;
        let s = map(sessions_q::get(&conn, &r.id))?;
        Ok(Response::new(GetSessionResponse {
            session: Some(session_to_proto(s)),
        }))
    }

    async fn list_sessions(
        &self,
        req: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let cur = map(parse_cursor(&r.cursor))?;
        let limit = if r.limit == 0 { 10 } else { r.limit };
        let conn = map(project.open_read_conn())?;
        let (rows, next) = map(sessions_q::list(&conn, limit, cur.as_ref()))?;
        let next_cursor = match next {
            Some(c) => Some(map(proto_cursor(c))?),
            None => None,
        };
        Ok(Response::new(ListSessionsResponse {
            sessions: rows.into_iter().map(session_to_proto).collect(),
            next_cursor,
        }))
    }

    async fn delete_session(
        &self,
        req: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        // FR12.8: refuse if any active observation references the session.
        let conn = map(project.open_read_conn())?;
        if map(sessions_q::has_observations(&conn, &r.id))? {
            return Err(Status::failed_precondition(format!(
                "session '{}' still has observations; delete or move them first",
                r.id
            )));
        }
        drop(conn);
        let (tx, rx) = tokio::sync::oneshot::channel();
        // The write thread doesn't have a "delete session" variant yet — we
        // submit a Custom request to run the DELETE under the write lock.
        let session_id = r.id.clone();
        map(project.write.send(WriteRequest::Custom {
            f: Box::new(move |conn: &mut rusqlite::Connection| {
                let n = conn
                    .execute("DELETE FROM sessions WHERE id = ?1", rusqlite::params![session_id])
                    .map_err(|e| Error::internal(format!("delete session: {e}")))?;
                if n == 0 {
                    return Err(Error::not_found(format!("session {session_id}")));
                }
                Ok(())
            }),
            reply: tx,
        }))?;
        await_write_reply(rx).await?;
        Ok(Response::new(DeleteSessionResponse {}))
    }

    // ---- Prompts ----

    #[instrument(skip(self, req), fields(rpc="SavePrompt"))]
    async fn save_prompt(
        &self,
        req: Request<SavePromptRequest>,
    ) -> Result<Response<SavePromptResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let sync_id = r.sync_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::SavePrompt {
            sync_id,
            session_id: r.session_id,
            content: r.content,
            reply: tx,
        }))?;
        let p = await_write_reply(rx).await?;
        Ok(Response::new(SavePromptResponse {
            prompt: Some(prompt_to_proto(p)),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="SearchPrompts"))]
    async fn search_prompts(
        &self,
        req: Request<SearchPromptsRequest>,
    ) -> Result<Response<SearchPromptsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let limit = if r.limit == 0 { 10 } else { r.limit };
        let conn = map(project.open_read_conn())?;
        let rows = map(prompts_q::search(&conn, &r.query, limit))?;
        Ok(Response::new(SearchPromptsResponse {
            prompts: rows.into_iter().map(prompt_to_proto).collect(),
        }))
    }

    #[instrument(skip(self, req), fields(rpc="RecentPrompts"))]
    async fn recent_prompts(
        &self,
        req: Request<RecentPromptsRequest>,
    ) -> Result<Response<RecentPromptsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let limit = if r.limit == 0 { 10 } else { r.limit };
        let conn = map(project.open_read_conn())?;
        let rows = map(prompts_q::recent(&conn, limit))?;
        Ok(Response::new(RecentPromptsResponse {
            prompts: rows.into_iter().map(prompt_to_proto).collect(),
        }))
    }

    async fn delete_prompt(
        &self,
        req: Request<DeletePromptRequest>,
    ) -> Result<Response<DeletePromptResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        let project = map(self.open_project(&r.project_name))?;
        let key = match r.key {
            Some(delete_prompt_request::Key::Id(id)) => PromptKey::Id(id),
            Some(delete_prompt_request::Key::SyncId(s)) => PromptKey::SyncId(s),
            None => return Err(Status::invalid_argument("missing prompt key")),
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::DeletePrompt { key, reply: tx }))?;
        await_write_reply(rx).await?;
        Ok(Response::new(DeletePromptResponse {}))
    }

    // ---- Project ops (Spec 2) ----

    async fn list_projects(
        &self,
        _req: Request<ListProjectsRequest>,
    ) -> Result<Response<ListProjectsResponse>, Status> {
        let _g = self.enter_rpc();
        let counts = map(projects_admin::list_projects_with_counts())?;
        let projects = counts
            .into_iter()
            .map(|p| ProjectInfo {
                normalized_name: p.normalized_name,
                display_name: p.display_name,
                observation_count: p.observation_count,
                session_count: p.session_count,
                prompt_count: p.prompt_count,
                created_at: p.created_at,
            })
            .collect();
        Ok(Response::new(ListProjectsResponse { projects }))
    }

    async fn current_project(
        &self,
        req: Request<CurrentProjectRequest>,
    ) -> Result<Response<CurrentProjectResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        // Daemon-side project detection mirrors the CLI's algorithm but
        // operates on the directory the caller is asking about. The full
        // PRD §9.1 walk lives in the CLI's project_detect; the daemon-side
        // is a simpler fallback used by the rare caller that doesn't run
        // detection locally.
        let dir = std::path::PathBuf::from(&r.directory);
        if !dir.is_dir() {
            return Err(Status::invalid_argument(format!(
                "directory does not exist: {}",
                dir.display()
            )));
        }
        let basename = dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Status::invalid_argument("directory has no usable basename"))?;
        let normalized = map(memlayer_core::project::normalize(basename))?;
        Ok(Response::new(CurrentProjectResponse {
            normalized_name: normalized,
            display_name: basename.to_string(),
            source: "git_root_basename".to_string(),
        }))
    }

    async fn merge_projects(
        &self,
        req: Request<MergeProjectsRequest>,
    ) -> Result<Response<MergeProjectsResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        let r = req.into_inner();
        if r.from == r.to {
            return Err(Status::invalid_argument(
                "cannot merge a project into itself",
            ));
        }
        let from_norm = map(memlayer_core::project::normalize(&r.from))?;
        let to_norm = map(memlayer_core::project::normalize(&r.to))?;
        let from_path = memlayer_core::paths::project_db_path(&from_norm);
        let to_path = memlayer_core::paths::project_db_path(&to_norm);
        if !from_path.exists() {
            return Err(Status::not_found(format!(
                "source project '{from_norm}' does not exist"
            )));
        }
        if !to_path.exists() {
            return Err(Status::not_found(format!(
                "target project '{to_norm}' does not exist"
            )));
        }
        // Route the merge through the target project's dedicated write
        // thread via WriteRequest::Custom. This keeps SQLite's single-writer
        // invariant intact — the merge tx serializes naturally with any
        // SaveObservation / UpdateObservation / etc. that the registry has
        // queued for the same target.
        let project = map(self.state.registry.get_or_open(&to_norm))?;
        let outcome: std::sync::Arc<std::sync::Mutex<Option<memlayer_storage::projects_admin::MergeOutcome>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let outcome_slot = outcome.clone();
        let from_for_closure = from_path.clone();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        map(project.write.send(memlayer_storage::write::WriteRequest::Custom {
            f: Box::new(move |conn| {
                let merged = memlayer_storage::projects_admin::merge_into_target_conn(
                    conn,
                    &from_for_closure,
                )?;
                *outcome_slot.lock().expect("outcome mutex") = Some(merged);
                Ok(())
            }),
            reply: reply_tx,
        }))?;
        map(reply_rx.await.map_err(|_| {
            memlayer_core::error::Error::Unavailable("merge_projects: write thread reply lost".into())
        }))
        .and_then(map)?;
        let outcome = outcome
            .lock()
            .expect("outcome mutex")
            .take()
            .ok_or_else(|| Status::internal("merge_projects produced no outcome"))?;
        Ok(Response::new(MergeProjectsResponse {
            observations_migrated: outcome.observations_migrated,
            sessions_migrated: outcome.sessions_migrated,
            prompts_migrated: outcome.prompts_migrated,
        }))
    }

    async fn delete_project(
        &self,
        req: Request<DeleteProjectRequest>,
    ) -> Result<Response<DeleteProjectResponse>, Status> {
        let _g = self.enter_rpc();
        map(self.check_writeable())?;
        // FR12.13: `--hard` is admin-only in TCP mode (FR7, EH-7).
        // Capture auth context before consuming the request body.
        let auth = req.extensions().get::<crate::auth::AuthCtx>().cloned();
        let inner = req.into_inner();
        if inner.hard {
            if let Some(ctx) = auth {
                if !ctx.is_admin {
                    return Err(Status::permission_denied(
                        "DeleteProject --hard requires an admin bearer token",
                    ));
                }
            }
        }
        let normalized = map(memlayer_core::project::normalize(&inner.project_name))?;
        if inner.hard {
            map(projects_admin::hard_delete_project(&normalized))?;
        } else {
            let db = memlayer_core::paths::project_db_path(&normalized);
            map(projects_admin::soft_delete_project_observations(&db))?;
        }
        Ok(Response::new(DeleteProjectResponse {}))
    }

    async fn consolidate_projects(
        &self,
        _req: Request<ConsolidateProjectsRequest>,
    ) -> Result<Response<ConsolidateProjectsResponse>, Status> {
        let _g = self.enter_rpc();
        let pairs = map(projects_admin::consolidate_candidates(0.85))?;
        let candidates = pairs
            .into_iter()
            .map(|c| consolidate_projects_response::Candidate {
                from: c.from,
                to: c.to,
                similarity: c.similarity,
            })
            .collect();
        Ok(Response::new(ConsolidateProjectsResponse { candidates }))
    }

    async fn prune_projects(
        &self,
        _req: Request<PruneProjectsRequest>,
    ) -> Result<Response<PruneProjectsResponse>, Status> {
        let _g = self.enter_rpc();
        let names = map(projects_admin::prune_candidates())?;
        Ok(Response::new(PruneProjectsResponse { would_remove: names }))
    }

    // ---- Tokens (admin-only in TCP mode / Spec 2) ----

    async fn create_token(
        &self,
        req: Request<CreateTokenRequest>,
    ) -> Result<Response<CreateTokenResponse>, Status> {
        let _g = self.enter_rpc();
        admin_guard::require_admin(&req)?;
        let r = req.into_inner();
        let store = self
            .state
            .token_store
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("token admin requires TCP mode"))?;
        let secret = map(generate_and_store_token(store, &r.name, r.is_admin))?;
        Ok(Response::new(CreateTokenResponse { secret }))
    }

    async fn list_tokens(
        &self,
        req: Request<ListTokensRequest>,
    ) -> Result<Response<ListTokensResponse>, Status> {
        let _g = self.enter_rpc();
        admin_guard::require_admin(&req)?;
        let store = self
            .state
            .token_store
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("token admin requires TCP mode"))?;
        let tokens = map(store.list())?
            .into_iter()
            .map(|m| list_tokens_response::Token {
                name: m.name,
                is_admin: m.is_admin,
                created_at: m.created_at,
                revoked_at: m.revoked_at,
            })
            .collect();
        Ok(Response::new(ListTokensResponse { tokens }))
    }

    async fn revoke_token(
        &self,
        req: Request<RevokeTokenRequest>,
    ) -> Result<Response<RevokeTokenResponse>, Status> {
        let _g = self.enter_rpc();
        admin_guard::require_admin(&req)?;
        let r = req.into_inner();
        let store = self
            .state
            .token_store
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("token admin requires TCP mode"))?;
        map(store.revoke(&r.name))?;
        Ok(Response::new(RevokeTokenResponse {}))
    }

    // ---- Daemon ops ----

    async fn daemon_status(
        &self,
        _req: Request<DaemonStatusRequest>,
    ) -> Result<Response<DaemonStatusResponse>, Status> {
        let _g = self.enter_rpc();
        let pid = std::process::id().to_string();
        Ok(Response::new(DaemonStatusResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid,
            started_at: self.state.started_at.to_rfc3339(),
            read_only_mode: self.state.disk_monitor.read_only(),
            in_flight_rpcs: self.state.in_flight.load(Ordering::Relaxed) as i64,
            cached_projects: self.state.registry.cached_count() as i64,
            cache_hit_ratio: self.state.registry.hit_ratio(),
        }))
    }

    async fn stats(
        &self,
        req: Request<StatsRequest>,
    ) -> Result<Response<StatsResponse>, Status> {
        let _g = self.enter_rpc();
        let r = req.into_inner();
        let mut out = Vec::new();
        let projects: Vec<String> = match r.project_name {
            Some(p) => vec![p],
            None => map(ProjectRegistry::list_known_on_disk())?
                .into_iter()
                .map(|(n, _)| n)
                .collect(),
        };
        for p in projects {
            let state = map(self.state.registry.get_or_open(&p))?;
            let conn = map(state.open_read_conn())?;
            let s = map(stats_q::count_for(&conn, &state.normalized))?;
            out.push(stats_response::ProjectStats {
                project: s.project,
                observations: s.observations,
                soft_deleted_observations: s.soft_deleted_observations,
                sessions: s.sessions,
                prompts: s.prompts,
            });
        }
        Ok(Response::new(StatsResponse { projects: out }))
    }

    async fn shutdown(
        &self,
        req: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        // Admin guard (FR2.3, EH-7) — checked by the auth interceptor for TCP.
        // For UDS, no auth is required (already privileged by file mode 0600).
        let auth = req.extensions().get::<crate::auth::AuthCtx>().cloned();
        if let Some(ctx) = auth {
            if !ctx.is_admin {
                return Err(Status::permission_denied("Shutdown requires admin token"));
            }
        }
        debug!("Shutdown RPC received; signalling event loop");
        let _ = self.state.shutdown_tx.send(true);
        Ok(Response::new(ShutdownResponse {}))
    }

    // ---- Sync (Spec 3) ----

    async fn sync_status(
        &self,
        req: Request<SyncStatusRequest>,
    ) -> Result<Response<SyncStatusResponse>, Status> {
        let _guard = self.enter_rpc();
        let r = req.into_inner();
        let project_name = r.project_name.as_deref().unwrap_or("default");
        let project = map(self.open_project(project_name))?;
        let resp = crate::sync_status::compute(&self.state, &project, &r).await?;
        Ok(Response::new(resp))
    }
    async fn sync_export(
        &self,
        req: Request<SyncExportRequest>,
    ) -> Result<Response<SyncExportResponse>, Status> {
        let _guard = self.enter_rpc();
        map(self.check_writeable())?;
        let resp = crate::sync_export::handle(&self.state, req.into_inner()).await?;
        Ok(Response::new(resp))
    }
    async fn sync_import(
        &self,
        _req: Request<SyncImportRequest>,
    ) -> Result<Response<SyncImportResponse>, Status> {
        Err(Status::unimplemented("SyncImport: implemented in spec-task-5"))
    }
    async fn sync_export_json(
        &self,
        _req: Request<SyncExportJsonRequest>,
    ) -> Result<Response<SyncExportJsonResponse>, Status> {
        Err(Status::unimplemented("SyncExportJson: implemented in Spec 3"))
    }
    async fn sync_import_json(
        &self,
        _req: Request<SyncImportJsonRequest>,
    ) -> Result<Response<SyncImportJsonResponse>, Status> {
        Err(Status::unimplemented("SyncImportJson: implemented in Spec 3"))
    }
    async fn sync_export_md(
        &self,
        _req: Request<SyncExportMdRequest>,
    ) -> Result<Response<SyncExportMdResponse>, Status> {
        Err(Status::unimplemented("SyncExportMd: implemented in Spec 3"))
    }
    async fn sync_import_md(
        &self,
        _req: Request<SyncImportMdRequest>,
    ) -> Result<Response<SyncImportMdResponse>, Status> {
        Err(Status::unimplemented("SyncImportMd: implemented in Spec 3"))
    }

    async fn export_mem(
        &self,
        req: Request<ExportMemRequest>,
    ) -> Result<Response<ExportMemResponse>, Status> {
        let _guard = self.enter_rpc();
        map(self.check_writeable())?;
        let resp = crate::mem_export::handle(&self.state, req.into_inner()).await?;
        Ok(Response::new(resp))
    }

    async fn import_mem(
        &self,
        req: Request<ImportMemRequest>,
    ) -> Result<Response<ImportMemResponse>, Status> {
        let _guard = self.enter_rpc();
        map(self.check_writeable())?;
        let resp = crate::mem_import::handle(&self.state, req.into_inner()).await?;
        Ok(Response::new(resp))
    }

    async fn get_observation_relations(
        &self,
        req: Request<GetObservationRelationsRequest>,
    ) -> Result<Response<GetObservationRelationsResponse>, Status> {
        let _guard = self.enter_rpc();
        let inner = req.into_inner();
        let project = map(self.open_project(&inner.project_name))?;
        let conn = map(project.open_read_conn())?;
        let rels = map(memlayer_storage::relations::get_relations_for_observation(
            &conn,
            inner.observation_id,
        ))?;
        let proto_rels = rels
            .into_iter()
            .map(|r| memlayer_proto::ObservationRelation {
                id: r.id,
                source_id: r.source_id,
                target_id: r.target_id,
                relation_type: r.relation_type,
                confidence: r.confidence,
                created_at: r.created_at,
            })
            .collect();
        Ok(Response::new(GetObservationRelationsResponse {
            relations: proto_rels,
        }))
    }

    // ---- Doctor (Spec 4) ----

    async fn doctor(
        &self,
        _req: Request<DoctorRequest>,
    ) -> Result<Response<DoctorResponse>, Status> {
        Err(Status::unimplemented("Doctor: implemented in Spec 4"))
    }
}

// ---------------------------------------------------------------------------
// Token creation helper (FR12.15, NFR6, EH-8)
// ---------------------------------------------------------------------------

/// Generate a fresh 64-char hex token, store its SHA-256 hash in `store`,
/// and return the plaintext to the caller. The plaintext is **never** logged.
///
/// Spec sections: NFR6 (no token secrets in logs), EH-8 (CreateToken handler
/// logs token name only).
pub fn generate_and_store_token(
    store: &TokenStore,
    name: &str,
    is_admin: bool,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut secret_bytes = [0u8; 32];
    getrandom::getrandom(&mut secret_bytes)
        .map_err(|e| Error::internal(format!("OS RNG: {e}")))?;
    let secret_hex = hex::encode(secret_bytes);
    let mut hasher = Sha256::new();
    hasher.update(secret_hex.as_bytes());
    let hash = hasher.finalize().to_vec();
    store.insert(TokenMeta {
        name: name.to_string(),
        hash,
        is_admin,
        created_at: chrono::Utc::now().to_rfc3339(),
        revoked_at: None,
    })?;
    // Log only the token name + admin flag — never the plaintext.
    tracing::info!(token_name = %name, is_admin, "token created");
    Ok(secret_hex)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    #[test]
    fn create_token_returns_distinct_hash() {
        // generate_and_store_token round-trip: the caller gets the plaintext
        // back, the store keeps SHA-256(plaintext), and the two are not
        // confusable. EH-8's other half (log scrubbing) lives in
        // create_token_log_does_not_leak_secret below.
        let dir = TempDir::new().unwrap();
        let store = TokenStore::open(dir.path().join("tokens.db")).unwrap();
        let secret = generate_and_store_token(&store, "alice", true).unwrap();

        // Caller receives a 64-char hex secret (32 random bytes).
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));

        let stored = store.list().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "alice");
        assert!(stored[0].is_admin);

        // The stored hash must NOT equal the plaintext bytes or its hex.
        assert_ne!(stored[0].hash, secret.as_bytes());
        assert_ne!(stored[0].hash, hex::decode(&secret).unwrap());

        // The stored hash must equal SHA-256(secret_hex_bytes).
        let mut h = Sha256::new();
        h.update(secret.as_bytes());
        let expected: Vec<u8> = h.finalize().to_vec();
        assert_eq!(stored[0].hash, expected);
    }

    /// EH-8 / NFR6: the `tracing::info!` emitted by `generate_and_store_token`
    /// must contain the token name but never the plaintext secret. We install
    /// a process-wide subscriber once (via OnceLock) that routes events into
    /// a thread-local capture buffer. The OnceLock guard handles the tracing
    /// callsite-interest cache: the global subscriber registers each
    /// callsite with `Interest::always()` the first time it's seen, so
    /// later tests on other threads can't poison the cache to `Never` and
    /// drop our event before it reaches the dispatcher.
    #[test]
    fn create_token_log_does_not_leak_secret() {
        use std::cell::RefCell;
        use std::sync::{Arc, Mutex, OnceLock};
        use tracing::field::Visit;
        use tracing::span::{Attributes, Id, Record};
        use tracing::{Event, Metadata, Subscriber};

        thread_local! {
            static CAPTURE: RefCell<Option<Arc<Mutex<String>>>> = const { RefCell::new(None) };
        }

        struct GlobalCaptureSub {
            next_id: std::sync::atomic::AtomicU64,
        }

        impl Subscriber for GlobalCaptureSub {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn register_callsite(&self, _: &'static Metadata<'static>) -> tracing::subscriber::Interest {
                tracing::subscriber::Interest::always()
            }
            fn new_span(&self, _: &Attributes<'_>) -> Id {
                let n = self
                    .next_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                Id::from_u64(n)
            }
            fn record(&self, _: &Id, _: &Record<'_>) {}
            fn record_follows_from(&self, _: &Id, _: &Id) {}
            fn event(&self, event: &Event<'_>) {
                CAPTURE.with(|c| {
                    let borrow = c.borrow();
                    if let Some(buf) = borrow.as_ref() {
                        struct V<'a>(&'a mut String);
                        impl<'a> Visit for V<'a> {
                            fn record_debug(
                                &mut self,
                                field: &tracing::field::Field,
                                value: &dyn std::fmt::Debug,
                            ) {
                                use std::fmt::Write;
                                let _ = write!(self.0, " {}={:?}", field.name(), value);
                            }
                            fn record_str(
                                &mut self,
                                field: &tracing::field::Field,
                                value: &str,
                            ) {
                                use std::fmt::Write;
                                let _ = write!(self.0, " {}={}", field.name(), value);
                            }
                            fn record_bool(
                                &mut self,
                                field: &tracing::field::Field,
                                value: bool,
                            ) {
                                use std::fmt::Write;
                                let _ = write!(self.0, " {}={}", field.name(), value);
                            }
                            fn record_u64(
                                &mut self,
                                field: &tracing::field::Field,
                                value: u64,
                            ) {
                                use std::fmt::Write;
                                let _ = write!(self.0, " {}={}", field.name(), value);
                            }
                            fn record_i64(
                                &mut self,
                                field: &tracing::field::Field,
                                value: i64,
                            ) {
                                use std::fmt::Write;
                                let _ = write!(self.0, " {}={}", field.name(), value);
                            }
                        }
                        let mut buf = buf.lock().unwrap();
                        buf.push_str(&format!("[{}]", event.metadata().level()));
                        let mut visitor = V(&mut buf);
                        event.record(&mut visitor);
                        buf.push('\n');
                    }
                });
            }
            fn enter(&self, _: &Id) {}
            fn exit(&self, _: &Id) {}
        }

        static GLOBAL_INSTALLED: OnceLock<()> = OnceLock::new();
        GLOBAL_INSTALLED.get_or_init(|| {
            let _ = tracing::subscriber::set_global_default(GlobalCaptureSub {
                next_id: std::sync::atomic::AtomicU64::new(0),
            });
        });

        // Install the per-test capture buffer in this thread's CAPTURE slot.
        let captured = Arc::new(Mutex::new(String::new()));
        CAPTURE.with(|c| *c.borrow_mut() = Some(captured.clone()));

        let dir = TempDir::new().unwrap();
        let store = TokenStore::open(dir.path().join("tokens.db")).unwrap();
        let secret = generate_and_store_token(&store, "alice", true).unwrap();

        // Drop the capture so other tests aren't affected.
        CAPTURE.with(|c| *c.borrow_mut() = None);

        let log = captured.lock().unwrap().clone();
        assert!(
            !log.is_empty(),
            "tracing subscriber captured nothing — \
             EH-8 cannot be verified.",
        );

        // Sanity: the production log line we expect is present.
        assert!(
            log.contains("token_name=alice") || log.contains("token_name=\"alice\""),
            "expected `token_name=alice` in captured log; got: {log:?}",
        );
        assert!(
            log.contains("token created"),
            "expected `token created` message in captured log; got: {log:?}",
        );

        // The actual EH-8 invariant: the plaintext secret must NOT appear
        // anywhere in the captured event fields.
        assert!(
            !log.contains(&secret),
            "EH-8 violated: plaintext secret leaked into tracing output. \
             Captured: {log:?}",
        );
        // Also reject any 16-char prefix in case a partial-truncation
        // regression slips through.
        assert!(
            !log.contains(&secret[..16]),
            "EH-8 violated: 16-char prefix of secret leaked. Captured: {log:?}",
        );
    }

    #[test]
    fn generate_and_store_token_each_call_is_unique() {
        let dir = TempDir::new().unwrap();
        let store = TokenStore::open(dir.path().join("tokens.db")).unwrap();
        let s1 = generate_and_store_token(&store, "alice", false).unwrap();
        let s2 = generate_and_store_token(&store, "bob", false).unwrap();
        assert_ne!(s1, s2, "two OsRng draws must not collide");
    }
}
