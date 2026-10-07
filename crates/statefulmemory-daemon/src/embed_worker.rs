//! Async embed worker pool — fans out from the synchronous save path so
//! `obs save` returns within the SC-1 50ms p95 budget while embeddings are
//! computed off the hot path.
//!
//! Design (spec retrieval-promotion P5):
//!
//! * Per-process pool of `n` OS threads, configurable via
//!   `cfg.embed.workers` (default 2, env `STATEFULMEMORY_EMBED_WORKERS`).
//! * Bounded crossbeam channel of capacity 1024. `try_queue` drops on
//!   full — prefer freshness over backlog (SC-1).
//! * Each worker holds an `Arc<BgeSmallEmbedder>` so the model weights are
//!   loaded once per process, not per thread.
//! * Workers compute the embedding, then send a `WriteRequest::InsertEmbedding`
//!   to the per-project write thread and block on the reply. Errors are
//!   logged and the task is dropped (SC-11: embed failure must not break
//!   `obs save`).

use std::sync::Arc;
use std::thread;

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use tokio::sync::oneshot;

use statefulmemory_embed::cache::EmbeddingCache;
use statefulmemory_embed::{BgeSmallEmbedder, Embedder};
use statefulmemory_storage::pragmas::STORED_EMBED_MODEL;
use statefulmemory_storage::write::WriteRequest;
use statefulmemory_storage::ProjectRegistry;

/// One unit of embed work queued by `service::save_observation` after the
/// synchronous save commits. Owns the title + content text so workers don't
/// hold any borrows on the request lifetime.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmbedTask {
    pub project_name: String,
    pub obs_id: i64,
    pub title: String,
    pub content: String,
}

const QUEUE_CAPACITY: usize = 1024;

/// Max tasks coalesced into a single batched embed forward. Larger batches
/// amortize the BertModel forward across more rows (reindex / import);
/// isolated saves still run as batch-of-1 because [`drain_batch`] never waits.
const EMBED_BATCH_MAX: usize = 32;

/// Outcome of [`EmbedWorkerPool::try_queue`]. Used by the audit log + tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueResult {
    Queued,
    /// Queue was full at the moment of try_send (SC-1: prefer freshness).
    Dropped,
    /// Workers all exited; pool is no longer usable.
    Disconnected,
}

/// Handle to the running pool. Cheap to clone.
#[derive(Clone)]
pub struct EmbedWorkerPool {
    tx: Sender<EmbedTask>,
}

impl EmbedWorkerPool {
    /// Spawn `n_workers` OS threads (clamped to >= 1). Each holds a clone
    /// of the shared embedder + project registry. Idempotent for the
    /// caller — the returned handle is the only way to enqueue.
    pub fn spawn(
        embedder: Arc<BgeSmallEmbedder>,
        registry: Arc<ProjectRegistry>,
        n_workers: usize,
        cache: Option<Arc<EmbeddingCache>>,
    ) -> Self {
        let (tx, rx) = bounded(QUEUE_CAPACITY);
        let n = n_workers.max(1);
        for i in 0..n {
            let rx = rx.clone();
            let embedder = embedder.clone();
            let registry = registry.clone();
            let cache = cache.clone();
            thread::Builder::new()
                .name(format!("statefulmemory-embed-{i}"))
                .spawn(move || run_loop(rx, embedder, registry, cache))
                .expect("spawn embed worker thread");
        }
        Self { tx }
    }

    /// Best-effort enqueue. Drops on full queue; returns immediately. Per
    /// SC-1, save latency must not depend on embed pipeline backpressure.
    pub fn try_queue(&self, task: EmbedTask) -> QueueResult {
        match self.tx.try_send(task) {
            Ok(_) => QueueResult::Queued,
            Err(TrySendError::Full(t)) => {
                tracing::warn!(
                    obs_id = t.obs_id,
                    project = %t.project_name,
                    "embed queue full — dropping task (capacity={QUEUE_CAPACITY})"
                );
                QueueResult::Dropped
            }
            Err(TrySendError::Disconnected(_)) => QueueResult::Disconnected,
        }
    }
}

