// Generated with AI Coding Rules Hub
//! Per-project extraction orchestrator (spec-task-18).
//!
//! Reads raw observations out of a single storage-side project DB, slices
//! them into overlapping 6-turn windows, fans out cache misses to claude-haiku
//! through [`ClaudeClient`], embeds the resulting facts via the supplied
//! [`Embedder`], and bulk-writes the rows into the eval-side `facts.db`.
//!
//! Two caches keep this idempotent across re-runs:
//!   * [`ExtractionCache`] keyed by `sha256(window_json)` — successful Haiku
//!     output and EH-4 "permanently failed" markers both land here so a
//!     resumed run skips work that already burned model spend.
//!   * [`EmbeddingCache`] keyed by `sha256(text)` — re-embedding identical
//!     `(subject predicate object)` strings is wasted CPU on a repeat run.
//!
//! Concurrency:
//!   The Haiku fan-out is bounded by `concurrency` (default 4) via a
//!   `tokio::sync::Semaphore`. `Arc<dyn ClaudeClient>` and `Vec<Turn>` are
//!   `Send`/`Clone`, so each window runs in its own `tokio::spawn`.
//!
//! Out of scope here:
//!   * Entity extraction (spec-task-19a/19c).
//!   * CLI subcommand wiring (spec-task-20).
//!   * Anything that touches the storage daemon's migrations (SC-10).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use memlayer_core::paths;
use memlayer_embed::cache::EmbeddingCache;
use memlayer_embed::Embedder;
use memlayer_extract::cache::{CachedExtraction, ExtractionCache};
use memlayer_extract::claude_cli::{ClaudeClient, HAIKU_MODEL};
use memlayer_extract::entities::EntityExtractor;
use memlayer_extract::{build_extraction_prompt, parse_facts, Fact, Turn};
use memlayer_storage::ProjectRegistry;

use crate::entities_writer::{bulk_upsert_entities, EntityRow};
use crate::facts_db::FactsDb;
use crate::facts_writer::{insert_facts_for_project_returning_ids, FactWithEmbedding};

const DEFAULT_CONCURRENCY: usize = 8;
const DEFAULT_WINDOW_SIZE: usize = 6;
const DEFAULT_WINDOW_STRIDE: usize = 3;

/// Per-conversation extraction stats.
#[derive(Debug, Clone, Default)]
pub struct ExtractStats {
    pub windows_total: usize,
    pub windows_extracted: usize,
    pub windows_cached: usize,
    pub windows_failed: usize,
    pub facts_written: usize,
    pub entities_written: usize,
    pub elapsed_ms: u128,
}

/// Orchestrator for the extraction pipeline.
///
/// Cheap to construct; one instance is reused across many `extract_project`
/// calls (one per LoCoMo conversation, one per LME session, etc.).
pub struct ExtractPipeline {
    pub data_dir: PathBuf,
    pub claude: Arc<dyn ClaudeClient>,
    pub embedder: Arc<dyn Embedder>,
    /// Optional entity extractor (spec-task-19c). When set, runs after fact
    /// insert to populate the entities + entity_links + entities_vec tables.
    /// When None, the pipeline behaves as in spec-task-18 (facts only).
    pub entity_extractor: Option<Arc<dyn EntityExtractor>>,
    /// Optional Haiku-backed entity extractor (P5 spec-task-27c). Used
    /// only on facts whose salience >= [`HAIKU_ENTITY_THRESHOLD`] —
    /// roughly the top 15% of extractions where richer entity capture
    /// is worth the LLM call. When None, all facts use
    /// `entity_extractor` (heuristic by default).
    pub haiku_entity_extractor: Option<Arc<dyn EntityExtractor>>,
    /// When true, after per-window extraction the pipeline groups facts
    /// by `source_session` and emits one session-summary fact per
    /// session via a single Haiku call (P5 spec-task-27d). Off by
    /// default to keep the smoke path cheap.
    pub session_summaries: bool,
    /// Concurrency cap for the Haiku fan-out (default 4).
    pub concurrency: usize,
    /// Window size in turns (default 6).
    pub window_size: usize,
    /// Window stride in turns (default 3 = 50% overlap).
    pub window_stride: usize,
    /// Number of facts.db shards (P4 spec-task-26). Default 1 = single
    /// `<benchmark>/facts.db` (LoCoMo / LongMemEval). When >1, each fact
    /// is routed to `<benchmark>-vec/shard-NN.db` via
    /// [`ShardRouter::shard_for(evidence_obs_id)`]. Used by BEAM-1M / 10M
    /// where a single facts.db saturates SQLite's page cache.
    pub shards: usize,
    /// Base directory for shard files when `shards > 1`. Defaults to
    /// `<data_dir>/<benchmark-tag>-vec/`. The pipeline is benchmark-
    /// agnostic so the caller (CLI) sets this when wiring BEAM.
    pub shard_dir: Option<PathBuf>,
}

