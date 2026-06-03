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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    prompts as prompts_q,
    read as read_q,
    sessions as sessions_q,
    stats as stats_q,
    write::{
        ObservationKey, ObservationPatch, PromptKey, SaveObservationInput, WriteRequest,
    },
    ProjectRegistry, ProjectState,
};

use crate::error_map::{map, to_status};
use crate::tokens::TokenStore;

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
        Ok(Response::new(SaveObservationResponse {
            observation: Some(obs_to_proto(obs)),
            similar_observations: vec![],
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
        let obs = map(rx.await.map_err(|_| Error::internal("write thread crashed"))?)?;
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
        map(rx.await.map_err(|_| Error::internal("write thread crashed"))?)?;
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
    // Spec 1 stubs; Spec 2 adds full implementations.

    async fn context(
        &self,
        _req: Request<ContextRequest>,
    ) -> Result<Response<ContextResponse>, Status> {
        Err(Status::unimplemented("Context RPC: implemented in Spec 2"))
    }
    async fn timeline(
        &self,
        _req: Request<TimelineRequest>,
    ) -> Result<Response<TimelineResponse>, Status> {
        Err(Status::unimplemented("Timeline RPC: implemented in Spec 2"))
    }
    async fn suggest_topic_key(
        &self,
        _req: Request<SuggestTopicKeyRequest>,
    ) -> Result<Response<SuggestTopicKeyResponse>, Status> {
        Err(Status::unimplemented("SuggestTopicKey RPC: implemented in Spec 2"))
    }
    async fn capture_passive(
        &self,
        _req: Request<CapturePassiveRequest>,
    ) -> Result<Response<CapturePassiveResponse>, Status> {
        Err(Status::unimplemented("CapturePassive RPC: implemented in Spec 2"))
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
        let session = map(rx.await.map_err(|_| Error::internal("write thread crashed"))?)?;
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
        let s = map(rx.await.map_err(|_| Error::internal("write thread crashed"))?)?;
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
        let s = map(rx.await.map_err(|_| Error::internal("write thread crashed"))?)?;
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
        _req: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        Err(Status::unimplemented("ListSessions: implemented in Spec 2"))
    }

    async fn delete_session(
        &self,
        _req: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        Err(Status::unimplemented("DeleteSession: implemented in Spec 2"))
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
        let p = map(rx.await.map_err(|_| Error::internal("write thread crashed"))?)?;
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
        _req: Request<DeletePromptRequest>,
    ) -> Result<Response<DeletePromptResponse>, Status> {
        Err(Status::unimplemented("DeletePrompt: implemented in Spec 2"))
    }

    // ---- Project ops (Spec 2) ----

    async fn list_projects(
        &self,
        _req: Request<ListProjectsRequest>,
    ) -> Result<Response<ListProjectsResponse>, Status> {
        Err(Status::unimplemented("ListProjects: implemented in Spec 2"))
    }
    async fn current_project(
        &self,
        _req: Request<CurrentProjectRequest>,
    ) -> Result<Response<CurrentProjectResponse>, Status> {
        Err(Status::unimplemented("CurrentProject: implemented in Spec 2"))
    }
    async fn merge_projects(
        &self,
        _req: Request<MergeProjectsRequest>,
    ) -> Result<Response<MergeProjectsResponse>, Status> {
        Err(Status::unimplemented("MergeProjects: implemented in Spec 2"))
    }
    async fn delete_project(
        &self,
        _req: Request<DeleteProjectRequest>,
    ) -> Result<Response<DeleteProjectResponse>, Status> {
        Err(Status::unimplemented("DeleteProject: implemented in Spec 2"))
    }
    async fn consolidate_projects(
        &self,
        _req: Request<ConsolidateProjectsRequest>,
    ) -> Result<Response<ConsolidateProjectsResponse>, Status> {
        Err(Status::unimplemented("ConsolidateProjects: implemented in Spec 2"))
    }
    async fn prune_projects(
        &self,
        _req: Request<PruneProjectsRequest>,
    ) -> Result<Response<PruneProjectsResponse>, Status> {
        Err(Status::unimplemented("PruneProjects: implemented in Spec 2"))
    }

    // ---- Tokens (admin / Spec 2) ----

    async fn create_token(
        &self,
        _req: Request<CreateTokenRequest>,
    ) -> Result<Response<CreateTokenResponse>, Status> {
        Err(Status::unimplemented("CreateToken: implemented in Spec 2"))
    }
    async fn list_tokens(
        &self,
        _req: Request<ListTokensRequest>,
    ) -> Result<Response<ListTokensResponse>, Status> {
        Err(Status::unimplemented("ListTokens: implemented in Spec 2"))
    }
    async fn revoke_token(
        &self,
        _req: Request<RevokeTokenRequest>,
    ) -> Result<Response<RevokeTokenResponse>, Status> {
        Err(Status::unimplemented("RevokeToken: implemented in Spec 2"))
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
        _req: Request<SyncStatusRequest>,
    ) -> Result<Response<SyncStatusResponse>, Status> {
        // Spec 1 returns an all-zeroes/empty stub.
        Ok(Response::new(SyncStatusResponse::default()))
    }
    async fn sync_export(
        &self,
        _req: Request<SyncExportRequest>,
    ) -> Result<Response<SyncExportResponse>, Status> {
        Err(Status::unimplemented("SyncExport: implemented in Spec 3"))
    }
    async fn sync_import(
        &self,
        _req: Request<SyncImportRequest>,
    ) -> Result<Response<SyncImportResponse>, Status> {
        Err(Status::unimplemented("SyncImport: implemented in Spec 3"))
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
