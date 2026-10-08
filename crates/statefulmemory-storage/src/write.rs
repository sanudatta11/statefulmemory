//! Write path: SaveObservation logic + per-project write thread with batching.
//!
//! Spec sections covered:
//! - FR4 (entire write path)
//! - SC-4..7, SC-27 (round-trip, dedupe, idempotency, review schedule)
//! - EC-1, EC-2, EC-9, EC-11, EC-18 (validation + soft-delete handling + crash recovery)
//!
//! Concurrency model:
//! - The dedicated `std::thread` for a project owns the write `rusqlite::Connection`.
//! - tokio handlers send `WriteRequest` messages to the thread via `crossbeam-channel`
//!   bounded queues. Each request carries a oneshot reply channel.
//! - The thread drains up to `batch_max` messages OR up to `batch_window` time,
//!   then wraps them in a single transaction.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::oneshot;
use tracing::{debug, error};
use uuid::Uuid;

use statefulmemory_core::error::{Error, Result};
use statefulmemory_core::time as mtime;

use crate::dedupe;
use crate::models::Observation;

// ---------------------------------------------------------------------------
// Public API: WriteRequest + WriteHandle
// ---------------------------------------------------------------------------

/// One unit of work the write thread processes.
pub enum WriteRequest {
    SaveObservation {
        input: SaveObservationInput,
        reply: oneshot::Sender<Result<Observation>>,
    },
    UpsertSession {
        id: String,
        directory: String,
        reply: oneshot::Sender<Result<crate::models::Session>>,
    },
    EndSession {
        id: String,
        summary: Option<String>,
        reply: oneshot::Sender<Result<crate::models::Session>>,
    },
    SaveSessionSummary {
        id: String,
        summary: String,
        reply: oneshot::Sender<Result<crate::models::Session>>,
    },
    SavePrompt {
        sync_id: String,
        session_id: String,
        content: String,
        reply: oneshot::Sender<Result<crate::models::Prompt>>,
    },
    SoftDeleteObservation {
        key: ObservationKey,
        reply: oneshot::Sender<Result<()>>,
    },
    HardDeleteObservation {
        key: ObservationKey,
        reply: oneshot::Sender<Result<()>>,
    },
    UpdateObservation {
        key: ObservationKey,
        patch: ObservationPatch,
        reply: oneshot::Sender<Result<Observation>>,
    },
    DeletePrompt {
        key: PromptKey,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Insert (or replace) the dense embedding for an observation. Written
    /// asynchronously by the daemon's embed worker pool after the synchronous
    /// save commits — never on the hot save path. Spec: retrieval-promotion
    /// SC-2, P5.
    InsertEmbedding {
        obs_id: i64,
        embedding: Vec<f32>,
        model: String,
        /// When true the embed worker has already quantized the vector; the
        /// handler stores the int8 BLOB + scale in observation_embedding_meta
        /// and skips the observations_vec write.
        quantize: bool,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Insert chunk texts + their embeddings for one observation (Phase 2.5,
    /// opt-in via `embed.chunk_long_content`). Replaces any prior chunks for
    /// `obs_id` (re-embeds/re-chunks overwrite cleanly). Never on the hot
    /// save path — queued by the embed worker alongside InsertEmbedding.
    InsertChunks {
        obs_id: i64,
        /// `(chunk_text, embedding)` pairs, in chunk order.
        chunks: Vec<(String, Vec<f32>)>,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Insert a batch of atomic facts extracted from an observation. Written
    /// asynchronously by the extract worker (config-gated). Spec:
    /// retrieval-promotion SC-8, P6.
    InsertFacts {
        obs_id: i64,
        facts: Vec<NewFact>,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Index one observation into the cue-entity graph (`entities`,
    /// `entity_mentions`, `entity_edges`). Sent by the daemon after a save
    /// commits when `graph.enabled` is set for the project. Spec: graph-briefing.
    IndexGraph {
        observation_id: i64,
        title: String,
        content: String,
        anchors: Vec<String>,
        created_at_epoch: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Run an ad-hoc closure under the write connection. Used by sync import,
    /// project merge, and other multi-row operations.
    Custom {
        #[allow(clippy::type_complexity)]
        f: Box<dyn FnOnce(&mut Connection) -> Result<()> + Send>,
        reply: oneshot::Sender<Result<()>>,
    },
    EnqueueJob {
        job: crate::jobs::NewJob,
        reply: oneshot::Sender<Result<i64>>,
    },
    ClaimJob {
        kind: String,
        reply: oneshot::Sender<Result<Option<crate::jobs::Job>>>,
    },
    CompleteJob {
        id: i64,
        reply: oneshot::Sender<Result<bool>>,
    },
    FailJob {
        id: i64,
        error: String,
        retry_delay_secs: i64,
        reply: oneshot::Sender<Result<crate::jobs::JobStatus>>,
    },
}

/// Caller-supplied input for [`WriteRequest::InsertFacts`]. Mirrors
/// `crate::facts::FactInput` but uses owned strings so the request can
/// cross threads.
#[derive(Debug, Clone)]
pub struct NewFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub temporal: Option<String>,
    pub salience: Option<f64>,
    /// "haiku" | "sonnet".
    pub extracted_by: String,
}

#[derive(Debug, Clone)]
pub enum ObservationKey {
    Id(i64),
    SyncId(String),
}

#[derive(Debug, Clone)]
pub enum PromptKey {
    Id(i64),
    SyncId(String),
}

#[derive(Debug, Clone, Default)]
pub struct ObservationPatch {
    pub title: Option<String>,
    pub content: Option<String>,
    pub topic_key: Option<String>,
    pub scope: Option<String>,
    pub r#type: Option<String>,
    pub code_anchor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SaveObservationInput {
    pub sync_id: Option<String>,
    pub session_id: String,
    pub r#type: String,
    pub title: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub scope: String,
    pub created_by: Option<String>,
    pub topic_key: Option<String>,
    pub code_anchor: Option<String>,
    /// Dedupe window in seconds (0 disables hash dedupe, EC-11).
    pub dedupe_window_secs: u64,
    /// Maximum content length (EC-2).
    pub max_content_chars: usize,
    /// When true, insert without topic upsert or conflict supersession.
    /// Used by the staleness benchmark baseline (add-only memory arm).
    pub skip_supersede: bool,
}

/// Handle to a running write thread.
#[derive(Debug, Clone)]
pub struct WriteHandle {
    tx: Sender<WriteRequest>,
}

impl WriteHandle {
    pub fn send(&self, req: WriteRequest) -> Result<()> {
        self.tx
            .send(req)
            .map_err(|_| Error::Unavailable("write thread channel closed".into()))
    }
}

/// Spawn a per-project write thread.
///
/// The thread owns the write connection and runs until the channel is closed
/// (i.e., until all `WriteHandle` clones are dropped).
pub fn spawn_write_thread(
    project_id: String,
    db_path: PathBuf,
    batch_max: usize,
    batch_window: Duration,
    conflict_classifier: Option<std::sync::Arc<dyn crate::conflict_judge::ConflictClassifier>>,
) -> Result<WriteHandle> {
    let (tx, rx) = crossbeam_channel::bounded::<WriteRequest>(1024);
    let conn = crate::db::open_write(&db_path)?;
    let pid_clone = project_id.clone();
    thread::Builder::new()
        .name(format!("statefulmemory-write-{project_id}"))
        .spawn(move || {
            run_write_loop(
                pid_clone,
                conn,
                rx,
                batch_max,
                batch_window,
                conflict_classifier,
            );
        })
        .map_err(|e| Error::internal(format!("spawn write thread: {e}")))?;
    debug!(%project_id, "spawned write thread");
    Ok(WriteHandle { tx })
}

// ---------------------------------------------------------------------------
// Thread main loop
// ---------------------------------------------------------------------------

fn run_write_loop(
    project_id: String,
    mut conn: Connection,
    rx: Receiver<WriteRequest>,
    batch_max: usize,
    batch_window: Duration,
    conflict_classifier: Option<std::sync::Arc<dyn crate::conflict_judge::ConflictClassifier>>,
) {
    loop {
        // Block until first message.
        let first = match rx.recv() {
            Ok(m) => m,
            Err(_) => {
                debug!(%project_id, "write thread channel closed; exiting");
                return;
            }
        };

        // Collect more messages up to batch_max or batch_window.
        let mut batch: Vec<WriteRequest> = Vec::with_capacity(batch_max);
        batch.push(first);
        let deadline = Instant::now() + batch_window;
        while batch.len() < batch_max {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }

        // Run the batch in one transaction. On panic, isolate via `catch_unwind`
        // so the channel stays alive (EH-2 / EC-18: write-thread crash recovery
        // is handled at the registry layer; here we just ensure we don't kill
        // the thread on a single bad message).
        if let Err(e) = process_batch(&mut conn, &mut batch, conflict_classifier.as_deref()) {
            error!(%project_id, error=%e, "batch failed");
        }
    }
}

fn process_batch(
    conn: &mut Connection,
    batch: &mut Vec<WriteRequest>,
    conflict_classifier: Option<&dyn crate::conflict_judge::ConflictClassifier>,
) -> Result<()> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| Error::internal(format!("BEGIN IMMEDIATE: {e}")))?;

    let mut replies: Vec<Box<dyn FnOnce() + Send>> = Vec::with_capacity(batch.len());

    for req in batch.drain(..) {
        match req {
            WriteRequest::SaveObservation { input, reply } => {
                let result = handle_save_observation(&tx, input, conflict_classifier);
                replies.push(Box::new(move || {
                    let _ = reply.send(result);
                }));
            }
            WriteRequest::UpsertSession {
                id,
                directory,
                reply,
            } => {
                let r = handle_upsert_session(&tx, &id, &directory);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::EndSession { id, summary, reply } => {
                let r = handle_end_session(&tx, &id, summary.as_deref());
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::SaveSessionSummary { id, summary, reply } => {
                let r = handle_save_session_summary(&tx, &id, &summary);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::SavePrompt {
                sync_id,
                session_id,
                content,
                reply,
            } => {
                let r = handle_save_prompt(&tx, &sync_id, &session_id, &content);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::SoftDeleteObservation { key, reply } => {
                let r = handle_soft_delete_obs(&tx, &key);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::HardDeleteObservation { key, reply } => {
                let r = handle_hard_delete_obs(&tx, &key);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::UpdateObservation { key, patch, reply } => {
                let r = handle_update_obs(&tx, &key, &patch);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::DeletePrompt { key, reply } => {
                let r = handle_delete_prompt(&tx, &key);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::InsertEmbedding {
                obs_id,
                embedding,
                model,
                quantize,
                reply,
            } => {
                let r = handle_insert_embedding(&tx, obs_id, &embedding, &model, quantize);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::InsertChunks {
                obs_id,
                chunks,
                reply,
            } => {
                let r = handle_insert_chunks(&tx, obs_id, &chunks);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::InsertFacts {
                obs_id,
                facts,
                reply,
            } => {
                let r = handle_insert_facts(&tx, obs_id, &facts);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::IndexGraph {
                observation_id,
                title,
                content,
                anchors,
                created_at_epoch,
                reply,
            } => {
                let r = handle_index_graph(
                    &tx,
                    observation_id,
                    &title,
                    &content,
                    &anchors,
                    created_at_epoch,
                );
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::EnqueueJob { job, reply } => {
                let r = crate::jobs::enqueue(&tx, &job);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::ClaimJob { kind, reply } => {
                let r = crate::jobs::claim(&tx, &kind);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::CompleteJob { id, reply } => {
                let r = crate::jobs::complete(&tx, id);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::FailJob {
                id,
                error,
                retry_delay_secs,
                reply,
            } => {
                let r = crate::jobs::fail(&tx, id, &error, retry_delay_secs);
                replies.push(Box::new(move || {
                    let _ = reply.send(r);
                }));
            }
            WriteRequest::Custom { f, reply } => {
                // Custom needs a `&mut Connection` but we hold a Transaction.
                // We commit the in-flight tx, then run the closure against the
                // bare connection. Critically, the prior requests' replies must
                // fire BEFORE we return — those writes have already committed
                // and their callers are blocked on `oneshot::Receiver`. Dropping
                // the reply closures (the original behavior) made the callers
                // see RecvError and surfaced as "write thread crashed" even
                // though the writes succeeded.
                tx.commit()
                    .map_err(|e| Error::internal(format!("COMMIT for custom: {e}")))?;
                for cb in replies.drain(..) {
                    cb();
                }
                let r = (f)(conn);
                let _ = reply.send(r);
                return Ok(());
            }
        }
    }

    tx.commit()
        .map_err(|e| Error::internal(format!("COMMIT: {e}")))?;
    for cb in replies {
        cb();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SaveObservation handler
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn handle_save_observation_for_tests(
    tx: &rusqlite::Transaction<'_>,
    input: SaveObservationInput,
) -> Result<Observation> {
    handle_save_observation(tx, input, None)
}

fn handle_save_observation(
    tx: &rusqlite::Transaction<'_>,
    input: SaveObservationInput,
    conflict_classifier: Option<&dyn crate::conflict_judge::ConflictClassifier>,
) -> Result<Observation> {
    // Validate.
    if input.content.trim().is_empty() {
        return Err(Error::invalid("content is empty"));
    }
    if input.content.chars().count() > input.max_content_chars {
        return Err(Error::invalid(format!(
            "content exceeds maximum of {} chars",
            input.max_content_chars
        )));
    }
    match input.scope.as_str() {
        "project" | "personal" | "team" => {}
        _ => return Err(Error::invalid(format!("invalid scope: {}", input.scope))),
    }

    // 1. sync_id idempotency: if caller-supplied sync_id is already present,
    //    return that row unchanged. (SC-7, EC-12)
    if let Some(sync_id) = &input.sync_id {
        if let Some(existing) = fetch_observation_by_sync_id(tx, sync_id)? {
            return Ok(existing);
        }
    }

    let now = mtime::now_rfc3339();
    let normalized_hash = dedupe::hash(&input.content);
    let review_after = dedupe::review_months_for_type(&input.r#type).map(mtime::months_from_now);

    // 2. Topic-key upsert: if (topic_key, scope) match a non-deleted row,
    //    update it in place and return. (SC-5, EC-9)
    if !input.skip_supersede {
        if let Some(topic_key) = &input.topic_key {
            if let Some(existing) = fetch_active_obs_by_topic(tx, topic_key, &input.scope)? {
                tx.execute(
                    "UPDATE observations
                    SET title = ?2,
                        content = ?3,
                        normalized_hash = ?4,
                        revision_count = revision_count + 1,
                        updated_at = ?5,
                        last_seen_at = ?5,
                         review_after = COALESCE(?6, review_after),
                         code_anchor = COALESCE(?7, code_anchor),
                         exported_at = NULL
                   WHERE id = ?1",
                    params![
                        existing.id,
                        &input.title,
                        &input.content,
                        &normalized_hash,
                        &now,
                        &review_after,
                        &input.code_anchor,
                    ],
                )
                .map_err(|e| Error::internal(format!("topic upsert update: {e}")))?;
                invalidate_observation_projections(tx, existing.id)?;
                return fetch_observation_by_id(tx, existing.id)?
                    .ok_or_else(|| Error::internal("topic upsert: row vanished"));
            }
        }
    }

    // 3. Normalized-hash dedupe: if a non-deleted row exists with the same
    //    hash inside the dedupe window, bump duplicate_count. (SC-6, EC-11)
    if input.dedupe_window_secs > 0 {
        if let Some(existing) =
            fetch_active_obs_by_hash_in_window(tx, &normalized_hash, input.dedupe_window_secs)?
        {
            tx.execute(
                "UPDATE observations
                        SET duplicate_count = duplicate_count + 1,
                            last_seen_at = ?2,
                            exported_at = NULL
                      WHERE id = ?1",
                params![existing.id, &now],
            )
            .map_err(|e| Error::internal(format!("hash-dedupe update: {e}")))?;
            return fetch_observation_by_id(tx, existing.id)?
                .ok_or_else(|| Error::internal("hash-dedupe: row vanished"));
        }
    }

    // 4. Insert.
    // Ensure the session row exists before inserting the observation — the
    // FOREIGN KEY requires it. CLI callers pass `--session $(uuidgen)` which
    // is a valid new session id; auto-creating it here is correct and
    // idempotent (handle_upsert_session returns early if it already exists).
    handle_upsert_session(tx, &input.session_id, "")
        .map_err(|e| Error::internal(format!("auto-upsert session: {e}")))?;

    let sync_id = input
        .sync_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    tx.execute(
        "INSERT INTO observations
            (sync_id, session_id, type, title, content, tool_name, scope,
             created_by, topic_key, code_anchor, normalized_hash, last_seen_at,
             created_at, updated_at, review_after)
         VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
        params![
            &sync_id,
            &input.session_id,
            &input.r#type,
            &input.title,
            &input.content,
            &input.tool_name,
            &input.scope,
            &input.created_by,
            &input.topic_key,
            &input.code_anchor,
            &normalized_hash,
            &now,
            &now,
            &review_after,
        ],
    )
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            Error::AlreadyExists("observation sync_id collision".into())
        } else {
            Error::internal(format!("insert observation: {e}"))
        }
    })?;
    let id = tx.last_insert_rowid();

    // 5. Conflict detection: find existing active observations of the same
    //    type+scope that likely describe the same fact with a conflicting value.
    //    Uses FTS5 BM25 on the title — top-1 hit within same type+scope.
    //    If found, optionally consult the LLM conflict judge before deciding
    //    whether to supersede. On judge error/timeout fall back to the BM25
    //    heuristic (unconditional supersession).
    let superseded_id = if input.skip_supersede {
        None
    } else {
        find_conflict_candidate(tx, id, &input)?
    };
    let do_supersede = if let Some(old_id) = superseded_id {
        should_supersede(old_id, id, tx, &input, conflict_classifier)
    } else {
        false
    };
    if do_supersede {
        let old_id = superseded_id.unwrap();
        tx.execute(
            "UPDATE observations
                SET deleted_at = ?2,
                    delete_reason = 'superseded',
                    superseded_by_id = ?3,
                    exported_at = NULL
              WHERE id = ?1",
            params![old_id, &now, id],
        )
        .map_err(|e| Error::internal(format!("supersede old obs: {e}")))?;
        invalidate_observation_projections(tx, old_id)?;
        tx.execute(
            "UPDATE observations SET superseded_count = superseded_count + 1 WHERE id = ?1",
            params![id],
        )
        .map_err(|e| Error::internal(format!("bump superseded_count: {e}")))?;
    }

    let mut obs = fetch_observation_by_id(tx, id)?
        .ok_or_else(|| Error::internal("post-insert fetch: row vanished"))?;
    if do_supersede {
        obs.superseded_ids = vec![superseded_id.unwrap()];
    }
    Ok(obs)
}

// ---------------------------------------------------------------------------
// Other write handlers (sessions, prompts, deletes, updates)
// ---------------------------------------------------------------------------

fn handle_upsert_session(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    directory: &str,
) -> Result<crate::models::Session> {
    // FR6.1: idempotent on id.
    let existing: Option<crate::models::Session> = tx
        .query_row(
            "SELECT id, directory, started_at, ended_at, summary FROM sessions WHERE id = ?1",
            params![id],
            crate::models::Session::from_row,
        )
        .ok();
    if let Some(s) = existing {
        return Ok(s);
    }
    tx.execute(
        "INSERT INTO sessions (id, directory) VALUES (?1, ?2)",
        params![id, directory],
    )
    .map_err(|e| Error::internal(format!("insert session: {e}")))?;
    tx.query_row(
        "SELECT id, directory, started_at, ended_at, summary FROM sessions WHERE id = ?1",
        params![id],
        crate::models::Session::from_row,
    )
    .map_err(|e| Error::internal(format!("post-insert session fetch: {e}")))
}

fn handle_end_session(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    summary: Option<&str>,
) -> Result<crate::models::Session> {
    let now = mtime::now_rfc3339();
    let n = tx
        .execute(
            "UPDATE sessions          SET ended_at = ?2, summary = COALESCE(?3, summary), exported_at = NULL WHERE id = ?1",

            params![id, now, summary],
        )
        .map_err(|e| Error::internal(format!("update session: {e}")))?;
    if n == 0 {
        return Err(Error::not_found(format!("session {id}")));
    }
    tx.query_row(
        "SELECT id, directory, started_at, ended_at, summary FROM sessions WHERE id = ?1",
        params![id],
        crate::models::Session::from_row,
    )
    .map_err(|e| Error::internal(format!("post-end session fetch: {e}")))
}

fn handle_save_session_summary(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    summary: &str,
) -> Result<crate::models::Session> {
    let n = tx
        .execute(
            "UPDATE sessions          SET summary = ?2, exported_at = NULL WHERE id = ?1",
            params![id, summary],
        )
        .map_err(|e| Error::internal(format!("update session summary: {e}")))?;
    if n == 0 {
        return Err(Error::not_found(format!("session {id}")));
    }
    tx.query_row(
        "SELECT id, directory, started_at, ended_at, summary FROM sessions WHERE id = ?1",
        params![id],
        crate::models::Session::from_row,
    )
    .map_err(|e| Error::internal(format!("post-summary session fetch: {e}")))
}

fn handle_save_prompt(
    tx: &rusqlite::Transaction<'_>,
    sync_id: &str,
    session_id: &str,
    content: &str,
) -> Result<crate::models::Prompt> {
    if content.trim().is_empty() {
        return Err(Error::invalid("prompt content is empty"));
    }
    // Idempotency: existing sync_id returns the same row.
    if let Ok(p) = tx.query_row(
        "SELECT id, sync_id, session_id, content, created_at FROM user_prompts WHERE sync_id = ?1",
        params![sync_id],
        crate::models::Prompt::from_row,
    ) {
        return Ok(p);
    }
    tx.execute(
        "INSERT INTO user_prompts (sync_id, session_id, content) VALUES (?1, ?2, ?3)",
        params![sync_id, session_id, content],
    )
    .map_err(|e| Error::internal(format!("insert prompt: {e}")))?;
    tx.query_row(
        "SELECT id, sync_id, session_id, content, created_at FROM user_prompts WHERE sync_id = ?1",
        params![sync_id],
        crate::models::Prompt::from_row,
    )
    .map_err(|e| Error::internal(format!("post-insert prompt fetch: {e}")))
}

pub fn invalidate_observation_projections(
    conn: &rusqlite::Connection,
    observation_id: i64,
) -> Result<()> {
    invalidate_observation_content(conn, observation_id)?;
    crate::graph::remove_observation(conn, observation_id)
}

pub fn invalidate_observation_content(
    conn: &rusqlite::Connection,
    observation_id: i64,
) -> Result<()> {
    conn.execute(
        "DELETE FROM observations_vec WHERE rowid = ?1",
        params![observation_id],
    )
    .map_err(|e| Error::internal(format!("invalidate embedding vector: {e}")))?;
    conn.execute(
        "DELETE FROM observation_embedding_meta WHERE observation_id = ?1",
        params![observation_id],
    )
    .map_err(|e| Error::internal(format!("invalidate embedding meta: {e}")))?;
    conn.execute(
        "DELETE FROM facts WHERE obs_id = ?1",
        params![observation_id],
    )
    .map_err(|e| Error::internal(format!("invalidate facts: {e}")))?;
    conn.execute(
        "UPDATE observations
            SET key_expand = '',
                verify_state = 'unanchored',
                verified_commit = NULL,
                verified_at = NULL
          WHERE id = ?1",
        params![observation_id],
    )
    .map_err(|e| Error::internal(format!("invalidate observation projections: {e}")))?;
    conn.execute(
        "DELETE FROM observation_anchors WHERE observation_id = ?1",
        params![observation_id],
    )
    .map_err(|e| Error::internal(format!("invalidate observation anchors: {e}")))?;
    Ok(())
}

fn observation_id_for_key(tx: &rusqlite::Transaction<'_>, key: &ObservationKey) -> Result<i64> {
    match key {
        ObservationKey::Id(id) => Ok(*id),
        ObservationKey::SyncId(sync_id) => tx
            .query_row(
                "SELECT id FROM observations WHERE sync_id = ?1",
                params![sync_id],
                |row| row.get(0),
            )
            .map_err(|e| Error::not_found(format!("observation sync_id '{sync_id}': {e}"))),
    }
}

fn handle_soft_delete_obs(tx: &rusqlite::Transaction<'_>, key: &ObservationKey) -> Result<()> {
    let observation_id = observation_id_for_key(tx, key)?;
    let now = mtime::now_rfc3339();
    let n = match key {
        ObservationKey::Id(id) => tx.execute(
            "UPDATE observations
                SET deleted_at = ?2, exported_at = NULL
              WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now],
        ),
        ObservationKey::SyncId(s) => tx.execute(
            "UPDATE observations
                SET deleted_at = ?2, exported_at = NULL
              WHERE sync_id = ?1 AND deleted_at IS NULL",
            params![s, now],
        ),
    }
    .map_err(|e| Error::internal(format!("soft-delete: {e}")))?;
    if n == 0 {
        return Err(Error::not_found("observation"));
    }
    invalidate_observation_projections(tx, observation_id)
}

fn handle_hard_delete_obs(tx: &rusqlite::Transaction<'_>, key: &ObservationKey) -> Result<()> {
    let observation_id = observation_id_for_key(tx, key)?;
    invalidate_observation_projections(tx, observation_id)?;
    let n = match key {
        ObservationKey::Id(id) => tx.execute("DELETE FROM observations WHERE id = ?1", params![id]),
        ObservationKey::SyncId(s) => {
            tx.execute("DELETE FROM observations WHERE sync_id = ?1", params![s])
        }
    }
    .map_err(|e| Error::internal(format!("hard-delete: {e}")))?;
    if n == 0 {
        return Err(Error::not_found("observation"));
    }
    Ok(())
}

fn handle_update_obs(
    tx: &rusqlite::Transaction<'_>,
    key: &ObservationKey,
    patch: &ObservationPatch,
) -> Result<Observation> {
    let existing = fetch_obs_by_key(tx, key)?.ok_or_else(|| Error::not_found("observation"))?;
    if existing.deleted_at.is_some() {
        return Err(Error::FailedPrecondition(
            "cannot update soft-deleted observation".into(),
        ));
    }
    let now = mtime::now_rfc3339();
    let title = patch.title.as_deref().unwrap_or(&existing.title);
    let content = patch.content.as_deref().unwrap_or(&existing.content);
    let topic_key = patch.topic_key.as_deref().or(existing.topic_key.as_deref());
    let scope = patch.scope.as_deref().unwrap_or(&existing.scope);
    let r#type = patch.r#type.as_deref().unwrap_or(&existing.r#type);
    let code_anchor = patch
        .code_anchor
        .as_deref()
        .or(existing.code_anchor.as_deref());
    let normalized_hash = dedupe::hash(content);
    tx.execute(
        "UPDATE observations
            SET title = ?2,
                content = ?3,
                topic_key = ?4,
                scope = ?5,
                type = ?6,
                normalized_hash = ?7,
                 code_anchor = ?8,
                 revision_count = revision_count + 1,
                 updated_at = ?9,
                 exported_at = NULL
            WHERE id = ?1",
        params![
            existing.id,
            title,
            content,
            topic_key,
            scope,
            r#type,
            &normalized_hash,
            code_anchor,
            &now,
        ],
    )
    .map_err(|e| Error::internal(format!("update: {e}")))?;
    invalidate_observation_projections(tx, existing.id)?;
    fetch_observation_by_id(tx, existing.id)?
        .ok_or_else(|| Error::internal("post-update fetch: row vanished"))
}

fn handle_delete_prompt(tx: &rusqlite::Transaction<'_>, key: &PromptKey) -> Result<()> {
    let n = match key {
        PromptKey::Id(id) => tx.execute("DELETE FROM user_prompts WHERE id = ?1", params![id]),
        PromptKey::SyncId(s) => {
            tx.execute("DELETE FROM user_prompts WHERE sync_id = ?1", params![s])
        }
    }
    .map_err(|e| Error::internal(format!("delete prompt: {e}")))?;
    if n == 0 {
        return Err(Error::not_found("prompt"));
    }
    Ok(())
}

/// Insert (or replace) one observation's dense embedding. The vec0 vtable
/// expects a contiguous f32 little-endian byte blob; we slice
/// `embedding.as_bytes()`-equivalent into a Vec<u8> here.
/// Supported embedding dimensions and their vec0 table names (Phase 6.3).
/// 384 = bge-small (V4/V14, the default); 768 = a bge-base/m3-class upgrade
/// tier (V16). Dimension is read from the vector's actual length — the
/// caller's configured model determines which dimension it produces, and
/// `pragmas::check_embed_model_compat` already refuses to start a project
/// against a model whose name doesn't match what's stored, so a project
/// never has rows in both dimensions' tables at once.
fn vec_table_names(dim: usize) -> Result<(&'static str, &'static str)> {
    match dim {
        384 => Ok(("observations_vec", "observations_vec_i8")),
        768 => Ok(("observations_vec_768", "observations_vec_768_i8")),
        other => Err(Error::invalid(format!(
            "unsupported embedding dimension {other} (supported: 384, 768)"
        ))),
    }
}

fn handle_insert_embedding(
    tx: &rusqlite::Transaction<'_>,
    obs_id: i64,
    embedding: &[f32],
    model: &str,
    quantize: bool,
) -> Result<()> {
    let dim = embedding.len();
    let (vec_table, vec_i8_table) = vec_table_names(dim)?;
    if quantize {
        // Store int8 representation (shared helper: same math feeds both the
        // legacy blob fallback and the ANN index below).
        let scale = statefulmemory_embed::quantize::calibrate_scale(&[embedding]);
        let int8: Vec<i8> = statefulmemory_embed::quantize::f32_to_int8(embedding, scale);
        // Store raw bytes: i8 → u8 reinterpret (safe, same bit pattern).
        let blob: Vec<u8> = int8.iter().map(|&x| x as u8).collect();
        tx.execute(
            "INSERT OR REPLACE INTO observation_embedding_meta \
                (observation_id, model, dim, created_at, quantized, quantized_blob, scale) \
             VALUES (?1, ?2, ?3, datetime('now'), 1, ?4, ?5)",
            params![obs_id, model, dim as i64, blob.clone(), scale],
        )
        .map_err(|e| Error::internal(format!("insert quantized meta: {e}")))?;

        // ANN index (Phase 6.2): also write into the native int8 vec0 column
        // (distance_metric=cosine, scale-invariant per vector — see V14) so
        // dense search can use the index instead of a brute-force Rust scan.
        // `vec_int8(?)` wraps the blob with the SQLITE_SUBTYPE sqlite-vec
        // needs to parse it as int8 rather than the float32 default; a bare
        // blob param has no subtype and would be rejected / misread.
        // Best-effort: V14/V16 may not have run yet on a DB opened by an
        // older binary mid-rollout — log and continue rather than failing.
        if let Err(e) = tx.execute(
            &format!(
                "INSERT OR REPLACE INTO {vec_i8_table}(rowid, embedding) VALUES (?1, vec_int8(?2))"
            ),
            params![obs_id, blob],
        ) {
            tracing::debug!(obs_id, dim, error = %e, "ANN int8 index insert failed (ANN index unavailable, legacy blob still written)");
        }
        return Ok(());
    }
    let mut blob = Vec::with_capacity(embedding.len() * 4);
    for f in embedding {
        blob.extend_from_slice(&f.to_le_bytes());
    }
    tx.execute(
        &format!("INSERT OR REPLACE INTO {vec_table}(rowid, embedding) VALUES (?1, ?2)"),
        params![obs_id, blob],
    )
    .map_err(|e| Error::internal(format!("insert {vec_table}: {e}")))?;
    tx.execute(
        "INSERT OR REPLACE INTO observation_embedding_meta \
            (observation_id, model, dim, created_at) \
         VALUES (?1, ?2, ?3, datetime('now'))",
        params![obs_id, model, dim as i64],
    )
    .map_err(|e| Error::internal(format!("insert observation_embedding_meta: {e}")))?;
    Ok(())
}

/// Replace an observation's chunk vectors (Phase 2.5, opt-in). Deletes any
/// prior chunks for `obs_id` first (cascades to `chunk_vectors` via
/// the V15 trigger), then inserts the new set. Empty `chunks` just clears —
/// used when a re-embedded observation no longer needs chunking (shrunk below
/// the threshold).
fn handle_insert_chunks(
    tx: &rusqlite::Transaction<'_>,
    obs_id: i64,
    chunks: &[(String, Vec<f32>)],
) -> Result<()> {
    tx.execute(
        "DELETE FROM observations_chunks WHERE obs_id = ?1",
        params![obs_id],
    )
    .map_err(|e| Error::internal(format!("delete prior chunks: {e}")))?;

    for (idx, (text, embedding)) in chunks.iter().enumerate() {
        if embedding.len() != 384 {
            return Err(Error::invalid(format!(
                "chunk embedding dim mismatch: got {}, expected 384",
                embedding.len()
            )));
        }
        tx.execute(
            "INSERT INTO observations_chunks (obs_id, chunk_idx, text) VALUES (?1, ?2, ?3)",
            params![obs_id, idx as i64, text],
        )
        .map_err(|e| Error::internal(format!("insert observations_chunks: {e}")))?;
        let chunk_id = tx.last_insert_rowid();

        let mut blob = Vec::with_capacity(embedding.len() * 4);
        for f in embedding {
            blob.extend_from_slice(&f.to_le_bytes());
        }
        tx.execute(
            "INSERT OR REPLACE INTO chunk_vectors(rowid, embedding) VALUES (?1, ?2)",
            params![chunk_id, blob],
        )
        .map_err(|e| Error::internal(format!("insert chunk_vectors: {e}")))?;
    }
    Ok(())
}

/// Insert a batch of facts under one transaction. Empty `facts` is a no-op
/// success — callers don't need to special-case "extractor returned nothing".
fn handle_insert_facts(
    tx: &rusqlite::Transaction<'_>,
    obs_id: i64,
    facts: &[NewFact],
) -> Result<()> {
    if facts.is_empty() {
        return Ok(());
    }
    let mut stmt = tx
        .prepare(
            "INSERT INTO facts (obs_id, subject, predicate, object, temporal, salience, extracted_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .map_err(|e| Error::internal(format!("prepare insert_fact: {e}")))?;
    // Fact-level supersession (Phase 3.2): a newly-inserted fact with the same
    // (subject, predicate) but a DIFFERENT object supersedes prior active facts
    // across the project, so the stale value stops surfacing via the fact lane
    // (search_facts filters superseded_by IS NULL). Same-object re-extractions
    // are left alone (not a contradiction). Case-insensitive, trimmed match.
    let mut supersede_stmt = tx
        .prepare(
            "UPDATE facts SET superseded_by = ?1 \
             WHERE id != ?1 AND superseded_by IS NULL \
               AND lower(trim(subject)) = lower(trim(?2)) \
               AND lower(trim(predicate)) = lower(trim(?3)) \
               AND lower(trim(object)) != lower(trim(?4))",
        )
        .map_err(|e| Error::internal(format!("prepare supersede_fact: {e}")))?;
    let mut expand_parts: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for f in facts {
        let salience = f.salience.unwrap_or(0.5);
        stmt.execute(params![
            obs_id,
            f.subject,
            f.predicate,
            f.object,
            f.temporal,
            salience,
            f.extracted_by,
        ])
        .map_err(|e| Error::internal(format!("insert fact: {e}")))?;
        let new_id = tx.last_insert_rowid();
        // Keep the heterogeneous graph projection in the same write
        // transaction as the authoritative fact. This makes fact chains
        // immediately explainable even when async extraction finishes later.
        let fact_node = crate::memory_graph::upsert_node(
            tx,
            "fact",
            &format!("fact:{new_id}"),
            &format!("{} {} {}", f.subject, f.predicate, f.object),
            &format!(
                "{}|{}|{}",
                statefulmemory_core::config::normalize_entity_name(&f.subject),
                statefulmemory_core::config::normalize_entity_name(&f.predicate),
                statefulmemory_core::config::normalize_entity_name(&f.object)
            ),
            None,
            salience,
            "llm",
        )?;
        let observation_node = crate::memory_graph::upsert_node(
            tx,
            "observation",
            &format!("observation:{obs_id}"),
            &format!("observation:{obs_id}"),
            &format!("observation:{obs_id}"),
            None,
            1.0,
            "deterministic",
        )?;
        crate::memory_graph::insert_edge(
            tx,
            fact_node,
            observation_node,
            "derived_from",
            1.0,
            salience,
            f.temporal.as_deref(),
            None,
            "llm",
            Some(obs_id),
            None,
            None,
        )?;
        for (relation, value) in [
            ("subject", f.subject.as_str()),
            ("object", f.object.as_str()),
        ] {
            let norm = statefulmemory_core::config::normalize_entity_name(value);
            if norm.is_empty() {
                continue;
            }
            let existing_entity_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM entities WHERE norm_name=?1",
                    [&norm],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| Error::internal(format!("lookup fact entity: {e}")))?;
            let entity_id = match existing_entity_id {
                Some(id) => id,
                None => crate::graph::upsert_entity(
                    tx,
                    "concept",
                    value,
                    chrono::Utc::now().timestamp(),
                )?,
            };
            let entity_node = crate::memory_graph::upsert_node(
                tx,
                "entity",
                &format!("entity:{entity_id}"),
                value,
                &norm,
                None,
                salience,
                "llm",
            )?;
            crate::memory_graph::insert_edge(
                tx,
                fact_node,
                entity_node,
                relation,
                1.0,
                salience,
                f.temporal.as_deref(),
                None,
                "llm",
                Some(obs_id),
                None,
                None,
            )?;
        }
        supersede_stmt
            .execute(params![new_id, f.subject, f.predicate, f.object])
            .map_err(|e| Error::internal(format!("supersede prior facts: {e}")))?;
        for term in [f.subject.as_str(), f.object.as_str()] {
            let t = term.trim();
            if t.len() < 2 {
                continue;
            }
            let key = t.to_ascii_lowercase();
            if seen.insert(key) {
                expand_parts.push(t.to_string());
            }
        }
    }
    if !expand_parts.is_empty() {
        let expand = expand_parts.join(" ");
        // Merge into key_expand (LongMemEval index-time merge). Trigger
        // refreshes observations_fts including the new column.
        tx.execute(
            "UPDATE observations SET key_expand = trim(COALESCE(key_expand, '') || ' ' || ?1) \
             WHERE id = ?2",
            params![expand, obs_id],
        )
        .map_err(|e| Error::internal(format!("update key_expand: {e}")))?;
    }
    Ok(())
}

/// Build the graph layer for one observation: upsert entities mined
/// deterministically from title/content/anchors, wire mentions, then link the
/// observation's entities pairwise with the `mentions` relation. Pair-edge
/// rules are fixed by the graph-briefing spec: undirected co-mention is
/// recorded directionally (first-seen order) at weight 1.0 with
/// `src_observation_id` set for provenance.
fn handle_index_graph(
    tx: &rusqlite::Transaction<'_>,
    observation_id: i64,
    title: &str,
    content: &str,
    anchors: &[String],
    created_at_epoch: i64,
) -> Result<()> {
    use statefulmemory_core::config::MentionSource;

    crate::memory_graph::remove_observation_projection(tx, observation_id)?;
    let observation_node = crate::memory_graph::upsert_node(
        tx,
        "observation",
        &format!("observation:{observation_id}"),
        title,
        &statefulmemory_core::config::normalize_entity_name(title),
        None,
        1.0,
        "deterministic",
    )?;
    let session_id: Option<String> = tx
        .query_row(
            "SELECT session_id FROM observations WHERE id=?1",
            [observation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| Error::internal(format!("lookup graph observation session: {e}")))?;
    if let Some(session_id) = session_id {
        let session_node = crate::memory_graph::upsert_node(
            tx,
            "session",
            &format!("session:{session_id}"),
            &session_id,
            &statefulmemory_core::config::normalize_entity_name(&session_id),
            None,
            1.0,
            "deterministic",
        )?;
        crate::memory_graph::insert_edge(
            tx,
            session_node,
            observation_node,
            "contains",
            1.0,
            1.0,
            None,
            None,
            "deterministic",
            Some(observation_id),
            None,
            None,
        )?;
    }
    let entities =
        statefulmemory_extract::entity_resolve::extract_entities(title, content, anchors);
    if entities.is_empty() {
        return Ok(());
    }
    let mut entity_ids: Vec<i64> = Vec::with_capacity(entities.len());
    for e in &entities {
        let id = crate::graph::upsert_entity(tx, e.kind.as_str(), &e.name, created_at_epoch)?;
        let graph_entity = crate::memory_graph::upsert_node(
            tx,
            "entity",
            &format!("entity:{id}"),
            &e.name,
            &statefulmemory_core::config::normalize_entity_name(&e.name),
            None,
            1.0,
            "deterministic",
        )?;
        let offsets = match e.source {
            MentionSource::Backtick | MentionSource::Token => {
                Some(format!("[{},{}]", e.offsets.0, e.offsets.1))
            }
            _ => None,
        };
        crate::graph::insert_mention(
            tx,
            id,
            observation_id,
            offsets.as_deref(),
            e.source.as_str(),
        )?;
        entity_ids.push(id);
        crate::memory_graph::insert_edge(
            tx,
            observation_node,
            graph_entity,
            "mentions",
            1.0,
            1.0,
            None,
            None,
            "deterministic",
            Some(observation_id),
            (e.offsets != statefulmemory_extract::entity_resolve::NOT_IN_TEXT)
                .then(|| format!("[{},{}]", e.offsets.0, e.offsets.1))
                .as_deref(),
            None,
        )?;
    }
    for i in 0..entity_ids.len() {
        for j in (i + 1)..entity_ids.len() {
            crate::graph::insert_edge(
                tx,
                entity_ids[i],
                entity_ids[j],
                "mentions",
                1.0,
                created_at_epoch,
                created_at_epoch,
                Some(observation_id),
            )?;
            let from = crate::memory_graph::node_by_ref(
                tx,
                "entity",
                &format!("entity:{}", entity_ids[i]),
            )?
            .map(|n| n.id);
            let to = crate::memory_graph::node_by_ref(
                tx,
                "entity",
                &format!("entity:{}", entity_ids[j]),
            )?
            .map(|n| n.id);
            if let (Some(from), Some(to)) = (from, to) {
                crate::memory_graph::insert_edge(
                    tx,
                    from,
                    to,
                    "co_occurs",
                    1.0,
                    1.0,
                    None,
                    None,
                    "deterministic",
                    Some(observation_id),
                    None,
                    None,
                )?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Lookup helpers (used by both write and read paths in this transaction)
// ---------------------------------------------------------------------------

fn fetch_observation_by_id(tx: &rusqlite::Transaction<'_>, id: i64) -> Result<Option<Observation>> {
    let mut stmt = tx
        .prepare(OBSERVATION_SELECT_FIELDS_BY_ID)
        .map_err(|e| Error::internal(format!("prepare: {e}")))?;
    let row = stmt.query_row(params![id], Observation::from_row).ok();
    Ok(row)
}

fn fetch_observation_by_sync_id(
    tx: &rusqlite::Transaction<'_>,
    sync_id: &str,
) -> Result<Option<Observation>> {
    let mut stmt = tx
        .prepare(OBSERVATION_SELECT_FIELDS_BY_SYNC)
        .map_err(|e| Error::internal(format!("prepare: {e}")))?;
    let row = stmt.query_row(params![sync_id], Observation::from_row).ok();
    Ok(row)
}

fn fetch_active_obs_by_topic(
    tx: &rusqlite::Transaction<'_>,
    topic_key: &str,
    scope: &str,
) -> Result<Option<Observation>> {
    let mut stmt = tx
        .prepare(
            "SELECT id, sync_id, session_id, type, title, content, tool_name, scope,
                    created_by, topic_key, normalized_hash, revision_count, duplicate_count,
                    last_seen_at, created_at, updated_at, deleted_at, review_after
             FROM observations
             WHERE topic_key = ?1 AND scope = ?2 AND deleted_at IS NULL
             ORDER BY updated_at DESC LIMIT 1",
        )
        .map_err(|e| Error::internal(format!("prepare topic select: {e}")))?;
    let row = stmt
        .query_row(params![topic_key, scope], Observation::from_row)
        .ok();
    Ok(row)
}

fn fetch_active_obs_by_hash_in_window(
    tx: &rusqlite::Transaction<'_>,
    hash: &str,
    window_secs: u64,
) -> Result<Option<Observation>> {
    // window_secs converted to seconds-string for SQLite datetime arithmetic.
    let mut stmt = tx
        .prepare(
            "SELECT id, sync_id, session_id, type, title, content, tool_name, scope,
                    created_by, topic_key, normalized_hash, revision_count, duplicate_count,
                    last_seen_at, created_at, updated_at, deleted_at, review_after
             FROM observations
             WHERE normalized_hash = ?1
               AND deleted_at IS NULL
               AND created_at >= datetime('now', ?2)
             ORDER BY created_at DESC LIMIT 1",
        )
        .map_err(|e| Error::internal(format!("prepare hash select: {e}")))?;
    let modifier = format!("-{window_secs} seconds");
    let row = stmt
        .query_row(params![hash, modifier], Observation::from_row)
        .ok();
    Ok(row)
}

fn fetch_obs_by_key(
    tx: &rusqlite::Transaction<'_>,
    key: &ObservationKey,
) -> Result<Option<Observation>> {
    match key {
        ObservationKey::Id(id) => fetch_observation_by_id(tx, *id),
        ObservationKey::SyncId(s) => fetch_observation_by_sync_id(tx, s),
    }
}

const OBSERVATION_SELECT_FIELDS_BY_ID: &str = "
    SELECT id, sync_id, session_id, type, title, content, tool_name, scope,
           created_by, topic_key, normalized_hash, revision_count, duplicate_count,
           last_seen_at, created_at, updated_at, deleted_at, review_after
      FROM observations WHERE id = ?1
";

const OBSERVATION_SELECT_FIELDS_BY_SYNC: &str = "
    SELECT id, sync_id, session_id, type, title, content, tool_name, scope,
           created_by, topic_key, normalized_hash, revision_count, duplicate_count,
           last_seen_at, created_at, updated_at, deleted_at, review_after
      FROM observations WHERE sync_id = ?1
";

/// Find a supersession candidate for the newly-inserted row `new_id` among
/// active observations of the same `(scope, type)`.
///
/// Two detectors, in priority order:
/// 1. **Title phrase (BM25 top-1).** Exact FTS5 phrase match on the title —
///    catches re-saves that keep the same title.
/// 2. **Topic-key equality.** When the new save carries a `topic_key`, match
///    any other active row with the same `(scope, type, topic_key)`. This
///    catches a *reworded* update whose title (and body) changed but whose
///    stable topic key did not — the title-phrase detector would miss it.
///
/// Returns the candidate observation's id, if any.
///
/// NOTE: in the normal save flow the topic-key branch is largely subsumed by
/// the step-2 in-place topic upsert in [`handle_save_observation`] (a matching
/// `(topic_key, scope)` row is updated in place and we return before conflict
/// detection ever runs), so today it mainly hardens the function against future
/// flow changes and is exercised directly in unit tests. True reworded-update
/// detection for the *no-topic-key* case needs dense-embedding neighbors, which
/// the write thread does not hold — tracked as a follow-up.
fn find_conflict_candidate(
    tx: &rusqlite::Transaction<'_>,
    new_id: i64,
    input: &SaveObservationInput,
) -> Result<Option<i64>> {
    // 1. Title-phrase query: title:"<escaped title>"
    // Double any literal double-quotes in the title so they don't break the
    // FTS5 phrase syntax (FTS5 uses "" as an escape for a literal quote).
    let escaped = input.title.replace('"', "\"\"");
    let query = format!("title:\"{}\"", escaped);
    let by_title: Option<i64> = tx
        .query_row(
            "SELECT o.id
               FROM observations o
               JOIN observations_fts f ON o.id = f.rowid
              WHERE f.observations_fts MATCH ?1
                AND o.scope = ?2
                AND o.type = ?3
                AND o.id != ?4
                AND o.deleted_at IS NULL
              ORDER BY bm25(observations_fts) ASC
              LIMIT 1",
            params![query, &input.scope, &input.r#type, new_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| Error::internal(format!("conflict scan (title): {e}")))?;
    if by_title.is_some() {
        return Ok(by_title);
    }

    // 2. Topic-key equality fallback — a reworded update with a stable
    //    topic_key but a changed title that the phrase detector missed.
    if let Some(topic_key) = &input.topic_key {
        let by_topic: Option<i64> = tx
            .query_row(
                "SELECT id
                   FROM observations
                  WHERE topic_key = ?1
                    AND scope = ?2
                    AND type = ?3
                    AND id != ?4
                    AND deleted_at IS NULL
                  ORDER BY updated_at DESC
                  LIMIT 1",
                params![topic_key, &input.scope, &input.r#type, new_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| Error::internal(format!("conflict scan (topic_key): {e}")))?;
        return Ok(by_topic);
    }

    Ok(None)
}

/// Observation types for which an *unresolved* supersession decision falls
/// back to last-writer-wins — the newer row supersedes the older. These types
/// are single-valued by design ("the current decision", "the current policy"),
/// so retaining a stale prior value is worse than dropping it. Every OTHER
/// type falls back to KEEP-BOTH, so a transient judge error or an unwired judge
/// can never silently delete history we did not positively classify as
/// superseded (Phase 3.1).
const SUPERSEDE_ON_FALLBACK_TYPES: &[&str] = &["decision", "policy"];

/// Conservative supersession decision used whenever the LLM conflict judge is
/// unavailable (no classifier wired, judge disabled, old row vanished, or the
/// call errored/timed out). See [`SUPERSEDE_ON_FALLBACK_TYPES`].
fn fallback_supersede(obs_type: &str) -> bool {
    SUPERSEDE_ON_FALLBACK_TYPES.contains(&obs_type)
}

/// Decide whether to supersede `old_id` given the new `input`.
///
/// 1. If a `conflict_classifier` is present and `cfg.conflict.enabled` is
///    true for this project, ask the LLM. `Supersedes` → supersede;
///    `ConflictsWith` → keep both and record `conflicts_with`;
///    `Compatible` or `NotConflict` → keep both.
/// 2. If no classifier is wired, the judge is disabled, the old row vanished
///    mid-txn, or the LLM errors/times out, defer to [`fallback_supersede`]:
///    last-writer-wins ONLY for the single-valued `decision` / `policy` types,
///    keep-both for everything else. Before Phase 3.1 this fallback superseded
///    unconditionally and could drop correct history on a transient error.
///
/// Decide whether `new_id` supersedes `old_id` on save.
///
/// Phase 3.1b: the LLM judge call that used to run HERE, synchronously inside
/// the `BEGIN IMMEDIATE` write transaction, has been removed. Holding the
/// per-project write lock for up to `conflict.timeout_secs` on every
/// title-collision was the actual latency risk 3.1b exists to close.
///
/// New behavior: when a classifier is configured and enabled, a detected
/// candidate is ALWAYS kept (both rows survive the save, `false`) and a
/// `conflicts_with` relation is recorded in-txn. The daemon already drains
/// every `conflicts_with` relation on the saved observation into
/// `ResolveWorkerPool` right after commit (see the resolve-pool block in
/// `service.rs`), and `resolve_worker::resolve_pair` → `apply_verdict` makes
/// the real (and more capable — it supports KeepNew/KeepOld/KeepBoth/
/// Synthesize) judgment call off the hot path, with durable retry, defaulting
/// to KeepBoth on any ambiguity or error. So no judgment quality is lost —
/// it moves to where the system was already set up to do it asynchronously.
///
/// When no classifier is configured, or `conflict.enabled` is false, there is
/// no async judge to hand off to either, so the type-aware fallback decides
/// synchronously (Goal 1: keep-both for everything except decision/policy).
///
/// `SaveObservation.superseded_ids` is consequently eventually-consistent for
/// LLM-judged collisions: a conflicting save's response will NOT include the
/// old id in `superseded_ids` (it hasn't been judged yet); the resolution
/// lands moments later via the resolve worker. This is the documented
/// contract change the deferred Phase 3.1 goal 2 called out.
fn should_supersede(
    old_id: i64,
    new_id: i64,
    tx: &rusqlite::Transaction<'_>,
    input: &SaveObservationInput,
    classifier: Option<&dyn crate::conflict_judge::ConflictClassifier>,
) -> bool {
    if classifier.is_none() {
        return fallback_supersede(&input.r#type); // no judge: type-aware fallback
    }

    let cfg = statefulmemory_core::config::load_resolved(None);
    if !cfg.conflict.enabled {
        return fallback_supersede(&input.r#type); // feature disabled: type-aware fallback
    }

    // A classifier is configured and enabled: defer the judgment to the async
    // resolve worker. Record the pending relation now (same transaction as
    // the save, so it's never lost) and keep both rows.
    let _ = crate::relations::add_relation(tx, new_id, old_id, "conflicts_with", 0.95);
    tracing::debug!(
        old_id,
        new_id,
        new_title = %input.title,
        "conflict candidate detected — deferring judgment to the async resolve worker"
    );
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_test_db() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let conn = crate::db::open_write(&path).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        // Re-acquire — rusqlite::Connection isn't Sync but we own it.
        (dir, conn)
    }

    fn save_input(content: &str) -> SaveObservationInput {
        SaveObservationInput {
            sync_id: None,
            session_id: "s1".into(),
            r#type: "note".into(),
            title: "t".into(),
            content: content.into(),
            tool_name: None,
            scope: "project".into(),
            created_by: None,
            topic_key: None,
            code_anchor: None,
            dedupe_window_secs: 60 * 60 * 24 * 30,
            max_content_chars: 50_000,
            skip_supersede: false,
        }
    }

    /// `save_input` with an explicit type + title (distinct content keeps the
    /// normalized hashes apart so hash-dedupe never masks the behavior).
    fn typed_input(ty: &str, title: &str, content: &str) -> SaveObservationInput {
        SaveObservationInput {
            r#type: ty.into(),
            title: title.into(),
            ..save_input(content)
        }
    }

    #[test]
    fn sc4_save_get_roundtrip() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let obs = handle_save_observation(&tx, save_input("hello"), None).unwrap();
        tx.commit().unwrap();
        assert_eq!(obs.revision_count, 1);
        assert_eq!(obs.duplicate_count, 1);
        let tx = conn.transaction().unwrap();
        let fetched = fetch_observation_by_sync_id(&tx, &obs.sync_id)
            .unwrap()
            .unwrap();
        assert_eq!(fetched.content, "hello");
    }

    struct ConflictsWithClassifier;
    impl crate::conflict_judge::ConflictClassifier for ConflictsWithClassifier {
        fn classify(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
        ) -> anyhow::Result<crate::conflict_judge::ConflictVerdict> {
            Ok(crate::conflict_judge::ConflictVerdict::ConflictsWith)
        }
    }

    /// Counts `.classify()` invocations. Used to prove the write-path hot
    /// path (Phase 3.1b) never calls the LLM judge synchronously.
    struct CountingClassifier(std::sync::atomic::AtomicUsize);
    impl crate::conflict_judge::ConflictClassifier for CountingClassifier {
        fn classify(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
        ) -> anyhow::Result<crate::conflict_judge::ConflictVerdict> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::conflict_judge::ConflictVerdict::Supersedes)
        }
    }

    #[test]
    fn should_supersede_never_calls_classifier_synchronously() {
        // Phase 3.1b: the whole point of the change — a conflict candidate
        // with a classifier configured must defer to the async resolve
        // worker (recording `conflicts_with` + returning false) WITHOUT
        // invoking the LLM classifier on the write-thread hot path.
        std::env::set_var("STATEFULMEMORY_CONFLICT_ENABLED", "true");
        let (_d, mut conn) = open_test_db();
        let classifier = CountingClassifier(std::sync::atomic::AtomicUsize::new(0));
        let mut a = save_input("use redis for sessions");
        a.title = "session store".into();
        a.sync_id = Some("sync-old-2".into());
        let mut b = save_input("use postgres for sessions");
        b.title = "session store".into();
        b.sync_id = Some("sync-new-2".into());
        let tx = conn.transaction().unwrap();
        handle_save_observation(&tx, a, Some(&classifier)).unwrap();
        let o2 = handle_save_observation(&tx, b, Some(&classifier)).unwrap();
        tx.commit().unwrap();
        std::env::remove_var("STATEFULMEMORY_CONFLICT_ENABLED");
        assert_eq!(
            classifier.0.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "classifier.classify() must NOT be called on the write-thread hot path"
        );
        assert_eq!(o2.deleted_at, None, "both rows kept — judgment deferred");
    }

    #[test]
    fn conflicts_with_keeps_both_rows() {
        std::env::set_var("STATEFULMEMORY_CONFLICT_ENABLED", "true");
        let (_d, mut conn) = open_test_db();
        let mut a = save_input("use redis for sessions");
        a.title = "session store".into();
        a.sync_id = Some("sync-old".into());
        let mut b = save_input("use postgres for sessions");
        b.title = "session store".into();
        b.sync_id = Some("sync-new".into());
        let tx = conn.transaction().unwrap();
        let o1 = handle_save_observation(&tx, a, Some(&ConflictsWithClassifier)).unwrap();
        let o2 = handle_save_observation(&tx, b, Some(&ConflictsWithClassifier)).unwrap();
        let n_active: i64 = tx
            .query_row(
                "SELECT count(*) FROM observations WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let n_rel: i64 = tx
            .query_row(
                "SELECT count(*) FROM observation_relations WHERE relation_type = 'conflicts_with'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        tx.commit().unwrap();
        std::env::remove_var("STATEFULMEMORY_CONFLICT_ENABLED");
        assert_eq!(n_active, 2, "both rows stay active");
        assert_eq!(o1.deleted_at, None);
        assert_eq!(o2.deleted_at, None);
        assert_eq!(n_rel, 1);
    }

    // ---------------------------------------------------------------------
    // Phase 3.1 — non-destructive supersession fallback (goal 1)
    // ---------------------------------------------------------------------

    /// Goal 1(a): with NO classifier wired, a same-title conflict for a
    /// non-decision/policy type must KEEP BOTH rows rather than drop history.
    #[test]
    fn fallback_without_classifier_keeps_both_for_non_decision_types() {
        for ty in ["note", "fact", "pattern", "preference", "context"] {
            let (_d, mut conn) = open_test_db();
            let tx = conn.transaction().unwrap();
            // Same title + type + scope so `find_conflict_candidate` surfaces
            // the older row; distinct content avoids hash-dedupe.
            let a = typed_input(ty, "shared conflict title", "the original value");
            let b = typed_input(ty, "shared conflict title", "a reworded value");
            let _o1 = handle_save_observation(&tx, a, None).unwrap();
            let o2 = handle_save_observation(&tx, b, None).unwrap();
            let n_active: i64 = tx
                .query_row(
                    "SELECT count(*) FROM observations WHERE deleted_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            tx.commit().unwrap();
            assert_eq!(
                n_active, 2,
                "type `{ty}`: both rows must stay active without a classifier"
            );
            assert!(
                o2.superseded_ids.is_empty(),
                "type `{ty}`: new obs must not supersede on fallback"
            );
        }
    }

    /// Goal 1(b): with NO classifier wired, `decision` and `policy` are
    /// single-valued (last-writer-wins) so the older row is still superseded.
    #[test]
    fn fallback_without_classifier_supersedes_decision_and_policy() {
        for ty in ["decision", "policy"] {
            let (_d, mut conn) = open_test_db();
            let tx = conn.transaction().unwrap();
            let a = typed_input(ty, "shared conflict title", "the original value");
            let b = typed_input(ty, "shared conflict title", "the revised value");
            let o1 = handle_save_observation(&tx, a, None).unwrap();
            let o2 = handle_save_observation(&tx, b, None).unwrap();
            let n_active: i64 = tx
                .query_row(
                    "SELECT count(*) FROM observations WHERE deleted_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let old_deleted: Option<String> = tx
                .query_row(
                    "SELECT deleted_at FROM observations WHERE id = ?1",
                    params![o1.id],
                    |r| r.get(0),
                )
                .unwrap();
            tx.commit().unwrap();
            assert_eq!(n_active, 1, "type `{ty}`: older row must be superseded");
            assert!(
                old_deleted.is_some(),
                "type `{ty}`: old row must be soft-deleted"
            );
            assert_eq!(
                o2.superseded_ids,
                vec![o1.id],
                "type `{ty}`: new obs records the superseded id"
            );
        }
    }

    /// `fallback_supersede` is the single source of truth for the type set.
    #[test]
    fn fallback_supersede_type_set() {
        assert!(fallback_supersede("decision"));
        assert!(fallback_supersede("policy"));
        for ty in [
            "note",
            "fact",
            "pattern",
            "preference",
            "context",
            "resolution",
        ] {
            assert!(
                !fallback_supersede(ty),
                "type `{ty}` must keep both on fallback"
            );
        }
    }

    // ---------------------------------------------------------------------
    // Phase 3.1 — semantic candidate detection (goal 3)
    // ---------------------------------------------------------------------

    /// Baseline: the title-phrase detector still finds a same-title candidate.
    #[test]
    fn find_conflict_candidate_matches_on_title_phrase() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let mut a = save_input("body one");
        a.title = "database choice".into();
        let o1 = handle_save_observation(&tx, a, None).unwrap();

        let mut input = save_input("body two");
        input.title = "database choice".into();
        // new_id is a not-yet-present id, so the old row is the only candidate.
        let found = find_conflict_candidate(&tx, o1.id + 1000, &input).unwrap();
        tx.commit().unwrap();
        assert_eq!(found, Some(o1.id));
    }

    /// Goal 3: a reworded update whose title changed is still detected via
    /// `topic_key` equality when the title-phrase match misses.
    #[test]
    fn find_conflict_candidate_falls_back_to_topic_key_on_title_miss() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let mut old = save_input("use redis for sessions");
        old.title = "use redis for sessions".into();
        old.topic_key = Some("arch/session-store".into());
        let o1 = handle_save_observation(&tx, old, None).unwrap();

        // Reworded save: same topic_key, a title that cannot phrase-match.
        let mut input = save_input("switched to postgres");
        input.title = "session storage moved off redis".into();
        input.topic_key = Some("arch/session-store".into());

        let found = find_conflict_candidate(&tx, o1.id + 1000, &input).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            found,
            Some(o1.id),
            "topic_key equality must surface the reworded prior row"
        );
    }

    /// A reworded title with NO topic_key and no phrase overlap yields no
    /// candidate — the detector stays conservative (dense neighbors deferred).
    #[test]
    fn find_conflict_candidate_no_match_without_topic_key_or_title_overlap() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let mut old = save_input("use redis for sessions");
        old.title = "use redis for sessions".into();
        let o1 = handle_save_observation(&tx, old, None).unwrap();

        let mut input = save_input("switched to postgres");
        input.title = "session storage moved off redis".into();
        let found = find_conflict_candidate(&tx, o1.id + 1000, &input).unwrap();
        tx.commit().unwrap();
        assert_eq!(found, None);
    }

    /// `topic_key` candidate detection is scoped by `(scope, type)`: a row with
    /// the same topic_key but a different type is not a candidate.
    #[test]
    fn find_conflict_candidate_topic_key_respects_type_and_scope() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let mut old = typed_input("note", "redis note", "redis body");
        old.topic_key = Some("arch/store".into());
        let o1 = handle_save_observation(&tx, old, None).unwrap();

        // Same topic_key, different type → not a candidate.
        let mut diff_type = typed_input("decision", "pg decision", "pg body");
        diff_type.topic_key = Some("arch/store".into());
        let found = find_conflict_candidate(&tx, o1.id + 1000, &diff_type).unwrap();
        tx.commit().unwrap();
        assert_eq!(found, None, "topic_key match must be gated by type");
    }

    #[test]
    fn sc5_topic_key_upsert() {
        let (_d, mut conn) = open_test_db();
        let mut input1 = save_input("first");
        input1.topic_key = Some("t/foo".into());
        let mut input2 = save_input("second");
        input2.topic_key = Some("t/foo".into());

        let tx = conn.transaction().unwrap();
        let _o1 = handle_save_observation(&tx, input1, None).unwrap();
        let o2 = handle_save_observation(&tx, input2, None).unwrap();
        tx.commit().unwrap();
        assert_eq!(o2.revision_count, 2);
        assert_eq!(o2.content, "second");

        // Only one row total.
        let tx = conn.transaction().unwrap();
        let n: i64 = tx
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn sc6_hash_dedupe() {
        let (_d, mut conn) = open_test_db();
        let mut a = save_input("Same  content");
        a.title = "title-a".into();
        let mut b = save_input("same   CONTENT");
        b.title = "title-b".into();
        let tx = conn.transaction().unwrap();
        let _o1 = handle_save_observation(&tx, a, None).unwrap();
        let o2 = handle_save_observation(&tx, b, None).unwrap();
        tx.commit().unwrap();
        assert_eq!(o2.duplicate_count, 2);
    }

    #[test]
    fn sc7_sync_id_idempotency() {
        let (_d, mut conn) = open_test_db();
        let mut a = save_input("payload");
        a.sync_id = Some("fixed-sync-id".into());
        let mut b = save_input("payload-different");
        b.sync_id = Some("fixed-sync-id".into());
        let tx = conn.transaction().unwrap();
        let _o1 = handle_save_observation(&tx, a, None).unwrap();
        let o2 = handle_save_observation(&tx, b, None).unwrap();
        tx.commit().unwrap();
        // Second save is a no-op — original row returned unchanged.
        assert_eq!(o2.content, "payload");
    }

    #[test]
    fn sc27_review_after_for_decision() {
        let (_d, mut conn) = open_test_db();
        let mut input = save_input("policy text");
        input.r#type = "decision".into();
        let tx = conn.transaction().unwrap();
        let obs = handle_save_observation(&tx, input, None).unwrap();
        tx.commit().unwrap();
        assert!(obs.review_after.is_some());
    }

    #[test]
    fn ec1_empty_content_rejected() {
        let (_d, mut conn) = open_test_db();
        let mut input = save_input("   ");
        input.content = "   ".into();
        let tx = conn.transaction().unwrap();
        let r = handle_save_observation(&tx, input, None);
        assert!(matches!(r, Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn ec2_oversized_content_rejected() {
        let (_d, mut conn) = open_test_db();
        let mut input = save_input(&"a".repeat(60_000));
        input.max_content_chars = 50_000;
        let tx = conn.transaction().unwrap();
        let r = handle_save_observation(&tx, input, None);
        assert!(matches!(r, Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn ec3_invalid_scope_rejected() {
        let (_d, mut conn) = open_test_db();
        let mut input = save_input("ok");
        input.scope = "bogus".into();
        let tx = conn.transaction().unwrap();
        let r = handle_save_observation(&tx, input, None);
        assert!(matches!(r, Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn ec11_dedupe_window_zero_disables_hash_dedupe() {
        let (_d, mut conn) = open_test_db();
        let mut a = save_input("x");
        a.dedupe_window_secs = 0;
        let mut b = save_input("x");
        b.dedupe_window_secs = 0;
        let tx = conn.transaction().unwrap();
        let _o1 = handle_save_observation(&tx, a, None).unwrap();
        let _o2 = handle_save_observation(&tx, b, None).unwrap();
        let n: i64 = tx
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn ec9_topic_upsert_against_soft_deleted_does_new_insert() {
        let (_d, mut conn) = open_test_db();
        let mut a = save_input("first");
        a.topic_key = Some("t/x".into());
        let tx = conn.transaction().unwrap();
        let o1 = handle_save_observation(&tx, a, None).unwrap();
        // Soft-delete it.
        handle_soft_delete_obs(&tx, &ObservationKey::Id(o1.id)).unwrap();
        // Now re-save with the same topic_key — must insert a new row, not resurrect.
        let mut b = save_input("second");
        b.topic_key = Some("t/x".into());
        let o2 = handle_save_observation(&tx, b, None).unwrap();
        assert_ne!(o1.id, o2.id);
        let n: i64 = tx
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(n, 2);
    }

    // ---------------------------------------------------------------------
    // FR12.1 / FR12.2 — Update + hard delete (spec2-t3)
    // ---------------------------------------------------------------------

    #[test]
    fn update_bumps_revision_and_locks_sync_id() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let original = handle_save_observation(&tx, save_input("v1 content"), None).unwrap();
        let original_sync_id = original.sync_id.clone();
        let original_created_at = original.created_at.clone();
        assert_eq!(original.revision_count, 1);

        // Apply a partial update — sync_id and created_at are not in the
        // patch struct, so they must remain locked by construction.
        let patch = ObservationPatch {
            title: Some("renamed".into()),
            content: Some("v2 content".into()),
            ..Default::default()
        };
        let updated = handle_update_obs(&tx, &ObservationKey::Id(original.id), &patch).unwrap();
        tx.commit().unwrap();

        assert_eq!(updated.id, original.id);
        assert_eq!(updated.title, "renamed");
        assert_eq!(updated.content, "v2 content");
        assert_eq!(updated.revision_count, 2);
        // sync_id and created_at locked.
        assert_eq!(updated.sync_id, original_sync_id, "sync_id must not change");
        assert_eq!(
            updated.created_at, original_created_at,
            "created_at must not change"
        );
        // updated_at advanced (or at least did not regress).
        assert!(updated.updated_at >= original.updated_at);
    }

    #[test]
    fn update_invalidates_derived_projections() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let obs = handle_save_observation(
            &tx,
            SaveObservationInput {
                title: "`alpha`".into(),
                content: "`alpha` and `beta`".into(),
                ..save_input("unused")
            },
            None,
        )
        .unwrap();
        handle_insert_embedding(&tx, obs.id, &vec![0.0; 384], "test", false).unwrap();
        handle_insert_facts(
            &tx,
            obs.id,
            &[NewFact {
                subject: "alpha".into(),
                predicate: "is".into(),
                object: "beta".into(),
                temporal: None,
                salience: None,
                extracted_by: "test".into(),
            }],
        )
        .unwrap();
        handle_index_graph(&tx, obs.id, "`alpha`", "`alpha` and `beta`", &[], 1).unwrap();
        let patch = ObservationPatch {
            content: Some("updated content".into()),
            ..Default::default()
        };
        handle_update_obs(&tx, &ObservationKey::Id(obs.id), &patch).unwrap();
        let embedding_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM observation_embedding_meta WHERE observation_id = ?1",
                params![obs.id],
                |row| row.get(0),
            )
            .unwrap();
        let fact_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE obs_id = ?1",
                params![obs.id],
                |row| row.get(0),
            )
            .unwrap();
        let mention_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM entity_mentions WHERE observation_id = ?1",
                params![obs.id],
                |row| row.get(0),
            )
            .unwrap();
        let state: (String, String, Option<String>) = tx
            .query_row(
                "SELECT key_expand, verify_state, exported_at FROM observations WHERE id = ?1",
                params![obs.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(embedding_count, 0);
        assert_eq!(fact_count, 0);
        assert_eq!(mention_count, 0);
        assert_eq!(state, (String::new(), "unanchored".into(), None));
    }

    #[test]
    fn update_resets_export_marker() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let obs = handle_save_observation(&tx, save_input("v1"), None).unwrap();
        tx.execute(
            "UPDATE observations SET exported_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
            params![obs.id],
        )
        .unwrap();
        handle_update_obs(
            &tx,
            &ObservationKey::Id(obs.id),
            &ObservationPatch {
                title: Some("v2".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let exported_at: Option<String> = tx
            .query_row(
                "SELECT exported_at FROM observations WHERE id = ?1",
                params![obs.id],
                |row| row.get(0),
            )
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(exported_at, None);
    }

    #[test]
    fn update_rejects_soft_deleted_observation() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let obs = handle_save_observation(&tx, save_input("v1"), None).unwrap();
        handle_soft_delete_obs(&tx, &ObservationKey::Id(obs.id)).unwrap();
        let patch = ObservationPatch {
            title: Some("new".into()),
            ..Default::default()
        };
        let r = handle_update_obs(&tx, &ObservationKey::Id(obs.id), &patch);
        tx.commit().unwrap();
        assert!(matches!(r, Err(Error::FailedPrecondition(_))));
    }

    #[test]
    fn hard_delete_removes_row_and_fts() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let obs = handle_save_observation(&tx, save_input("uniqueneedlexyz"), None).unwrap();
        tx.commit().unwrap();

        // FTS5 row should be present pre-delete.
        let pre_fts: i64 = conn
            .query_row(
                "SELECT count(*) FROM observations_fts WHERE observations_fts MATCH 'uniqueneedlexyz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_fts, 1);

        // Hard-delete.
        let tx = conn.transaction().unwrap();
        handle_hard_delete_obs(&tx, &ObservationKey::Id(obs.id)).unwrap();
        tx.commit().unwrap();

        let row_count: i64 = conn
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0, "observations row should be gone");

        // FTS5 trigger (obs_fts_ad) should have removed the FTS5 entry.
        let post_fts: i64 = conn
            .query_row(
                "SELECT count(*) FROM observations_fts WHERE observations_fts MATCH 'uniqueneedlexyz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            post_fts, 0,
            "FTS5 entry should have been removed by trigger"
        );
    }

    #[test]
    fn hard_delete_returns_not_found_on_missing_row() {
        let (_d, mut conn) = open_test_db();
        let tx = conn.transaction().unwrap();
        let r = handle_hard_delete_obs(&tx, &ObservationKey::Id(99_999));
        tx.commit().unwrap();
        assert!(matches!(r, Err(Error::NotFound(_))));
    }

    /// Regression for review-finding "silent reply drop in WriteRequest::Custom":
    /// a batch of [SaveObservation, Custom] used to commit both writes but drop
    /// the SaveObservation reply closure, so the caller saw RecvError on its
    /// oneshot::Receiver and the daemon reported "write thread crashed". After
    /// the fix, prior replies fire before the Custom early-return.
    #[test]
    fn process_batch_custom_does_not_drop_prior_replies() {
        let (_d, mut conn) = open_test_db();

        let (save_tx, save_rx) = oneshot::channel::<Result<Observation>>();
        let (custom_tx, custom_rx) = oneshot::channel::<Result<()>>();

        let mut batch: Vec<WriteRequest> = vec![
            WriteRequest::SaveObservation {
                input: save_input("hello-from-batch"),
                reply: save_tx,
            },
            WriteRequest::Custom {
                f: Box::new(|_conn| Ok(())),
                reply: custom_tx,
            },
        ];

        process_batch(&mut conn, &mut batch, None).expect("process_batch");

        // The SaveObservation reply must arrive — its write was committed.
        let saved = save_rx
            .blocking_recv()
            .expect("SaveObservation reply must fire even when Custom follows");
        let obs = saved.expect("SaveObservation must succeed");
        assert_eq!(obs.content, "hello-from-batch");

        // The Custom reply also fires.
        let custom_result = custom_rx.blocking_recv().expect("Custom reply must fire");
        assert!(custom_result.is_ok());

        // And the saved observation is durable on disk.
        let tx = conn.transaction().unwrap();
        let fetched = fetch_observation_by_sync_id(&tx, &obs.sync_id)
            .unwrap()
            .expect("observation must be persisted after process_batch");
        assert_eq!(fetched.content, "hello-from-batch");
    }

    /// IndexGraph riding the real write thread: entities / mentions / pairwise
    /// `mentions` edges must persist and be visible to the graph read fns.
    /// FK on entity_mentions.observation_id requires the observation row to
    /// exist first, so the thread batch is [SaveObservation, IndexGraph].
    #[test]
    fn write_thread_index_graph_persists_entities_mentions_and_edges() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("t.db");

        // Ensure an observation row exists for the FK before indexing.
        {
            let conn = crate::db::open_write(&db_path).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO observations (id, sync_id, session_id, type, title, content)
                 VALUES (1, 'sync1', 's1', 'decision', 't', 'c')",
                [],
            )
            .unwrap();
        }

        let handle = spawn_write_thread(
            "gproj".into(),
            db_path.clone(),
            8,
            Duration::from_millis(5),
            None,
        )
        .unwrap();

        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(WriteRequest::IndexGraph {
                observation_id: 1,
                title: "Fix loader".to_string(),
                content: "see `Widget` inside crates/foo/bar.rs".to_string(),
                anchors: vec!["src/auth.rs::run".to_string()],
                created_at_epoch: 1_700_000_000,
                reply: reply_tx,
            })
            .unwrap();
        reply_rx
            .blocking_recv()
            .expect("IndexGraph reply must fire")
            .expect("IndexGraph must succeed");

        let conn = crate::db::open_read(&db_path).unwrap();
        // Entities: anchor pair + backtick pair, deduped by norm name.
        let lookup = |prefix: &str| {
            crate::graph::entity_lookup(&conn, prefix, 10)
                .into_iter()
                .map(|e| e.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(lookup("src/auth.rs"), vec!["src/auth.rs".to_string()]);
        assert_eq!(lookup("run"), vec!["run".to_string()]);
        assert_eq!(lookup("widget"), vec!["Widget".to_string()]);
        assert_eq!(
            lookup("crates/foo/bar.rs"),
            vec!["crates/foo/bar.rs".to_string()]
        );
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 4);

        let mentions: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mentions, 4);

        // Pairwise edges among the observation's 4 entities = 6 unique pairs.
        let edges: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entity_edges WHERE relation = 'mentions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(edges, 6);
        let (weight, first_seen, last_seen): (f64, i64, i64) = conn
            .query_row(
                "SELECT weight, first_seen, last_seen FROM entity_edges LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(weight, 1.0);
        assert_eq!(first_seen, 1_700_000_000);
        assert_eq!(last_seen, 1_700_000_000);

        // Idempotent re-index: mention dedupe on (entity, obs, source) keeps
        // row counts stable, though edges accumulate weight by design.
        let (reply_tx2, reply_rx2) = oneshot::channel();
        handle
            .send(WriteRequest::IndexGraph {
                observation_id: 1,
                title: "Fix loader".to_string(),
                content: "see `Widget` inside crates/foo/bar.rs".to_string(),
                anchors: vec!["src/auth.rs::run".to_string()],
                created_at_epoch: 1_700_000_100,
                reply: reply_tx2,
            })
            .unwrap();
        reply_rx2
            .blocking_recv()
            .expect("IndexGraph reply must fire")
            .expect("IndexGraph re-index must succeed");
        let mentions: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mentions, 4);
    }

    /// IndexGraph is additive: content change never deletes stale
    /// entities/mentions (lock for handle_index_graph's no-DELETE contract).
    #[test]
    fn write_thread_index_graph_content_change_keeps_stale_mentions() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("t.db");

        {
            let conn = crate::db::open_write(&db_path).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO observations (id, sync_id, session_id, type, title, content)
                 VALUES (1, 'sync1', 's1', 'decision', 't', 'c')",
                [],
            )
            .unwrap();
        }

        let handle = spawn_write_thread(
            "gstale".into(),
            db_path.clone(),
            8,
            Duration::from_millis(5),
            None,
        )
        .unwrap();

        let index = |content: &str| {
            let (tx, rx) = oneshot::channel();
            handle
                .send(WriteRequest::IndexGraph {
                    observation_id: 1,
                    title: "t".to_string(),
                    content: content.to_string(),
                    anchors: vec!["src/auth.rs::run".to_string()],
                    created_at_epoch: 1_700_000_000,
                    reply: tx,
                })
                .unwrap();
            rx.blocking_recv()
                .expect("IndexGraph reply must fire")
                .expect("IndexGraph must succeed");
        };

        index("see `Widget` inside crates/foo/bar.rs");
        index("rewritten to `Gadget` inside crates/foo/bar.rs");

        let conn = crate::db::open_read(&db_path).unwrap();
        let has = |norm: &str| -> bool {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM entity_mentions m
                     JOIN entities e ON e.id = m.entity_id
                     WHERE e.norm_name = ?1",
                    params![norm],
                    |r| r.get(0),
                )
                .unwrap();
            n > 0
        };
        assert!(
            has("widget"),
            "stale backtick entity mention must survive content change (additive contract)"
        );
        assert!(has("gadget"), "new backtick entity must be indexed");
        assert!(has("run"), "anchor-derived mentions must survive re-index");
        let widget_mentions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entity_mentions m
                 JOIN entities e ON e.id = m.entity_id WHERE e.norm_name = 'widget'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            widget_mentions, 1,
            "re-index must not duplicate the stale mention"
        );
    }

    #[test]
    fn insert_facts_supersedes_prior_contradicting_fact() {
        // Phase 3.2: a newer fact with the same (subject, predicate) but a
        // different object supersedes the stale one; same-object re-extraction
        // does not.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let mut conn = crate::db::open_write(&path).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1','/tmp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observations (sync_id, session_id, type, title, content) VALUES ('o1','s1','note','t','c')",
            [],
        )
        .unwrap();
        let obs1 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO observations (sync_id, session_id, type, title, content) VALUES ('o2','s1','note','t2','c2')",
            [],
        )
        .unwrap();
        let obs2 = conn.last_insert_rowid();

        let mk = |object: &str| NewFact {
            subject: "team".into(),
            predicate: "prefers".into(),
            object: object.into(),
            temporal: None,
            salience: Some(0.9),
            extracted_by: "haiku".into(),
        };
        for (obs, obj) in [(obs1, "diesel"), (obs2, "pgx")] {
            let tx = conn.transaction().unwrap();
            handle_insert_facts(&tx, obs, &[mk(obj)]).unwrap();
            tx.commit().unwrap();
        }

        // Active fact lane shows pgx, not the superseded diesel.
        let hits = crate::facts::search_facts(&conn, "diesel OR pgx", 10).unwrap();
        let objs: Vec<&str> = hits.iter().map(|f| f.object.as_str()).collect();
        assert!(objs.contains(&"pgx"), "new value active: {objs:?}");
        assert!(
            !objs.contains(&"diesel"),
            "stale value superseded: {objs:?}"
        );

        // Same-object re-extraction is not a contradiction → both stay active.
        let tx = conn.transaction().unwrap();
        handle_insert_facts(&tx, obs1, &[mk("pgx")]).unwrap();
        tx.commit().unwrap();
        let active = crate::facts::search_facts(&conn, "pgx", 10).unwrap();
        assert_eq!(active.len(), 2, "both pgx facts active (same object)");
    }

    #[test]
    fn insert_facts_merges_key_expand_for_fts() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("t.db");
        {
            let conn = crate::db::open_write(&db_path).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO observations (id, sync_id, session_id, type, title, content)
                 VALUES (1, 'sync1', 's1', 'note', 'auth note', 'picked a library')",
                [],
            )
            .unwrap();
        }
        let handle = spawn_write_thread(
            "fproj".into(),
            db_path.clone(),
            8,
            Duration::from_millis(5),
            None,
        )
        .unwrap();
        let (tx, rx) = oneshot::channel();
        handle
            .send(WriteRequest::InsertFacts {
                obs_id: 1,
                facts: vec![NewFact {
                    subject: "team".into(),
                    predicate: "chose".into(),
                    object: "pgx".into(),
                    temporal: None,
                    salience: Some(0.9),
                    extracted_by: "test".into(),
                }],
                reply: tx,
            })
            .unwrap();
        rx.blocking_recv()
            .expect("InsertFacts reply")
            .expect("InsertFacts ok");
        let conn = crate::db::open_read(&db_path).unwrap();
        let expand: String = conn
            .query_row(
                "SELECT key_expand FROM observations WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(expand.contains("pgx"), "key_expand={expand}");
        // FTS should match via key_expand without query-time expand.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations_fts WHERE observations_fts MATCH 'pgx'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(n >= 1, "FTS must index key_expand tokens");
    }

    #[test]
    fn handle_insert_embedding_quantized_writes_ann_index() {
        // Phase 6.2: quantize=true must populate BOTH the legacy
        // observation_embedding_meta blob (back-compat) AND the native
        // sqlite-vec int8 ANN table (observations_vec_i8, V14) via vec_int8().
        let (_dir, mut conn) = open_test_db();
        let obs_id = {
            let tx = conn.transaction().unwrap();
            let id = handle_save_observation_for_tests(&tx, save_input("alpha")).unwrap();
            tx.commit().unwrap();
            id.id
        };

        let mut embedding = vec![0.0_f32; 384];
        embedding[0] = 1.0;
        embedding[17] = 0.5;

        let tx = conn.transaction().unwrap();
        handle_insert_embedding(&tx, obs_id, &embedding, "bge-small-en-v1.5", true).unwrap();
        tx.commit().unwrap();

        // Legacy blob still present (back-compat / fallback).
        let quantized: bool = conn
            .query_row(
                "SELECT quantized FROM observation_embedding_meta WHERE observation_id = ?1",
                params![obs_id],
                |r| r.get::<_, i64>(0).map(|v| v == 1),
            )
            .unwrap();
        assert!(quantized, "legacy quantized flag must still be set");

        // ANN table has exactly one row for this observation.
        let ann_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM observations_vec_i8 WHERE rowid = ?1",
                params![obs_id],
                |r| r.get(0),
            )
            .expect("observations_vec_i8 must contain the inserted row");
        assert_eq!(ann_rowid, obs_id);
    }

    #[test]
    fn handle_insert_embedding_quantized_ann_round_trips_via_search_dense() {
        // End-to-end: quantize=true write path -> read::search_dense (which
        // tries the ANN table first) finds the exact-match row ranked first.
        let (_dir, mut conn) = open_test_db();
        let mut mk = |content: &str| -> i64 {
            let tx = conn.transaction().unwrap();
            let id = handle_save_observation_for_tests(&tx, save_input(content)).unwrap();
            tx.commit().unwrap();
            id.id
        };
        let id_a = mk("alpha body");
        let id_b = mk("beta body");

        let mut vec_a = vec![0.0_f32; 384];
        vec_a[0] = 1.0;
        let mut vec_b = vec![0.0_f32; 384];
        vec_b[200] = 1.0;

        for (id, v) in [(id_a, &vec_a), (id_b, &vec_b)] {
            let tx = conn.transaction().unwrap();
            handle_insert_embedding(&tx, id, v, "bge-small-en-v1.5", true).unwrap();
            tx.commit().unwrap();
        }

        let hits = crate::read::search_dense(&conn, &vec_a, 2).unwrap();
        assert!(!hits.is_empty(), "ANN search must return hits");
        assert_eq!(
            hits[0].id, id_a,
            "exact-match vector must rank first via the ANN index"
        );
    }

    #[test]
    fn handle_insert_chunks_writes_rows_and_cascades_on_delete() {
        let (_dir, mut conn) = open_test_db();
        let obs_id = {
            let tx = conn.transaction().unwrap();
            let id = handle_save_observation_for_tests(&tx, save_input("long body")).unwrap();
            tx.commit().unwrap();
            id.id
        };

        let mut v1 = vec![0.0_f32; 384];
        v1[0] = 1.0;
        let mut v2 = vec![0.0_f32; 384];
        v2[1] = 1.0;
        let chunks = vec![
            ("chunk one text".to_string(), v1.clone()),
            ("chunk two text".to_string(), v2.clone()),
        ];

        let tx = conn.transaction().unwrap();
        handle_insert_chunks(&tx, obs_id, &chunks).unwrap();
        tx.commit().unwrap();

        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM observations_chunks WHERE obs_id = ?1",
                params![obs_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "both chunks inserted");
        let n_vec: i64 = conn
            .query_row("SELECT count(*) FROM chunk_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n_vec, 2, "both chunk vectors inserted");

        // Re-inserting (re-embed) replaces, doesn't duplicate.
        let tx = conn.transaction().unwrap();
        handle_insert_chunks(&tx, obs_id, &chunks[..1]).unwrap();
        tx.commit().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM observations_chunks WHERE obs_id = ?1",
                params![obs_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "re-insert replaces prior chunks");

        // Cascade: deleting the observation drops its chunks + chunk vectors
        // (V15 FK cascade + AFTER DELETE trigger).
        conn.execute("DELETE FROM observations WHERE id = ?1", params![obs_id])
            .unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM observations_chunks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "chunks cascade-deleted with the observation");
        let n_vec: i64 = conn
            .query_row("SELECT count(*) FROM chunk_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n_vec, 0,
            "chunk vectors cleaned up by the AFTER DELETE trigger"
        );
    }

    #[test]
    fn chunked_dense_search_max_pools_best_chunk_per_observation() {
        // End-to-end: an observation with TWO chunks, only one of which
        // matches the query closely, must still surface the observation —
        // and search_dense_chunks_scored must rank it via its best chunk,
        // not get diluted by averaging with the irrelevant chunk.
        let (_dir, mut conn) = open_test_db();
        let obs_id = {
            let tx = conn.transaction().unwrap();
            let id = handle_save_observation_for_tests(&tx, save_input("long body")).unwrap();
            tx.commit().unwrap();
            id.id
        };

        let mut query = vec![0.0_f32; 384];
        query[0] = 1.0;

        let mut matching_chunk = vec![0.0_f32; 384];
        matching_chunk[0] = 1.0; // identical to query -> distance ~0
        let mut unrelated_chunk = vec![0.0_f32; 384];
        unrelated_chunk[200] = 1.0; // orthogonal -> large distance

        let tx = conn.transaction().unwrap();
        handle_insert_chunks(
            &tx,
            obs_id,
            &[
                ("unrelated passage".to_string(), unrelated_chunk),
                ("matching passage".to_string(), matching_chunk),
            ],
        )
        .unwrap();
        tx.commit().unwrap();

        let hits = crate::read::search_dense_chunks_scored(&conn, &query, 10).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "one observation surfaces once, not per-chunk"
        );
        assert_eq!(hits[0].0.id, obs_id);
        assert!(
            hits[0].1 < 0.01,
            "best-chunk distance should reflect the matching chunk, not an average: {}",
            hits[0].1
        );
    }

    #[test]
    fn handle_insert_embedding_routes_768_dim_to_v16_tables() {
        // Phase 6.3: a 768-dim embedding (e.g. a bge-base-class model) must
        // route to the V16 parallel table set, NOT the 384-dim V4 tables —
        // and must be independently searchable via read::search_dense.
        let (_dir, mut conn) = open_test_db();
        let obs_id = {
            let tx = conn.transaction().unwrap();
            let id = handle_save_observation_for_tests(&tx, save_input("alpha")).unwrap();
            tx.commit().unwrap();
            id.id
        };

        let mut embedding = vec![0.0_f32; 768];
        embedding[0] = 1.0;

        let tx = conn.transaction().unwrap();
        handle_insert_embedding(&tx, obs_id, &embedding, "bge-base-en-v1.5", false).unwrap();
        tx.commit().unwrap();

        // Lands in the 768-dim table, NOT the 384-dim one.
        let in_768: i64 = conn
            .query_row(
                "SELECT count(*) FROM observations_vec_768 WHERE rowid = ?1",
                params![obs_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            in_768, 1,
            "768-dim vector must land in observations_vec_768"
        );
        let in_384: i64 = conn
            .query_row("SELECT count(*) FROM observations_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(in_384, 0, "the 384-dim table must stay empty");

        // Stored dim is recorded correctly.
        let stored_dim: i64 = conn
            .query_row(
                "SELECT dim FROM observation_embedding_meta WHERE observation_id = ?1",
                params![obs_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored_dim, 768);

        // End-to-end: search_dense (768-dim query) finds it via the ANN path.
        let hits = crate::read::search_dense(&conn, &embedding, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].id, obs_id,
            "exact-match 768-dim vector found via search_dense"
        );
    }

    #[test]
    fn handle_insert_embedding_768_quantized_routes_to_v16_ann_table() {
        // Same as above but for the int8-quantized ANN path (V16's
        // observations_vec_768_i8), proving 6.2's ANN work and 6.3's
        // dimension-routing compose correctly together.
        let (_dir, mut conn) = open_test_db();
        let obs_id = {
            let tx = conn.transaction().unwrap();
            let id = handle_save_observation_for_tests(&tx, save_input("beta")).unwrap();
            tx.commit().unwrap();
            id.id
        };
        let mut embedding = vec![0.0_f32; 768];
        embedding[5] = 1.0;

        let tx = conn.transaction().unwrap();
        handle_insert_embedding(&tx, obs_id, &embedding, "bge-base-en-v1.5", true).unwrap();
        tx.commit().unwrap();

        let in_768_i8: i64 = conn
            .query_row(
                "SELECT count(*) FROM observations_vec_768_i8 WHERE rowid = ?1",
                params![obs_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            in_768_i8, 1,
            "768-dim quantized vector must land in observations_vec_768_i8"
        );

        let hits = crate::read::search_dense(&conn, &embedding, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, obs_id);
    }

    #[test]
    fn unsupported_embedding_dimension_is_rejected() {
        let (_dir, mut conn) = open_test_db();
        let obs_id = {
            let tx = conn.transaction().unwrap();
            let id = handle_save_observation_for_tests(&tx, save_input("gamma")).unwrap();
            tx.commit().unwrap();
            id.id
        };
        let bad = vec![0.0_f32; 512]; // not 384 or 768
        let tx = conn.transaction().unwrap();
        let err = handle_insert_embedding(&tx, obs_id, &bad, "mystery-model", false).unwrap_err();
        assert!(format!("{err}").contains("512"), "{err}");
    }
}