/// Per-fact salience threshold for upgrading entity extraction from
/// the heuristic to Haiku. Locked-grill choice: ~15% of LoCoMo facts
/// land at or above this. Tightening (e.g. 0.9) cuts cost; loosening
/// (e.g. 0.75) catches more entities at higher LLM spend.
pub const HAIKU_ENTITY_THRESHOLD: f32 = 0.85;

impl ExtractPipeline {
    pub fn new(
        data_dir: impl Into<PathBuf>,
        claude: Arc<dyn ClaudeClient>,
        embedder: Arc<dyn Embedder>,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            claude,
            embedder,
            entity_extractor: None,
            haiku_entity_extractor: None,
            session_summaries: false,
            concurrency: DEFAULT_CONCURRENCY,
            window_size: DEFAULT_WINDOW_SIZE,
            window_stride: DEFAULT_WINDOW_STRIDE,
            shards: 1,
            shard_dir: None,
        }
    }

    /// Builder-style setter for the entity extractor (spec-task-19c).
    pub fn with_entity_extractor(mut self, e: Arc<dyn EntityExtractor>) -> Self {
        self.entity_extractor = Some(e);
        self
    }

    /// Builder-style setter for the Haiku-backed entity extractor used
    /// on high-salience facts (P5 spec-task-27c).
    pub fn with_haiku_entity_extractor(mut self, e: Arc<dyn EntityExtractor>) -> Self {
        self.haiku_entity_extractor = Some(e);
        self
    }

    /// Builder-style setter to enable session-summary tier facts
    /// (P5 spec-task-27d). One Haiku call per session.
    pub fn with_session_summaries(mut self, enabled: bool) -> Self {
        self.session_summaries = enabled;
        self
    }

    /// Builder-style setter for sharded extraction (P4 spec-task-26).
    /// `shards <= 1` keeps the single-DB path. `shards > 1` requires a
    /// `shard_dir` to be set via [`Self::with_shard_dir`].
    pub fn with_shards(mut self, shards: usize) -> Self {
        self.shards = shards.max(1);
        self
    }

    /// Set the shard directory for sharded BEAM extraction. Files
    /// written there are named `shard-NN.db` (zero-padded) per
    /// [`ShardRouter::shard_path`].
    pub fn with_shard_dir(mut self, dir: PathBuf) -> Self {
        self.shard_dir = Some(dir);
        self
    }

    /// Extract facts for one project (one LoCoMo conversation, one LME
    /// session, etc.). Reads raw observations from the storage-side project
    /// DB, builds overlapping windows, fans out to Haiku for cache misses,
    /// embeds the resulting facts, writes to facts.db. Idempotent across
    /// re-runs via the extraction + embedding caches.
    pub async fn extract_project(
        &self,
        project: &str,
        facts_db_path: &Path,
    ) -> Result<ExtractStats> {
        let t0 = Instant::now();
        let mut stats = ExtractStats::default();

        // ---------- 1. Open targets ----------
        // Mirror retrieve.rs: storage layer reads MEMLAYER_DATA_DIR through
        // memlayer-core::paths, so set it before the registry opens.
        std::env::set_var("MEMLAYER_DATA_DIR", &self.data_dir);
        paths::ensure_dirs(&self.data_dir).ok();

        let mut facts_db = FactsDb::open(facts_db_path)
            .with_context(|| format!("open facts.db at {}", facts_db_path.display()))?;

        let registry = Arc::new(ProjectRegistry::new(
            /* lru_capacity      */ 64,
            /* write_batch_max   */ 1,
            /* write_batch_window*/ Duration::from_millis(50),
        ));
        let project_state = registry
            .get_or_open(project)
            .with_context(|| format!("open project '{project}' for extraction"))?;
        let read_conn = project_state
            .open_read_conn()
            .context("open read connection on project DB")?;

        let extraction_cache = ExtractionCache::open(&self.data_dir)
            .context("open extraction cache")?;
        let embedding_cache = EmbeddingCache::open(&self.data_dir)
            .context("open embedding cache")?;

        // ---------- 2. Read all observations for the project ----------
        let turns = read_turns(&read_conn).context("read observations into Turn list")?;
        debug!(project, turn_count = turns.len(), "loaded turns from project DB");

        if turns.is_empty() {
            stats.elapsed_ms = t0.elapsed().as_millis();
            return Ok(stats);
        }

        // ---------- 3. Slice into windows ----------
        let windows = build_windows(&turns, self.window_size, self.window_stride);
        stats.windows_total = windows.len();

        // Cache key per window: deterministic JSON of the turn slice.
        let window_keys: Vec<String> = windows
            .iter()
            .map(|w| serde_json::to_string(w).context("serialize window for cache key"))
            .collect::<Result<Vec<_>>>()?;

        // ---------- 4. Bulk cache lookup ----------
        let key_refs: Vec<&str> = window_keys.iter().map(|s| s.as_str()).collect();
        let (cache_hits, miss_indices) = extraction_cache
            .get_many(&key_refs)
            .context("extraction cache lookup")?;

        // Tally cache hits / failed-marker hits up front. Misses are counted
        // per spawned task because some may parse-fail and convert into
        // `windows_failed` instead of `windows_extracted`.
        for hit in &cache_hits {
            match hit {
                Some(CachedExtraction::Ok(_)) => stats.windows_cached += 1,
                Some(CachedExtraction::Failed) => stats.windows_failed += 1,
                None => {}
            }
        }

        info!(
            project,
            windows_total = windows.len(),
            cache_hits = stats.windows_cached,
            misses = miss_indices.len(),
            "starting extraction"
        );

        if miss_indices.is_empty() {
            // All windows were cache hits — nothing to fan out.
            stats.elapsed_ms = t0.elapsed().as_millis();
            return Ok(stats);
        }

        // ---------- 5a. Probe: run first miss synchronously ----------
        // If the very first Haiku call returns empty (bad model ID, auth
        // failure, rate-limit), bail immediately rather than spawning
        // hundreds of tasks that will all fail identically.
        {
            let probe_idx = miss_indices[0];
            let probe_window = &windows[probe_idx];
            let probe_prompt = build_extraction_prompt(probe_window, None);
            info!(project, window = probe_idx, "probe: testing first window before fan-out");
            let probe_raw = self.claude.ask(&probe_prompt, HAIKU_MODEL).await
                .with_context(|| format!("probe window {probe_idx} failed"))?;
            let probe_facts = parse_facts(&probe_raw, probe_window)
                .unwrap_or_default();
            if probe_facts.is_empty() {
                // Cache the failure marker and abort — no point running the rest.
                let _ = extraction_cache.put_failed(&window_keys[probe_idx]);
                stats.windows_failed += 1;
                stats.elapsed_ms = t0.elapsed().as_millis();
                warn!(
                    project,
                    window = probe_idx,
                    "probe window returned empty facts — aborting fan-out (check model ID / auth)"
                );
                return Ok(stats);
            }
            info!(project, window = probe_idx, facts = probe_facts.len(), "probe ok — proceeding with fan-out");
            // Persist probe result now; fan-out loop skips index 0.
            extraction_cache.put_many(&[(window_keys[probe_idx].clone(), probe_facts)])
                .context("cache probe result")?;
            stats.windows_extracted += 1;
        }

        // ---------- 5b. Fan out Haiku for remaining misses ----------
        // We use an mpsc channel rather than collecting JoinHandles so the
        // main loop sees results as-completed rather than in spawn order.
        // Critical: the semaphore acquire happens *inside* each spawned
        // task, not in the spawn loop. If we acquired before spawning, the
        // spawn loop would block on permits and the receiver wouldn't
        // start draining until nearly the end of the run — which silently
        // suppresses every progress line. Acquiring inside lets the spawn
        // loop fire off all N tasks in milliseconds; tasks queue at the
        // semaphore inside their own futures.
        let sem = Arc::new(Semaphore::new(self.concurrency.max(1)));
        let total_to_spawn = miss_indices.len().saturating_sub(1);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(usize, Result<Vec<Fact>>)>();

        for (task_n, &i) in miss_indices.iter().enumerate().skip(1) {
            let sem = sem.clone();
            let claude = self.claude.clone();
            let window = windows[i].clone();
            let prompt = build_extraction_prompt(&window, None);
            let project_name = project.to_string();
            let total_misses = miss_indices.len();
            let tx = tx.clone();
            tokio::spawn(async move {
                let _permit = match sem.acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        // Semaphore closed — should be unreachable while we
                        // hold the Arc; bail without sending so the receiver
                        // sees fewer messages than expected (caller will
                        // notice via stats).
                        return;
                    }
                };
                debug!(
                    project = %project_name,
                    window = i,
                    task = task_n,
                    total = total_misses,
                    "calling claude haiku"
                );
                let t = std::time::Instant::now();
                let raw = match claude.ask(&prompt, HAIKU_MODEL).await {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(project = %project_name, window = i, error = %e, "claude call failed");
                        let _ = tx.send((i, Err(e)));
                        return;
                    }
                };
                debug!(
                    project = %project_name,
                    window = i,
                    elapsed_ms = t.elapsed().as_millis(),
                    response_len = raw.len(),
                    "claude haiku responded"
                );
                let parsed = parse_facts(&raw, &window);
                let _ = tx.send((i, parsed));
            });
        }
        // Drop our local sender so the receiver loop terminates once all
        // spawned tasks have dropped their clones.
        drop(tx);

        // Collected per-miss results: (window_index, facts_or_failure).
        let mut fresh_results: Vec<(usize, std::result::Result<Vec<Fact>, ()>)> =
            Vec::with_capacity(miss_indices.len());
        let total_handles = total_to_spawn;
        let progress_t0 = std::time::Instant::now();
        let mut completed = 0usize;
        let mut last_log_at = std::time::Instant::now();
        while let Some((i, result)) = rx.recv().await {
            match result {
                Ok(facts) => {
                    if facts.is_empty() {
                        // EH-4: parse_facts returned empty (either Haiku gave
                        // us prose-only output or the slice was unparseable).
                        // Mark as permanently failed so we don't reburn spend
                        // on the next run.
                        warn!(project, window_idx = i, "parse_facts returned empty; marking window failed (EH-4)");
                        stats.windows_failed += 1;
                        fresh_results.push((i, Err(())));
                    } else {
                        debug!(project, window_idx = i, facts = facts.len(), "extracted facts");
                        stats.windows_extracted += 1;
                        fresh_results.push((i, Ok(facts)));
                    }
                }
                Err(e) => {
                    warn!(window_idx = i, error = %e, "haiku call failed; marking window failed");
                    stats.windows_failed += 1;
                    fresh_results.push((i, Err(())));
                }
            }
            completed += 1;
            // Aggregate progress line — emit every completion for the first 5
            // (so the user sees movement immediately), then every 10 windows
            // OR at least every 30 seconds. This keeps the log readable on
            // 1.5k-window benchmarks while guaranteeing visible heartbeat
            // when calls are slow.
            let since_last = last_log_at.elapsed();
            let should_log = completed <= 5
                || completed == total_handles
                || completed % 10 == 0
                || since_last.as_secs() >= 30;
            if should_log {
                let elapsed = progress_t0.elapsed();
                let secs = elapsed.as_secs_f64().max(0.001);
                let rate_per_min = (completed as f64 / secs) * 60.0;
                let remaining = total_handles.saturating_sub(completed);
                let eta_s = if completed > 0 {
                    (secs / completed as f64) * remaining as f64
                } else {
                    0.0
                };
                info!(
                    project,
                    done = completed,
                    total = total_handles,
                    extracted = stats.windows_extracted,
                    failed = stats.windows_failed,
                    elapsed_s = elapsed.as_secs(),
                    rate_per_min = format!("{rate_per_min:.1}"),
                    eta_s = eta_s as u64,
                    "extract progress"
                );
                last_log_at = std::time::Instant::now();
            }
        }

        // ---------- 6. Persist successful + failed extractions ----------
        let mut put_items: Vec<(String, Vec<Fact>)> = Vec::new();
        for (i, res) in &fresh_results {
            match res {
                Ok(facts) => put_items.push((window_keys[*i].clone(), facts.clone())),
                Err(()) => {
                    if let Err(e) = extraction_cache.put_failed(&window_keys[*i]) {
                        warn!(window_idx = i, error = %e, "failed to persist failure marker");
                    }
                }
            }
        }
        if !put_items.is_empty() {
            extraction_cache
                .put_many(&put_items)
                .context("persist successful extractions")?;
        }

        // ---------- 7. Flatten facts (cached + fresh) ----------
        let mut all_facts: Vec<Fact> = Vec::new();
        // Cached hits first (in original window order).
        for hit in cache_hits.iter() {
            if let Some(CachedExtraction::Ok(facts)) = hit {
                all_facts.extend(facts.iter().cloned());
            }
        }
        // Then fresh successes.
        for (_, res) in &fresh_results {
            if let Ok(facts) = res {
                all_facts.extend(facts.iter().cloned());
            }
        }

        if all_facts.is_empty() {
            stats.elapsed_ms = t0.elapsed().as_millis();
            info!(
                project,
                ?stats,
                "extract_project: no facts extracted"
            );
            return Ok(stats);
        }

        // ---------- 8. Embed facts ----------
        let texts: Vec<String> = all_facts
            .iter()
            .map(|f| format!("{} {} {}", f.subject, f.predicate, f.object))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let (emb_hits, emb_miss_indices) = embedding_cache
            .get_many(&text_refs)
            .context("embedding cache lookup")?;

        let mut embeddings: Vec<Option<Vec<f32>>> = emb_hits;

        if !emb_miss_indices.is_empty() {
            let miss_texts: Vec<&str> =
                emb_miss_indices.iter().map(|&i| text_refs[i]).collect();
            let fresh = self
                .embedder
                .embed(&miss_texts)
                .context("embed fact texts")?;
            if fresh.len() != miss_texts.len() {
                anyhow::bail!(
                    "embedder returned {} vectors for {} inputs",
                    fresh.len(),
                    miss_texts.len()
                );
            }
            // Persist freshly computed embeddings.
            let put_items: Vec<(String, Vec<f32>)> = emb_miss_indices
                .iter()
                .zip(fresh.iter())
                .map(|(&i, v)| (texts[i].clone(), v.clone()))
                .collect();
            embedding_cache
                .put_many(&put_items)
                .context("persist freshly embedded facts")?;
            // Splice fresh vectors back into the per-fact Option<Vec<f32>>
            // array at their original indices.
            for (&i, v) in emb_miss_indices.iter().zip(fresh.into_iter()) {
                embeddings[i] = Some(v);
            }
        }

        // Build the writer batch. Any None at this point is a bug — a miss
        // index that we just filled — but guard against it defensively.
        let mut batch: Vec<FactWithEmbedding> = Vec::with_capacity(all_facts.len());
        for (fact, emb) in all_facts.into_iter().zip(embeddings.into_iter()) {
            match emb {
                Some(embedding) => batch.push(FactWithEmbedding { fact, embedding }),
                None => {
                    warn!(
                        subject = %fact.subject,
                        predicate = %fact.predicate,
                        "missing embedding for fact; skipping"
                    );
                }
            }
        }

        // ---------- 9. Write to facts.db ----------
        // P4 spec-task-26: when shards > 1, group facts by
        // ShardRouter::shard_for(evidence_obs_id) and write each
        // sub-batch into its own `<shard_dir>/shard-NN.db`. fact_ids
        // are stitched back into original input order so subsequent
        // entity_links / session_summary code keeps working unchanged.
        // The single-shard path is byte-identical to before.
        let fact_ids: Vec<i64> = if let (true, Some(shard_dir)) = (self.shards > 1, self.shard_dir.as_ref()) {
            use crate::sharding::ShardRouter;
            use std::collections::HashMap;
            let router = ShardRouter::new(self.shards);
            // Group input indices by shard.
            let mut by_shard: HashMap<usize, Vec<usize>> = HashMap::new();
            for (i, fwe) in batch.iter().enumerate() {
                let s = router.shard_for(fwe.fact.evidence_obs_id);
                by_shard.entry(s).or_default().push(i);
            }
            let mut id_by_input: Vec<i64> = vec![0; batch.len()];
            for (shard_idx, indices) in by_shard {
                let shard_path = router.shard_path(shard_dir, shard_idx);
                let mut shard_db = FactsDb::open(&shard_path).with_context(|| {
                    format!("open shard at {}", shard_path.display())
                })?;
                let sub_batch: Vec<FactWithEmbedding> =
                    indices.iter().map(|&i| batch[i].clone()).collect();
                let sub_ids = insert_facts_for_project_returning_ids(
                    &mut shard_db.conn,
                    project,
                    &sub_batch,
                )
                .with_context(|| format!("shard-{shard_idx:02} insert"))?;
                for (out_pos, &id) in indices.iter().zip(sub_ids.iter()) {
                    id_by_input[*out_pos] = id;
                }
            }
            id_by_input
        } else {
            insert_facts_for_project_returning_ids(&mut facts_db.conn, project, &batch)
                .context("insert facts into facts.db")?
        };
        stats.facts_written = fact_ids.len();

        // ---------- 10. Entity extraction + writeback (spec-task-19c) ----------
        // Sharded mode skips entity + summary post-processing — those
        // tables live in the primary facts.db and would point at ids
        // that exist only in per-shard DBs. BEAM (the sharded use
        // case) doesn't use facts at all per spec §6, so this is a
        // no-op trade-off.
        if self.shards > 1 {
            warn!(
                project,
                shards = self.shards,
                "skipping entity_links + session_summaries in sharded extraction"
            );
        } else if let Some(ext) = &self.entity_extractor {
            let entity_count = extract_and_write_entities(
                &mut facts_db.conn,
                project,
                &batch,
                &fact_ids,
                ext.as_ref(),
                self.haiku_entity_extractor.as_deref(),
                &self.embedder,
                &embedding_cache,
            )
            .await
            .context("extract + persist entities for project")?;
            stats.entities_written = entity_count;
        }

        // ---------- 11. Session-summary facts (P5 spec-task-27d) ----------
        if self.shards <= 1 && self.session_summaries && !batch.is_empty() {
            match write_session_summaries(
                &mut facts_db.conn,
                project,
                &batch,
                &fact_ids,
                self.claude.as_ref(),
                &self.embedder,
                &embedding_cache,
            )
            .await
            {
                Ok(written) => {
                    info!(project, session_summaries = written, "wrote session-tier facts");
                    stats.facts_written += written;
                }
                Err(e) => {
                    warn!(project, error = %e, "session-summary pass failed; non-fatal");
                }
            }
        }

        stats.elapsed_ms = t0.elapsed().as_millis();
        info!(project, ?stats, "extract_project complete");
        Ok(stats)
    }
}

