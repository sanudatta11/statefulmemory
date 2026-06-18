//! Async extract worker pool — config-gated atomic-fact extraction off the
//! save hot path. Mirror of `embed_worker` shape but with extra wrinkles:
//!
//! * **Config-gated.** `cfg.extract.enabled` is re-read inside the worker
//!   (per task) so per-project overrides take effect immediately. SC-7
//!   guarantees zero LLM calls when disabled globally.
//! * **Model-selectable per task.** Worker re-resolves `cfg.extract.model`
//!   per task and dispatches to a Haiku or Sonnet [`ClaudeCliExtractor`].
//!   No restart needed when the user changes models in `config.toml`.
//! * **Skip very short content.** `<10` chars worth of body produces no
//!   meaningful facts; we trace-skip the call entirely.
//!
//! Spec: retrieval-promotion P6, SC-7, SC-8, SC-12.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tokio::sync::oneshot;

use memlayer_core::config::{self, ModelKind};
use memlayer_extract::claude_cli::{ClaudeClient, ClaudeCliClient};
use memlayer_extract::{ClaudeCliExtractor, Turn};
use memlayer_storage::write::{NewFact, WriteRequest};
use memlayer_storage::ProjectRegistry;

/// One unit of extract work queued by `service::save_observation` after
/// the synchronous save commits.
#[derive(Debug, Clone)]
pub struct ExtractTask {
    pub project_name: String,
    pub obs_id: i64,
    pub title: String,
    pub content: String,
    pub session_id: Option<String>,
}

const QUEUE_CAPACITY: usize = 256;
const MIN_CONTENT_CHARS: usize = 10;

/// Outcome of [`ExtractWorkerPool::try_queue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueResult {
    Queued,
    Dropped,
    Disconnected,
}

#[derive(Clone)]
pub struct ExtractWorkerPool {
    tx: Sender<ExtractTask>,
}

impl ExtractWorkerPool {
    /// Spawn `n_workers` (clamped >=1) OS threads sharing the given
    /// `ClaudeClient`. Each worker resolves the extract model + timeout
    /// per task so config changes apply without a daemon restart.
    pub fn spawn(
        client: Arc<dyn ClaudeClient>,
        registry: Arc<ProjectRegistry>,
        n_workers: usize,
    ) -> Self {
        let (tx, rx) = bounded(QUEUE_CAPACITY);
        let n = n_workers.max(1);
        for i in 0..n {
            let rx = rx.clone();
            let client = client.clone();
            let registry = registry.clone();
            thread::Builder::new()
                .name(format!("memlayer-extract-{i}"))
                .spawn(move || run_loop(rx, client, registry))
                .expect("spawn extract worker thread");
        }
        Self { tx }
    }

    /// Spawn with the production [`ClaudeCliClient`].
    pub fn spawn_with_real_client(
        registry: Arc<ProjectRegistry>,
        n_workers: usize,
    ) -> Self {
        Self::spawn(Arc::new(ClaudeCliClient::new()), registry, n_workers)
    }

    pub fn try_queue(&self, task: ExtractTask) -> QueueResult {
        match self.tx.try_send(task) {
            Ok(_) => QueueResult::Queued,
            Err(TrySendError::Full(t)) => {
                tracing::warn!(
                    obs_id = t.obs_id,
                    project = %t.project_name,
                    "extract queue full — dropping task (capacity={QUEUE_CAPACITY})"
                );
                QueueResult::Dropped
            }
            Err(TrySendError::Disconnected(_)) => QueueResult::Disconnected,
        }
    }
}

/// Number of consecutive 429-shaped failures that trigger a pause. Spec
/// retrieval-promotion error-handling: "if 5 consecutive rate-limit
/// errors, pause the worker for 5 minutes".
const RATE_LIMIT_PAUSE_THRESHOLD: u32 = 5;
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(5 * 60);

/// Heuristic classifier for rate-limit-shaped errors from the Claude
/// shell-out. The CLI reports HTTP 429 inside its stderr text; we look
/// for substrings that consistently appear when Bedrock or Anthropic
/// rate-limits us. False positives are bounded — the worker recovers
/// after the pause window. False negatives just lose the pause backoff.
pub fn looks_like_rate_limit(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("rate-limit")
        || lower.contains("too many requests")
        || lower.contains("throttl")
}

fn run_loop(
    rx: Receiver<ExtractTask>,
    client: Arc<dyn ClaudeClient>,
    registry: Arc<ProjectRegistry>,
) {
    // The worker uses a single-threaded tokio runtime so it can drive the
    // async ClaudeCliExtractor::extract() to completion from this OS thread.
    // Blocking on `Handle::block_on` keeps the design simple — one thread,
    // one tokio current-thread runtime, nothing fancy.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "extract worker tokio runtime init failed; thread exiting");
            return;
        }
    };

    let mut consecutive_429: u32 = 0;
    while let Ok(task) = rx.recv() {
        match rt.block_on(process(&task, &client, &registry)) {
            Ok(n) => {
                consecutive_429 = 0;
                tracing::trace!(
                    obs_id = task.obs_id,
                    project = %task.project_name,
                    facts = n,
                    "extract task completed",
                );
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if looks_like_rate_limit(&msg) {
                    consecutive_429 = consecutive_429.saturating_add(1);
                    tracing::warn!(
                        obs_id = task.obs_id,
                        project = %task.project_name,
                        consecutive = consecutive_429,
                        "extract rate-limited",
                    );
                    if consecutive_429 >= RATE_LIMIT_PAUSE_THRESHOLD {
                        tracing::warn!(
                            consecutive = consecutive_429,
                            pause_secs = RATE_LIMIT_PAUSE.as_secs(),
                            "extract worker pausing on persistent 429s",
                        );
                        std::thread::sleep(RATE_LIMIT_PAUSE);
                        consecutive_429 = 0;
                    }
                } else {
                    consecutive_429 = 0;
                    tracing::warn!(
                        obs_id = task.obs_id,
                        project = %task.project_name,
                        error = %e,
                        "extract task failed; observation has no facts but is still searchable",
                    );
                }
            }
        }
    }
}

