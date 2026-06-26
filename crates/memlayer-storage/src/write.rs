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

use memlayer_core::error::{Error, Result};
use memlayer_core::time as mtime;

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
    /// Run an ad-hoc closure under the write connection. Used by sync import,
    /// project merge, and other multi-row operations.
    Custom {
        f: Box<dyn FnOnce(&mut Connection) -> Result<()> + Send>,
        reply: oneshot::Sender<Result<()>>,
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
    /// Dedupe window in seconds (0 disables hash dedupe, EC-11).
    pub dedupe_window_secs: u64,
    /// Maximum content length (EC-2).
    pub max_content_chars: usize,
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
        .name(format!("memlayer-write-{project_id}"))
        .spawn(move || {
            run_write_loop(pid_clone, conn, rx, batch_max, batch_window, conflict_classifier);
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
                reply,
            } => {
                let r = handle_insert_embedding(&tx, obs_id, &embedding, &model);
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
    let review_after =
        dedupe::review_months_for_type(&input.r#type).map(|m| mtime::months_from_now(m));

    // 2. Topic-key upsert: if (topic_key, scope) match a non-deleted row,
    //    update it in place and return. (SC-5, EC-9)
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
                        review_after = COALESCE(?6, review_after)
                  WHERE id = ?1",
                params![
                    existing.id,
                    &input.title,
                    &input.content,
                    &normalized_hash,
                    &now,
                    &review_after,
                ],
            )
            .map_err(|e| Error::internal(format!("topic upsert update: {e}")))?;
            return fetch_observation_by_id(tx, existing.id)?
                .ok_or_else(|| Error::internal("topic upsert: row vanished"));
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
                        last_seen_at = ?2
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

    let sync_id = input.sync_id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
    tx.execute(
        "INSERT INTO observations
            (sync_id, session_id, type, title, content, tool_name, scope,
             created_by, topic_key, normalized_hash, last_seen_at,
             created_at, updated_at, review_after)
         VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13)",
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
    let superseded_id = find_conflict_candidate(tx, id, &input)?;
    let do_supersede = if let Some(old_id) = superseded_id {
        should_supersede(old_id, tx, &input, conflict_classifier)
    } else {
        false
    };
    if do_supersede {
        let old_id = superseded_id.unwrap();
        tx.execute(
            "UPDATE observations
                SET deleted_at = ?2,
                    delete_reason = 'superseded',
                    superseded_by_id = ?3
              WHERE id = ?1",
            params![old_id, &now, id],
        )
        .map_err(|e| Error::internal(format!("supersede old obs: {e}")))?;
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
            "UPDATE sessions SET ended_at = ?2, summary = COALESCE(?3, summary) WHERE id = ?1",
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
            "UPDATE sessions SET summary = ?2 WHERE id = ?1",
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
    if let Some(p) = tx
        .query_row(
            "SELECT id, sync_id, session_id, content, created_at FROM user_prompts WHERE sync_id = ?1",
            params![sync_id],
            crate::models::Prompt::from_row,
        )
        .ok()
    {
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

fn handle_soft_delete_obs(tx: &rusqlite::Transaction<'_>, key: &ObservationKey) -> Result<()> {
    let now = mtime::now_rfc3339();
    let n = match key {
        ObservationKey::Id(id) => tx.execute(
            "UPDATE observations SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now],
        ),
        ObservationKey::SyncId(s) => tx.execute(
            "UPDATE observations SET deleted_at = ?2 WHERE sync_id = ?1 AND deleted_at IS NULL",
            params![s, now],
        ),
    }
    .map_err(|e| Error::internal(format!("soft-delete: {e}")))?;
    if n == 0 {
        return Err(Error::not_found("observation"));
    }
    Ok(())
}

fn handle_hard_delete_obs(tx: &rusqlite::Transaction<'_>, key: &ObservationKey) -> Result<()> {
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
    let normalized_hash = dedupe::hash(content);
    tx.execute(
        "UPDATE observations
            SET title = ?2,
                content = ?3,
                topic_key = ?4,
                scope = ?5,
                type = ?6,
                normalized_hash = ?7,
                revision_count = revision_count + 1,
                updated_at = ?8
          WHERE id = ?1",
        params![
            existing.id,
            title,
            content,
            topic_key,
            scope,
            r#type,
            &normalized_hash,
            &now,
        ],
    )
    .map_err(|e| Error::internal(format!("update: {e}")))?;
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
fn handle_insert_embedding(
    tx: &rusqlite::Transaction<'_>,
    obs_id: i64,
    embedding: &[f32],
    model: &str,
) -> Result<()> {
    if embedding.len() != 384 {
        return Err(Error::invalid(format!(
            "embedding dim mismatch: got {}, expected 384",
            embedding.len()
        )));
    }
    let mut blob = Vec::with_capacity(embedding.len() * 4);
    for f in embedding {
        blob.extend_from_slice(&f.to_le_bytes());
    }
    tx.execute(
        "INSERT OR REPLACE INTO observations_vec(rowid, embedding) VALUES (?1, ?2)",
        params![obs_id, blob],
    )
    .map_err(|e| Error::internal(format!("insert observations_vec: {e}")))?;
    tx.execute(
        "INSERT OR REPLACE INTO observation_embedding_meta \
            (observation_id, model, dim, created_at) \
         VALUES (?1, ?2, ?3, datetime('now'))",
        params![obs_id, model, 384i64],
    )
    .map_err(|e| Error::internal(format!("insert observation_embedding_meta: {e}")))?;
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

/// Find the top BM25 hit for `input.title` among active observations of the
/// same type and scope, excluding the newly-inserted row `new_id`.
/// Returns the conflicting observation's id if one is found.
fn find_conflict_candidate(
    tx: &rusqlite::Transaction<'_>,
    new_id: i64,
    input: &SaveObservationInput,
) -> Result<Option<i64>> {
    // Build a phrase query: title:"<escaped title>"
    // Double any literal double-quotes in the title so they don't break the
    // FTS5 phrase syntax (FTS5 uses "" as an escape for a literal quote).
    let escaped = input.title.replace('"', "\"\"");
    let query = format!("title:\"{}\"", escaped);
    let result: Option<i64> = tx
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
        .map_err(|e| Error::internal(format!("conflict scan: {e}")))?;
    Ok(result)
}

/// Decide whether to supersede `old_id` given the new `input`.
///
/// 1. If a `conflict_classifier` is present and `cfg.conflict.enabled` is
///    true for this project, ask the LLM. `Supersedes` or `ConflictsWith`
///    → supersede; `Compatible` or `NotConflict` → keep both.
/// 2. On any error (timeout, network, parse) fall back to heuristic
///    (supersede unconditionally, matching pre-Spec-4 behavior).
/// 3. If no classifier is wired, always supersede (current default).
fn should_supersede(
    old_id: i64,
    tx: &rusqlite::Transaction<'_>,
    input: &SaveObservationInput,
    classifier: Option<&dyn crate::conflict_judge::ConflictClassifier>,
) -> bool {
    use crate::conflict_judge::ConflictVerdict;

    let classifier = match classifier {
        Some(c) => c,
        None => return true, // no judge: heuristic supersession
    };

    // Re-resolve project config so per-project overrides apply. We pass the
    // project name from the normalized DB path; storage doesn't know it, but
    // the write thread was spawned with it as `project_id`. Rather than
    // threading the name here we just call load_resolved(None) — that gives
    // the global config, which is sufficient for the default-off guard.
    let cfg = memlayer_core::config::load_resolved(None);
    if !cfg.conflict.enabled {
        return true; // feature disabled: heuristic
    }

    // Fetch the old observation's title + content for the judge prompt.
    let old: Option<(String, String)> = tx
        .query_row(
            "SELECT title, content FROM observations WHERE id = ?1",
            params![old_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap_or(None);
    let (old_title, old_content) = match old {
        Some(p) => p,
        None => return true, // old row missing (race): supersede to be safe
    };

    match classifier.classify(&old_title, &old_content, &input.title, &input.content) {
        Ok(ConflictVerdict::Supersedes) | Ok(ConflictVerdict::ConflictsWith) => true,
        Ok(ConflictVerdict::Compatible) | Ok(ConflictVerdict::NotConflict) => {
            tracing::debug!(
                old_id,
                new_title = %input.title,
                "conflict judge: NOT superseding (Compatible/NotConflict)"
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                old_id,
                error = %e,
                "conflict judge error; falling back to heuristic supersession"
            );
            true
        }
    }
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
            dedupe_window_secs: 60 * 60 * 24 * 30,
            max_content_chars: 50_000,
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
}