/// For each (fact, fact_id), run the entity extractor, dedup names per
/// project, embed the unique names (cached), and bulk-upsert into
/// `entities` + `entity_links` + `entities_vec`. Returns the count of
/// distinct entities written.
async fn extract_and_write_entities(
    conn: &mut rusqlite::Connection,
    project: &str,
    batch: &[FactWithEmbedding],
    fact_ids: &[i64],
    extractor: &dyn EntityExtractor,
    haiku_extractor: Option<&dyn EntityExtractor>,
    embedder: &Arc<dyn Embedder>,
    embedding_cache: &EmbeddingCache,
) -> Result<usize> {
    use std::collections::BTreeMap;

    if batch.is_empty() || fact_ids.is_empty() {
        return Ok(0);
    }

    // 1. Per-fact entity extraction. Sequential is fine here — each call is
    //    cheap (heuristic) or already cached at the LLM client layer.
    //    BTreeMap keeps deterministic ordering for downstream embedding.
    //    P5 spec-task-27c: facts whose salience >= HAIKU_ENTITY_THRESHOLD
    //    use the Haiku extractor (when configured); the rest use the
    //    heuristic. ~15% of facts hit the threshold, capping cost.
    let mut entity_to_fact_ids: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (fwe, &fact_id) in batch.iter().zip(fact_ids.iter()) {
        let chosen: &dyn EntityExtractor = match (haiku_extractor, fwe.fact.salience) {
            (Some(h), s) if s >= HAIKU_ENTITY_THRESHOLD => h,
            _ => extractor,
        };
        let names = match chosen.extract(&fwe.fact).await {
            Ok(n) => n,
            Err(e) => {
                warn!(
                    fact_id,
                    subject = %fwe.fact.subject,
                    error = %e,
                    "entity extractor failed for fact; skipping"
                );
                continue;
            }
        };
        for name in names {
            if name.is_empty() {
                continue;
            }
            entity_to_fact_ids
                .entry(name)
                .or_default()
                .push(fact_id);
        }
    }

    if entity_to_fact_ids.is_empty() {
        return Ok(0);
    }

    // 2. Embed the unique entity names (cached).
    let names: Vec<String> = entity_to_fact_ids.keys().cloned().collect();
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let (cached, miss_indices) = embedding_cache
        .get_many(&name_refs)
        .context("entity-name cache lookup")?;
    let mut embeddings: Vec<Option<Vec<f32>>> = cached;

    if !miss_indices.is_empty() {
        let miss_texts: Vec<&str> = miss_indices.iter().map(|&i| name_refs[i]).collect();
        let fresh = embedder
            .embed(&miss_texts)
            .context("embed entity names")?;
        if fresh.len() != miss_texts.len() {
            anyhow::bail!(
                "embedder returned {} vectors for {} entity names",
                fresh.len(),
                miss_texts.len()
            );
        }
        let put_items: Vec<(String, Vec<f32>)> = miss_indices
            .iter()
            .zip(fresh.iter())
            .map(|(&i, v)| (names[i].clone(), v.clone()))
            .collect();
        embedding_cache
            .put_many(&put_items)
            .context("persist entity-name embeddings")?;
        for (&i, v) in miss_indices.iter().zip(fresh.into_iter()) {
            embeddings[i] = Some(v);
        }
    }

    // 3. Build EntityRow batch and bulk-upsert.
    let mut rows: Vec<EntityRow> = Vec::with_capacity(names.len());
    for (i, name) in names.into_iter().enumerate() {
        let Some(embedding) = embeddings[i].take() else {
            warn!(name = %name, "missing embedding for entity name; skipping");
            continue;
        };
        let fact_ids = entity_to_fact_ids.remove(&name).unwrap_or_default();
        rows.push(EntityRow {
            name,
            embedding,
            linked_fact_ids: fact_ids,
            kind: None,
        });
    }
    let written = bulk_upsert_entities(conn, project, &rows)
        .context("bulk upsert entities")?;
    Ok(written)
}

