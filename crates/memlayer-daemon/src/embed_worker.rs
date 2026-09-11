//! Async embed worker pool — fans out from the synchronous save path so
//! `obs save` returns within the SC-1 50ms p95 budget while embeddings are
//! computed off the hot path.
//!
//! Design (spec retrieval-promotion P5):
//!
//! * Per-process pool of `n` OS threads, configurable via
//!   `cfg.embed.workers` (default 2, env `MEMLAYER_EMBED_WORKERS`).
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

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tokio::sync::oneshot;

use memlayer_embed::{BgeSmallEmbedder, Embedder};
use memlayer_storage::pragmas::STORED_EMBED_MODEL;
use memlayer_storage::write::WriteRequest;
use memlayer_storage::ProjectRegistry;

/// One unit of embed work queued by `service::save_observation` after the
/// synchronous save commits. Owns the title + content text so workers don't
/// hold any borrows on the request lifetime.
#[derive(Debug, Clone)]
pub struct EmbedTask {
    pub project_name: String,
    pub obs_id: i64,
    pub title: String,
    pub content: String,
}

const QUEUE_CAPACITY: usize = 1024;

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
    ) -> Self {
        let (tx, rx) = bounded(QUEUE_CAPACITY);
        let n = n_workers.max(1);
        for i in 0..n {
            let rx = rx.clone();
            let embedder = embedder.clone();
            let registry = registry.clone();
            thread::Builder::new()
                .name(format!("memlayer-embed-{i}"))
                .spawn(move || run_loop(rx, embedder, registry))
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
) {
    while let Ok(task) = rx.recv() {
        match process_with_retry(&task, &embedder, &registry, &RETRY_BACKOFFS) {
            Ok(()) => {
                tracing::trace!(obs_id = task.obs_id, project = %task.project_name, "embed landed");
            }
            Err(e) => {
                // SC-11: embed failures must not surface to the save call;
                // log and drop after exhausting the retry budget.
                tracing::warn!(
                    obs_id = task.obs_id,
                    project = %task.project_name,
                    error = %e,
                    "embed task failed after {} retries; observation searchable via BM25 only",
                    RETRY_BACKOFFS.len(),
                );
            }
        }
    }
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
    let mut vecs = embedder
        .embed(&[combined.as_str()])
        .map_err(|e| anyhow::anyhow!("embed: {e}"))?;
    let vec = vecs
        .pop()
        .ok_or_else(|| anyhow::anyhow!("embedder returned empty result"))?;

    // Re-resolve config per task so per-project overrides apply.
    let cfg = memlayer_core::config::load_resolved(Some(&task.project_name));
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