fn run_loop(
    rx: Receiver<EmbedTask>,
    embedder: Arc<BgeSmallEmbedder>,
    registry: Arc<ProjectRegistry>,
    cache: Option<Arc<EmbeddingCache>>,
) {
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(first) => {
                // Coalesce any already-queued tasks into one batched forward.
                // Isolated saves stay batch-of-1 (no added latency); bursts
                // (reindex / import) drain up to EMBED_BATCH_MAX per forward.
                let batch = drain_batch(&rx, first, EMBED_BATCH_MAX);
                process_batch(&batch, &embedder, &registry, cache.as_deref());
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Err(error) = recover_one(&embedder, &registry) {
                    tracing::debug!(error = %error, "durable embed recovery poll failed");
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Collect `first` plus any immediately-available tasks (non-blocking) up to
/// `max`. Never waits, so a lone save isn't delayed waiting to fill a batch.
fn drain_batch(rx: &Receiver<EmbedTask>, first: EmbedTask, max: usize) -> Vec<EmbedTask> {
    let mut batch = Vec::with_capacity(max.max(1));
    batch.push(first);
    while batch.len() < max {
        match rx.try_recv() {
            Ok(task) => batch.push(task),
            Err(_) => break,
        }
    }
    batch
}

/// Embed a drained batch in a single BertModel forward, then write each
/// embedding to its project's write thread. Tasks may span projects (the
/// queue is global); embedding is model-only so it batches across projects,
/// and the writes dispatch per project afterward.
///
/// Fallbacks preserve the pre-batch guarantees: one task uses the per-task
/// retry path; a batch-embed error or length mismatch re-runs each task
/// individually (with retry); a per-write failure retries that one task.
fn process_batch(
    tasks: &[EmbedTask],
    embedder: &BgeSmallEmbedder,
    registry: &ProjectRegistry,
    cache: Option<&EmbeddingCache>,
) {
    match tasks.len() {
        0 => {}
        1 => process_queued_task(&tasks[0], embedder, registry),
        _ => {
            let combined: Vec<String> = tasks
                .iter()
                .map(|t| format!("{} {}", t.title, t.content))
                .collect();
            let refs: Vec<&str> = combined.iter().map(|s| s.as_str()).collect();

            // On-disk embedding cache (Phase 1.6c): reuse vectors for identical
            // content (reindex / duplicates) so we only run BERT on the misses.
            let (cached, miss_idx): (Vec<Option<Vec<f32>>>, Vec<usize>) = match cache {
                Some(c) => c.get_many(&refs).unwrap_or_else(|e| {
                    tracing::debug!(error = %e, "embed cache get_many failed — embedding all");
                    (vec![None; tasks.len()], (0..tasks.len()).collect())
                }),
                None => (vec![None; tasks.len()], (0..tasks.len()).collect()),
            };
            let miss_texts: Vec<&str> = miss_idx.iter().map(|&i| refs[i]).collect();
            let embedded = if miss_texts.is_empty() {
                Vec::new()
            } else {
                match embedder.embed(&miss_texts) {
                    Ok(v) if v.len() == miss_texts.len() => v,
                    _ => {
                        tracing::debug!(
                            batch = tasks.len(),
                            misses = miss_texts.len(),
                            "batch embed failed/mismatch — falling back to per-task retry"
                        );
                        for task in tasks {
                            process_queued_task(task, embedder, registry);
                        }
                        return;
                    }
                }
            };

            // Assemble full vectors in task order (cached where present, freshly
            // embedded otherwise) and collect the newly-embedded for caching.
            let mut emb_iter = embedded.into_iter();
            let mut to_cache: Vec<(String, Vec<f32>)> = Vec::new();
            let mut full: Vec<Vec<f32>> = Vec::with_capacity(tasks.len());
            for (i, _task) in tasks.iter().enumerate() {
                match &cached[i] {
                    Some(c) => full.push(c.clone()),
                    None => match emb_iter.next() {
                        Some(e) => {
                            to_cache.push((combined[i].clone(), e.clone()));
                            full.push(e);
                        }
                        None => break,
                    },
                }
            }
            if full.len() != tasks.len() {
                tracing::debug!("cache/embed assembly short — falling back to per-task");
                for task in tasks {
                    process_queued_task(task, embedder, registry);
                }
                return;
            }
            if let Some(c) = cache {
                if !to_cache.is_empty() {
                    if let Err(e) = c.put_many(&to_cache) {
                        tracing::debug!(error = %e, "embed cache put_many failed (ignored)");
                    }
                }
            }

            for (task, vec) in tasks.iter().zip(full) {
                if let Err(error) = write_embedding(task, vec, embedder, registry) {
                    tracing::debug!(
                        obs_id = task.obs_id,
                        error = %error,
                        "batch write failed — retrying task individually"
                    );
                    process_queued_task(task, embedder, registry);
                } else {
                    tracing::trace!(
                        obs_id = task.obs_id,
                        project = %task.project_name,
                        "embed landed (batched, cache-aware)"
                    );
                }
            }
        }
    }
}

fn process_queued_task(
    task: &EmbedTask,
    embedder: &BgeSmallEmbedder,
    registry: &ProjectRegistry,
) {
    match process_with_retry(task, embedder, registry, &RETRY_BACKOFFS) {
        Ok(()) => {
            tracing::trace!(obs_id = task.obs_id, project = %task.project_name, "embed landed");
        }
        Err(error) => {
            tracing::warn!(
                obs_id = task.obs_id,
                project = %task.project_name,
                error = %error,
                "embed task failed after {} retries; observation searchable via BM25 only",
                RETRY_BACKOFFS.len(),
            );
        }
    }
}

fn recover_one(
    embedder: &BgeSmallEmbedder,
    registry: &ProjectRegistry,
) -> anyhow::Result<()> {
    let projects = ProjectRegistry::list_known_on_disk()?;
    for (_name, config) in projects {
        let project = registry.get_or_open(&config.project_display_name)?;
        let (reply, receiver) = oneshot::channel();
        project.write.send(WriteRequest::ClaimJob {
            kind: "embed".into(),
            reply,
        })?;
        let Some(job) = receiver.blocking_recv()?? else {
            continue;
        };
        let task = match serde_json::from_str::<EmbedTask>(&job.payload) {
            Ok(task) => task,
            Err(error) => {
                fail_job(registry, &job.project_name, job.id, &error.to_string())?;
                continue;
            }
        };
        match process_with_retry(&task, embedder, registry, &RETRY_BACKOFFS) {
            Ok(()) => complete_job(registry, &job.project_name, job.id)?,
            Err(error) => fail_job(registry, &job.project_name, job.id, &error.to_string())?,
        }
    }
    Ok(())
}

fn complete_job(
    registry: &ProjectRegistry,
    project_name: &str,
    id: i64,
) -> anyhow::Result<()> {
    let project = registry.get_or_open(project_name)?;
    let (reply, receiver) = oneshot::channel();
    project.write.send(WriteRequest::CompleteJob { id, reply })?;
    receiver.blocking_recv()??;
    Ok(())
}

fn fail_job(
    registry: &ProjectRegistry,
    project_name: &str,
    id: i64,
    error: &str,
) -> anyhow::Result<()> {
    let project = registry.get_or_open(project_name)?;
    let (reply, receiver) = oneshot::channel();
    project.write.send(WriteRequest::FailJob {
        id,
        error: error.to_string(),
        retry_delay_secs: 30,
        reply,
    })?;
    receiver.blocking_recv()??;
    Ok(())
}

/// Exponential backoff schedule for transient embed failures. Three retries
/// at 100ms / 1s / 10s — total worst-case wait ~11s before drop. Spec
/// retrieval-promotion error-handling table: "Embed worker crashes /
/// hangs → retry up to 3× with exponential backoff".
const RETRY_BACKOFFS: [std::time::Duration; 3] = [
    std::time::Duration::from_millis(100),
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(10),
];

/// Run [`process`] with up to `backoffs.len()` retries. Each failure logs
/// at trace level so a flapping embedder is observable in the daemon log,
/// but the caller doesn't see the noise unless the final attempt fails.
fn process_with_retry(
    task: &EmbedTask,
    embedder: &BgeSmallEmbedder,
    registry: &ProjectRegistry,
    backoffs: &[std::time::Duration],
) -> anyhow::Result<()> {
    let mut attempt = 0usize;
    retry_with_backoff(backoffs, || {
        let result = process(task, embedder, registry);
        if let Err(ref e) = result {
            tracing::trace!(
                obs_id = task.obs_id,
                attempt = attempt + 1,
                error = %e,
                "embed attempt failed"
            );
            attempt += 1;
        }
        result
    })
}

fn process(
    task: &EmbedTask,
    embedder: &BgeSmallEmbedder,
    registry: &ProjectRegistry,
) -> anyhow::Result<()> {
    let combined = format!("{} {}", task.title, task.content);
    let vec = embedder
        .embed(&[combined.as_str()])
        .map_err(|e| anyhow::anyhow!("embed: {e}"))?
        .pop()
        .ok_or_else(|| anyhow::anyhow!("embedder returned empty result"))?;
    write_embedding(task, vec, embedder, registry)
}

/// Send one already-computed embedding to its project's write thread and block
/// on the reply. Factored out of [`process`] so the batched path can reuse it
/// after a single multi-row forward.
///
/// Phase 2.5 (opt-in via `embed.chunk_long_content`): after the whole-document
/// embedding lands, also chunk long content, embed each chunk, and write them
/// via `InsertChunks`. Best-effort — a chunking failure is logged and does
/// NOT fail the save (the whole-document vector is already written).
fn write_embedding(
    task: &EmbedTask,
    vec: Vec<f32>,
    embedder: &BgeSmallEmbedder,
    registry: &ProjectRegistry,
) -> anyhow::Result<()> {
    // Re-resolve config per task so per-project overrides apply.
    let cfg = statefulmemory_core::config::load_resolved(Some(&task.project_name));
    let quantize = cfg.embed.quantize;

    let project = registry
        .get_or_open(&task.project_name)
        .map_err(|e| anyhow::anyhow!("open project: {e}"))?;

    let (reply_tx, reply_rx) = oneshot::channel();
    project
        .write
        .send(WriteRequest::InsertEmbedding {
            obs_id: task.obs_id,
            embedding: vec,
            model: STORED_EMBED_MODEL.to_string(),
            quantize,
            reply: reply_tx,
        })
        .map_err(|e| anyhow::anyhow!("send InsertEmbedding: {e}"))?;

    // We're on a dedicated worker thread (no async runtime), so block
    // until the write thread replies. The write thread keeps its own
    // batching window; this blocks at most ~5ms on a healthy system.
    let r = reply_rx
        .blocking_recv()
        .map_err(|e| anyhow::anyhow!("recv InsertEmbedding reply: {e}"))?;
    r.map_err(|e| anyhow::anyhow!("write embedding: {e}"))?;

    if cfg.embed.chunk_long_content {
        if let Err(e) = write_chunks(task, embedder, &cfg, &project) {
            tracing::debug!(
                obs_id = task.obs_id,
                error = %e,
                "chunk embedding failed (whole-document vector already written; ignored)"
            );
        }
    }
    Ok(())
}

/// Chunk `task`'s content, embed each chunk, and write them via
/// `InsertChunks`. No-op (not an error) when the content doesn't need
/// chunking (`chunk_by_words` returns a single whole-text chunk).
fn write_chunks(
    task: &EmbedTask,
    embedder: &BgeSmallEmbedder,
    cfg: &statefulmemory_core::config::StatefulMemoryConfig,
    project: &std::sync::Arc<statefulmemory_storage::ProjectState>,
) -> anyhow::Result<()> {
    let combined = format!("{} {}", task.title, task.content);
    let pieces = statefulmemory_embed::chunker::chunk_by_words(
        &combined,
        cfg.embed.chunk_words,
        cfg.embed.chunk_overlap_words,
    );
    if pieces.len() <= 1 {
        return Ok(()); // short content — no separate chunk vectors needed.
    }

    let refs: Vec<&str> = pieces.iter().map(|s| s.as_str()).collect();
    let vecs = embedder
        .embed(&refs)
        .map_err(|e| anyhow::anyhow!("embed chunks: {e}"))?;
    if vecs.len() != pieces.len() {
        anyhow::bail!(
            "chunk embed count mismatch: {} pieces, {} vectors",
            pieces.len(),
            vecs.len()
        );
    }
    let chunks: Vec<(String, Vec<f32>)> = pieces.into_iter().zip(vecs).collect();

    let (reply_tx, reply_rx) = oneshot::channel();
    project
        .write
        .send(WriteRequest::InsertChunks {
            obs_id: task.obs_id,
            chunks,
            reply: reply_tx,
        })
        .map_err(|e| anyhow::anyhow!("send InsertChunks: {e}"))?;
    let r = reply_rx
        .blocking_recv()
        .map_err(|e| anyhow::anyhow!("recv InsertChunks reply: {e}"))?;
    r.map_err(|e| anyhow::anyhow!("write chunks: {e}"))?;
    Ok(())
}

/// Generic retry-with-backoff helper used by `process_with_retry` and
/// exercised directly in unit tests. Public-in-module so tests can drive
/// the retry policy without standing up a real embedder.
#[allow(clippy::needless_range_loop)]
fn retry_with_backoff<F, T, E>(backoffs: &[std::time::Duration], mut op: F) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
{
    let max_attempts = backoffs.len() + 1;
    let mut last_err: Option<E> = None;
    for attempt in 0..max_attempts {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < max_attempts {
                    std::thread::sleep(backoffs[attempt]);
                }
            }
        }
    }
    Err(last_err.expect("loop ran at least once"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_task(obs_id: i64) -> EmbedTask {
        EmbedTask {
            project_name: "p".into(),
            obs_id,
            title: "a".into(),
            content: "b".into(),
        }
    }

    #[test]
    fn full_queue_drops_silently() {
        // Channel of capacity 1; we hold the receiver alive so try_send
        // sees the queue as Full (not Disconnected) on the second send.
        let (tx, _rx) = bounded::<EmbedTask>(1);
        let pool = EmbedWorkerPool { tx };
        let r1 = pool.try_queue(mk_task(1));
        assert_eq!(r1, QueueResult::Queued);
        let r2 = pool.try_queue(mk_task(2));
        assert_eq!(r2, QueueResult::Dropped, "must drop on full queue (SC-1)");
        // Keep _rx alive across both sends.
        drop(_rx);
    }

    #[test]
    fn disconnected_queue_returns_disconnected() {
        let (tx, rx) = bounded::<EmbedTask>(8);
        drop(rx);
        let pool = EmbedWorkerPool { tx };
        assert_eq!(pool.try_queue(mk_task(1)), QueueResult::Disconnected);
    }

    #[test]
    fn pool_is_clone() {
        let (tx, _rx) = bounded::<EmbedTask>(1);
        let pool = EmbedWorkerPool { tx };
        let _clone: EmbedWorkerPool = pool.clone();
    }

    #[test]
    fn drain_batch_collects_available_up_to_max() {
        let (tx, rx) = bounded::<EmbedTask>(16);
        for i in 0..5 {
            tx.try_send(mk_task(i)).unwrap();
        }
        // Mirror run_loop: recv the first, then drain the rest non-blocking.
        let first = rx.recv().unwrap();
        let batch = drain_batch(&rx, first, EMBED_BATCH_MAX);
        assert_eq!(batch.len(), 5, "drains all queued tasks when under max");
        assert_eq!(batch[0].obs_id, 0);
        assert_eq!(batch[4].obs_id, 4);
        assert!(rx.try_recv().is_err(), "channel drained");
    }

    #[test]
    fn drain_batch_respects_max_and_leaves_remainder() {
        let (tx, rx) = bounded::<EmbedTask>(16);
        for i in 0..10 {
            tx.try_send(mk_task(i)).unwrap();
        }
        let first = rx.recv().unwrap();
        let batch = drain_batch(&rx, first, 4);
        assert_eq!(batch.len(), 4, "never exceeds max");
        // 10 queued; recv took 1, drain took 3 more (max 4) → 6 remain.
        let mut remaining = 0;
        while rx.try_recv().is_ok() {
            remaining += 1;
        }
        assert_eq!(remaining, 6);
    }

    #[test]
    fn drain_batch_single_task_when_queue_empty() {
        let (_tx, rx) = bounded::<EmbedTask>(16);
        let batch = drain_batch(&rx, mk_task(42), EMBED_BATCH_MAX);
        assert_eq!(batch.len(), 1, "lone save stays batch-of-1 (no waiting)");
        assert_eq!(batch[0].obs_id, 42);
    }

    #[test]
    fn retry_backoffs_are_three_steps_total_under_15s() {
        // Spec retrieval-promotion error-handling: 3 retries at 100ms, 1s,
        // 10s. Total worst-case wait ~11s.
        assert_eq!(RETRY_BACKOFFS.len(), 3);
        assert_eq!(RETRY_BACKOFFS[0], std::time::Duration::from_millis(100));
        assert_eq!(RETRY_BACKOFFS[1], std::time::Duration::from_secs(1));
        assert_eq!(RETRY_BACKOFFS[2], std::time::Duration::from_secs(10));
        let total: std::time::Duration = RETRY_BACKOFFS.iter().sum();
        assert!(total < std::time::Duration::from_secs(15));
    }

    #[test]
    fn retry_loop_succeeds_on_first_attempt_when_op_succeeds() {
        // Drive process_with_retry with a tiny operation harness that
        // counts attempts. The real `process` needs a BgeSmallEmbedder;
        // here we exercise the retry policy directly via a free-standing
        // helper so the test stays hermetic.
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let backoffs = [
            std::time::Duration::from_millis(0),
            std::time::Duration::from_millis(0),
            std::time::Duration::from_millis(0),
        ];
        let result = retry_with_backoff(&backoffs, || {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<(), anyhow::Error>(())
        });
        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_loop_exhausts_budget_on_persistent_failure() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let backoffs = [
            std::time::Duration::from_millis(0),
            std::time::Duration::from_millis(0),
            std::time::Duration::from_millis(0),
        ];
        let result = retry_with_backoff(&backoffs, || {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(anyhow::anyhow!("flaky"))
        });
        assert!(result.is_err());
        // 1 initial + 3 retries = 4 attempts.
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 4);
    }

    #[test]
    fn retry_loop_recovers_after_two_failures() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let backoffs = [
            std::time::Duration::from_millis(0),
            std::time::Duration::from_millis(0),
            std::time::Duration::from_millis(0),
        ];
        let result = retry_with_backoff(&backoffs, || {
            let n = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 2 {
                Err::<(), _>(anyhow::anyhow!("flaky"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