/// Read all live observations for the open project as `Turn`s, ordered by id.
///
/// Conventions (matching the LoCoMo ingest adapter):
///   * `title` becomes `Turn::speaker`.
///   * `content` becomes `Turn::text` (already prefixed `Speaker: ...` from
///     the ingest side, which the extraction prompt tolerates).
fn read_turns(conn: &rusqlite::Connection) -> Result<Vec<Turn>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, title, content
               FROM observations
              WHERE deleted_at IS NULL
              ORDER BY id ASC",
        )
        .context("prepare observations select")?;
    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let session_id: Option<String> = row.get(1)?;
            let title: String = row.get(2)?;
            let content: String = row.get(3)?;
            Ok(Turn {
                speaker: title,
                text: content,
                obs_id: id,
                session_id,
            })
        })
        .context("query observations")?;
    let mut turns = Vec::new();
    for r in rows {
        turns.push(r.context("read observation row")?);
    }
    Ok(turns)
}

/// Build overlapping windows of `size` turns advancing by `stride` between
/// starts. Stride is clamped to at least 1 so we always make progress; if
/// fewer than `size` turns exist we still emit a single short window so we
/// don't lose those facts entirely.
fn build_windows(turns: &[Turn], size: usize, stride: usize) -> Vec<Vec<Turn>> {
    if turns.is_empty() {
        return Vec::new();
    }
    let stride = stride.max(1);
    let size = size.max(1);

    if turns.len() <= size {
        return vec![turns.to_vec()];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while start < turns.len() {
        let end = (start + size).min(turns.len());
        out.push(turns[start..end].to_vec());
        if end == turns.len() {
            break;
        }
        start += stride;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(idx: i64) -> Turn {
        Turn {
            speaker: format!("speaker-{idx}"),
            text: format!("text-{idx}"),
            obs_id: idx,
            session_id: Some("s".into()),
        }
    }

    #[test]
    fn build_windows_short_input_returns_single_window() {
        let turns: Vec<Turn> = (1..=4).map(turn).collect();
        let w = build_windows(&turns, 6, 3);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].len(), 4);
    }

    #[test]
    fn build_windows_overlaps_at_50_percent() {
        let turns: Vec<Turn> = (1..=12).map(turn).collect();
        let w = build_windows(&turns, 6, 3);
        // Starts at 0, 3, 6 => windows of size 6, 6, 6 (last is 6..12).
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].first().unwrap().obs_id, 1);
        assert_eq!(w[1].first().unwrap().obs_id, 4);
        assert_eq!(w[2].first().unwrap().obs_id, 7);
        assert_eq!(w[2].last().unwrap().obs_id, 12);
    }

    #[test]
    fn build_windows_handles_uneven_tail() {
        let turns: Vec<Turn> = (1..=10).map(turn).collect();
        let w = build_windows(&turns, 6, 3);
        // Starts at 0, 3, 6 => last window is [7..10] (size 4).
        assert_eq!(w.len(), 3);
        assert_eq!(w.last().unwrap().len(), 4);
        assert_eq!(w.last().unwrap().last().unwrap().obs_id, 10);
    }

    #[test]
    fn build_windows_empty_input() {
        let w = build_windows(&[], 6, 3);
        assert!(w.is_empty());
    }
}