async fn process(
    task: &ExtractTask,
    client: &Arc<dyn ClaudeClient>,
    registry: &ProjectRegistry,
) -> anyhow::Result<usize> {
    // Per-task config resolution: pick up per-project overrides + env vars
    // every time. SC-12 expects extract_model in audit log to reflect the
    // model that *actually* ran.
    let cfg = config::load_resolved(Some(&task.project_name));
    if !cfg.extract.enabled {
        // Config flipped off between queue and dequeue; honor it.
        return Ok(0);
    }

    // SC-7 corollary: skip trivial content rather than burn tokens.
    if task.content.trim().chars().count() < MIN_CONTENT_CHARS {
        tracing::trace!(
            obs_id = task.obs_id,
            "skipping extract: content under {MIN_CONTENT_CHARS} chars"
        );
        return Ok(0);
    }

    let extractor = ClaudeCliExtractor::new(
        client.clone(),
        cfg.extract.model.cli_model_id(),
        Duration::from_secs(cfg.extract.timeout_secs),
    );

    let turn = Turn {
        speaker: "system".into(),
        text: format!("{}\n{}", task.title, task.content),
        obs_id: task.obs_id,
        session_id: task.session_id.clone(),
    };

    let facts = extractor.extract(&turn).await?;
    if facts.is_empty() {
        return Ok(0);
    }

    let extracted_by = cfg.extract.model.as_lowercase().to_string();
    let new_facts: Vec<NewFact> = facts
        .into_iter()
        .map(|f| NewFact {
            subject: f.subject,
            predicate: f.predicate,
            object: f.object,
            temporal: f.temporal,
            salience: Some(f.salience as f64),
            extracted_by: extracted_by.clone(),
        })
        .collect();
    let n = new_facts.len();

    let project = registry
        .get_or_open(&task.project_name)
        .map_err(|e| anyhow::anyhow!("open project: {e}"))?;

    let (reply_tx, reply_rx) = oneshot::channel();
    project
        .write
        .send(WriteRequest::InsertFacts {
            obs_id: task.obs_id,
            facts: new_facts,
            reply: reply_tx,
        })
        .map_err(|e| anyhow::anyhow!("send InsertFacts: {e}"))?;

    let r = reply_rx.await.map_err(|e| anyhow::anyhow!("recv InsertFacts reply: {e}"))?;
    r.map_err(|e| anyhow::anyhow!("write facts: {e}"))?;

    Ok(n)
}

/// Pick the configured model up front for a project so `service::save_observation`
/// can record `extract_model` in the audit log even before the worker runs.
/// Returns `None` when extract is disabled for that project.
pub fn resolved_model_for(project_name: &str) -> Option<ModelKind> {
    let cfg = config::load_resolved(Some(project_name));
    if !cfg.extract.enabled {
        return None;
    }
    Some(cfg.extract.model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_task(obs_id: i64) -> ExtractTask {
        ExtractTask {
            project_name: "p".into(),
            obs_id,
            title: "t".into(),
            content: "c".into(),
            session_id: None,
        }
    }

    #[test]
    fn full_queue_drops_silently() {
        let (tx, _rx) = bounded::<ExtractTask>(1);
        let pool = ExtractWorkerPool { tx };
        assert_eq!(pool.try_queue(mk_task(1)), QueueResult::Queued);
        assert_eq!(pool.try_queue(mk_task(2)), QueueResult::Dropped);
        drop(_rx);
    }

    #[test]
    fn disconnected_queue_returns_disconnected() {
        let (tx, rx) = bounded::<ExtractTask>(8);
        drop(rx);
        let pool = ExtractWorkerPool { tx };
        assert_eq!(pool.try_queue(mk_task(1)), QueueResult::Disconnected);
    }

    #[test]
    fn resolved_model_for_returns_none_when_disabled() {
        // Default cfg has extract.enabled=false → expect None.
        // Ensure no env override leaks in.
        std::env::remove_var("MEMLAYER_EXTRACT_ENABLED");
        std::env::remove_var("MEMLAYER_DATA_DIR");
        // Set a fresh empty data dir so no global config.toml leaks.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("MEMLAYER_DATA_DIR", dir.path());
        assert_eq!(resolved_model_for("any-project"), None);
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn looks_like_rate_limit_matches_known_patterns() {
        assert!(looks_like_rate_limit("HTTP 429 Too Many Requests"));
        assert!(looks_like_rate_limit("Bedrock returned a rate limit error"));
        assert!(looks_like_rate_limit("Throttled by upstream"));
        assert!(looks_like_rate_limit("rate-limit exceeded"));
        assert!(!looks_like_rate_limit("connection refused"));
        assert!(!looks_like_rate_limit("invalid model id"));
        assert!(!looks_like_rate_limit("timeout after 30s"));
    }

    #[test]
    fn rate_limit_pause_constants_match_spec() {
        // Spec retrieval-promotion error-handling: pause 5min after 5
        // consecutive rate limits.
        assert_eq!(RATE_LIMIT_PAUSE_THRESHOLD, 5);
        assert_eq!(RATE_LIMIT_PAUSE, Duration::from_secs(300));
    }
}
