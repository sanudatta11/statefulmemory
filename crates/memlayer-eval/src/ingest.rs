// Generated with AI Coding Rules Hub
//! Bulk ingestion of EvalMemory records into memlayer projects via the
//! storage layer directly (no daemon / gRPC required).
//!
//! Performance design (eval workload only — fresh datasets, no dedup needed):
//!   1. Cache opened projects across the whole run (skip per-call `get_or_open`).
//!   2. Track upserted (project, session_id) pairs so each session is sent once.
//!   3. Pipeline: send all `UpsertSession` + `SaveObservation` requests for a
//!      large slice, then await every reply at the end. The per-project write
//!      thread coalesces requests into one SQLite transaction (up to
//!      `write_batch_max`), turning ~6k round-trips into a few transactions.
//!   4. `write_batch_max` is sized large enough that one batch covers a whole
//!      conversation, so each project commits once per ingest call.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::oneshot;
use tracing::info;

use memlayer_core::paths;
use memlayer_storage::{
    write::{SaveObservationInput, WriteRequest},
    ProjectRegistry, ProjectState,
};

use crate::datasets::EvalMemory;

/// Ingest a slice of memories into a fresh registry rooted at `data_dir`.
///
/// Returns the total number of observations written.
pub async fn ingest_memories(
    data_dir: &Path,
    memories: &[EvalMemory],
    _batch_size_hint: usize,
) -> Result<usize> {
    // Point the storage layer at our eval data dir.
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);
    paths::ensure_dirs(data_dir).context("ensure eval data dirs")?;

    // Large batch_max so a whole conversation lands in one transaction.
    let registry = Arc::new(ProjectRegistry::new(
        /* lru_capacity */ 64,
        /* write_batch_max */ 8192,
        /* write_batch_window */ Duration::from_millis(10),
    ));

    let mut project_cache: HashMap<String, Arc<ProjectState>> = HashMap::new();
    let mut upserted_sessions: HashSet<(String, String)> = HashSet::new();
    let directory = data_dir.to_string_lossy().to_string();

    let mut session_replies: Vec<oneshot::Receiver<memlayer_core::Result<memlayer_storage::Session>>> = Vec::new();
    let mut obs_replies: Vec<oneshot::Receiver<
        memlayer_core::Result<memlayer_storage::Observation>,
    >> = Vec::new();

    // Phase 1: fire all writes, collecting reply receivers.
    for mem in memories {
        let project = match project_cache.get(&mem.project) {
            Some(p) => p.clone(),
            None => {
                let p = registry
                    .get_or_open(&mem.project)
                    .with_context(|| format!("open project '{}'", mem.project))?;
                project_cache.insert(mem.project.clone(), p.clone());
                p
            }
        };

        // Send UpsertSession only on first sight of a (project, session_id) pair.
        if upserted_sessions.insert((mem.project.clone(), mem.session_id.clone())) {
            let (reply_tx, reply_rx) = oneshot::channel();
            project
                .write
                .send(WriteRequest::UpsertSession {
                    id: mem.session_id.clone(),
                    directory: directory.clone(),
                    reply: reply_tx,
                })
                .context("send UpsertSession")?;
            session_replies.push(reply_rx);
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        project
            .write
            .send(WriteRequest::SaveObservation {
                input: SaveObservationInput {
                    sync_id: Some(uuid::Uuid::new_v4().to_string()),
                    session_id: mem.session_id.clone(),
                    r#type: mem.obs_type.clone(),
                    title: mem.title.clone(),
                    content: mem.content.clone(),
                    tool_name: None,
                    scope: "project".to_string(),
                    created_by: Some("memlayer-eval".to_string()),
                    topic_key: mem.topic_key.clone(),
                    dedupe_window_secs: 0, // disable dedup in eval
                    max_content_chars: 8192,
                },
                reply: reply_tx,
            })
            .context("send SaveObservation")?;
        obs_replies.push(reply_rx);
    }

    // Phase 2: drain replies. Sessions first (FK ordering is guaranteed by
    // channel FIFO + single-transaction commit, but awaiting in this order
    // surfaces session errors before observation errors for clearer diagnostics).
    for rx in session_replies {
        rx.await.context("await UpsertSession reply")??;
    }
    let mut total_written = 0usize;
    for rx in obs_replies {
        rx.await.context("await SaveObservation reply")??;
        total_written += 1;
    }

    info!(total = total_written, "ingest complete");
    Ok(total_written)
}

/// Stream-ingest a large BEAM-scale dataset without materialising the full
/// vector in memory. Accepts an iterator instead of a slice.
pub async fn ingest_stream<I>(
    data_dir: &Path,
    memories: I,
    batch_size: usize,
) -> Result<usize>
where
    I: Iterator<Item = EvalMemory>,
{
    let mut buf: Vec<EvalMemory> = Vec::with_capacity(batch_size);
    let mut total = 0usize;

    let mut iter = memories.peekable();
    while iter.peek().is_some() {
        buf.clear();
        buf.extend(iter.by_ref().take(batch_size));
        total += ingest_memories(data_dir, &buf, batch_size).await?;
    }
    Ok(total)
}