/// Group window-tier facts by `source_session`, ask Haiku for a 2-3
/// sentence summary per session, and insert each summary as a single
/// `tier='session'` fact. One Haiku call per session — caps the cost
/// at ~$0.50/benchmark for LoCoMo's ~70 sessions.
///
/// Failures (Haiku error, embedding error, insert error) are logged
/// but non-fatal: the per-window facts already extracted remain valid.
///
/// Returns the count of session-tier facts written. Session entries
/// with fewer than 3 source facts are skipped (not enough signal).
async fn write_session_summaries(
    conn: &mut rusqlite::Connection,
    project: &str,
    batch: &[FactWithEmbedding],
    fact_ids: &[i64],
    claude: &dyn ClaudeClient,
    embedder: &Arc<dyn Embedder>,
    embedding_cache: &EmbeddingCache,
) -> Result<usize> {
    use std::collections::HashMap;

    if batch.len() != fact_ids.len() || batch.is_empty() {
        return Ok(0);
    }

    // 1. Group facts by source_session, drop facts without one.
    let mut by_session: HashMap<String, Vec<&FactWithEmbedding>> = HashMap::new();
    for fwe in batch {
        if let Some(sess) = &fwe.fact.source_session {
            by_session.entry(sess.clone()).or_default().push(fwe);
        }
    }

    let mut written = 0usize;
    for (session, facts) in by_session {
        if facts.len() < 3 {
            continue;
        }
        // 2. Top 5 by salience as Haiku context.
        let mut top: Vec<&FactWithEmbedding> = facts.clone();
        top.sort_by(|a, b| {
            b.fact
                .salience
                .partial_cmp(&a.fact.salience)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        top.truncate(5);
        let mean_salience: f32 = top.iter().map(|f| f.fact.salience).sum::<f32>()
            / top.len() as f32;

        let context: String = top
            .iter()
            .map(|f| {
                let temp = f
                    .fact
                    .temporal
                    .as_deref()
                    .map(|t| format!("[{t}] "))
                    .unwrap_or_default();
                format!("- {}{} {} {}", temp, f.fact.subject, f.fact.predicate, f.fact.object)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "Summarize this conversation session in 2-3 short sentences. \
             Focus on what was decided, learned, or shared.\n\n\
             Top facts from the session:\n{context}\n\n\
             Respond with ONLY the summary text. No prose preamble, no bullets."
        );

        // 3. Haiku call. Failures are non-fatal — log and skip this session.
        let summary = match claude
            .ask(&prompt, memlayer_extract::claude_cli::HAIKU_MODEL)
            .await
        {
            Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
            Ok(_) => {
                warn!(project, session = %session, "session summary returned empty");
                continue;
            }
            Err(e) => {
                warn!(project, session = %session, error = %e, "session summary Haiku call failed");
                continue;
            }
        };

        // 4. Embed the summary text (cached) so it's first-class in
        //    facts_vec / facts_fts and shows up in retrieval.
        let embedding = match embed_summary(embedder, embedding_cache, &summary) {
            Ok(v) => v,
            Err(e) => {
                warn!(project, session = %session, error = %e, "embed summary failed");
                continue;
            }
        };

        // 5. Insert via direct SQL — bypasses canonical-key dedup since
        //    session summaries have a unique (project, predicate, object)
        //    combo by construction.
        let tx = conn.transaction().context("begin session summary tx")?;
        let inserted = {
            let mut ins_fact = tx
                .prepare(
                    "INSERT OR IGNORE INTO facts(project, evidence_obs_id, subject, predicate, \
                                                  object, temporal, salience, source_session, tier) \
                     VALUES (?1, 0, ?2, 'session_summary', ?3, NULL, ?4, ?5, 'session')",
                )
                .context("prepare session summary insert")?;
            let changes_before = tx.changes();
            ins_fact
                .execute(rusqlite::params![
                    project,
                    project,
                    summary,
                    mean_salience as f64,
                    session,
                ])
                .context("insert session summary")?;
            tx.changes() > changes_before
        };
        if inserted {
            let rowid = tx.last_insert_rowid();
            let bytes: Vec<u8> = embedding.iter().flat_map(|x| x.to_le_bytes()).collect();
            tx.execute(
                "INSERT INTO facts_vec(rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![rowid, bytes],
            )
            .context("insert session summary into facts_vec")?;
            written += 1;
        }
        tx.commit().context("commit session summary tx")?;
    }
    Ok(written)
}

/// Embed text via the cache, falling back to a fresh embed on miss.
fn embed_summary(
    embedder: &Arc<dyn Embedder>,
    cache: &EmbeddingCache,
    text: &str,
) -> Result<Vec<f32>> {
    let (cached, misses) = cache
        .get_many(&[text])
        .context("read embedding cache")?;
    if misses.is_empty() {
        if let Some(v) = cached.into_iter().next().flatten() {
            return Ok(v);
        }
    }
    let mut vs = embedder
        .embed(&[text])
        .context("embed session summary")?;
    let v = vs.remove(0);
    cache
        .put_many(&[(text.to_string(), v.clone())])
        .context("cache session summary embedding")?;
    Ok(v)
}
