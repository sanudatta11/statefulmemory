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
    models::{Observation, Prompt, Session},
    projects_admin,
    prompts as prompts_q,
    read as read_q,
    sessions as sessions_q,
    stats as stats_q,
    write::{
        ObservationKey, ObservationPatch, PromptKey, SaveObservationInput, WriteRequest,
    },
    ProjectRegistry, ProjectState,
};

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

    fn open_project(&self, name: &str) -> Result<Arc<ProjectState>> {
        if name.trim().is_empty() {
            return Err(Error::invalid("project_name is required"));
        }
        self.state.registry.get_or_open(name)
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
        let input = SaveObservationInput {
            sync_id: r.sync_id,
            session_id: r.session_id,
            r#type: r.r#type,
            title: r.title,
            content: r.content,
            tool_name: r.tool_name,
            scope: if r.scope.is_empty() { "project".into() } else { r.scope },
            created_by: r.created_by,
            topic_key: r.topic_key,
            dedupe_window_secs: self.state.dedupe_window.as_secs(),
            max_content_chars: self.state.max_content_chars,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::SaveObservation { input, reply: tx }))?;
        let obs = rx.await.map_err(|_| Status::internal("write thread crashed"))?;
        let obs = map(obs)?;
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
            }));
        }
        let hits = map(read_q::search(
            &conn,
            &r.query,
            r.r#type.as_deref(),
            r.scope.as_deref(),
            limit,
        ))?;
        Ok(Response::new(SearchObservationsResponse {
            observations: hits.into_iter().map(obs_to_proto).collect(),
            warning: None,
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
            observations: rows.into_iter().map(obs_to_proto).collect(),
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
        let (recents, topics) = map(read_q::recent_active(&conn, limit))?;
        let snapshot = ContextSnapshot {
            recent_observations: recents.into_iter().map(obs_to_proto).collect(),
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
        Ok(Response::new(ContextResponse {
            snapshot: Some(snapshot),
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
        .and_then(|inner| map(inner))?;
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
