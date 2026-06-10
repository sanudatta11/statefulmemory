// Generated with AI Coding Rules Hub
//! Bulk ingestion of EvalMemory records into memlayer projects via the
//! storage layer directly (no daemon / gRPC required).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info};

use memlayer_core::paths;
use memlayer_storage::{
    write::{SaveObservationInput, WriteRequest},
    ProjectRegistry,
};

use crate::datasets::EvalMemory;

/// Ingest a slice of memories into a fresh registry rooted at `data_dir`.
///
/// Creates one project per unique `EvalMemory::project`. Starts a session for
/// each unique `(project, session_id)` pair, then bulk-loads observations in
/// batches of `batch_size`.
///
/// Returns the total number of observations written.
pub async fn ingest_memories(
    data_dir: &Path,
    memories: &[EvalMemory],
    batch_size: usize,
) -> Result<usize> {
    // Point the storage layer at our eval data dir.
    // SAFETY: we set this env var only during an eval run; the process is
    // single-purpose and not shared with a live daemon.
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);
    paths::ensure_dirs(data_dir).context("ensure eval data dirs")?;

    let registry = Arc::new(ProjectRegistry::new(
        /* lru_capacity */ 64,
        /* write_batch_max */ batch_size,
        /* write_batch_window */ Duration::from_millis(50),
    ));

    let mut total_written = 0usize;
    let mut batch_idx = 0usize;

    for chunk in memories.chunks(batch_size) {
        let mut pending_replies = Vec::with_capacity(chunk.len());

        for mem in chunk {
            let project = registry.get_or_open(&mem.project)
                .with_context(|| format!("open project '{}'", mem.project))?;

            // Ensure session exists (UpsertSession is idempotent).
            {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                project.write.send(WriteRequest::UpsertSession {
                    id: mem.session_id.clone(),
                    directory: data_dir.to_string_lossy().to_string(),
                    reply: reply_tx,
                }).context("send UpsertSession")?;
                reply_rx.await.context("await UpsertSession reply")??;
            }

            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            project.write.send(WriteRequest::SaveObservation {
                input: SaveObservationInput {
                    sync_id: Some(uuid::Uuid::new_v4().to_string()),
                    session_id: mem.session_id.clone(),
                    r#type: mem.obs_type.clone(),
                    title: mem.title.clone(),
                    content: mem.content.clone(),
                    tool_name: None,
                    scope: "global".to_string(),
                    created_by: Some("memlayer-eval".to_string()),
                    topic_key: mem.topic_key.clone(),
                    dedupe_window_secs: 0, // disable dedup in eval
                    max_content_chars: 8192,
                },
                reply: reply_tx,
            }).context("send SaveObservation")?;
            pending_replies.push(reply_rx);
        }

        // Drain replies for this batch.
        for rx in pending_replies {
            rx.await.context("await SaveObservation reply")??;
            total_written += 1;
        }

        batch_idx += 1;
        if batch_idx % 100 == 0 {
            debug!(batch = batch_idx, written = total_written, "ingest progress");
        }
    }

    info!(total = total_written, "ingest complete");
    Ok(total_written)
}

/// Stream-ingest a large BEAM-scale dataset without materialising the full
/// vector in memory. Accepts an iterator instead of a slice.
///
/// The iterator is consumed in `batch_size` chunks; each chunk is ingested
/// as a single write-thread batch.
pub async fn ingest_stream<I>(
    data_dir: &Path,
    memories: I,
    batch_size: usize,
) -> Result<usize>
where
    I: Iterator<Item = EvalMemory>,
{
    // Collect one batch at a time.
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
