//! The MCP server struct and its tool registry.
//!
//! `MemoryServer` hosts the six `memory_*` tools via rmcp's `#[tool_router]`.
//! Each tool maps onto an existing daemon gRPC RPC through [`LazyClient`].

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ErrorData, Peer, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;

use crate::client::LazyClient;
use crate::error::McpError;
use crate::render;
use crate::scope::resolve_project;
use crate::tools::{AddArgs, ContextArgs, FactsArgs, HealthArgs, RecentArgs, SearchArgs};

/// Local stdio MCP server exposing memlayer memory operations as tools.
#[derive(Clone)]
pub struct MemoryServer {
    /// Lazily-connected daemon client (shared across cloned handler instances).
    pub(crate) client: Arc<LazyClient>,
    /// cwd-derived project used when a tool call omits `project`.
    pub(crate) base_project: String,
    /// Fallback identity when MCP initialize has not yet supplied clientInfo.
    pub(crate) client_info: String,
}

impl MemoryServer {
    pub fn new(socket_path: PathBuf, base_project: String, client_info: String) -> Self {
        Self {
            client: Arc::new(LazyClient::new(socket_path)),
            base_project,
            client_info,
        }
    }

    fn effective_created_by(&self, peer: &Peer<RoleServer>) -> String {
        peer.peer_info()
            .map(|info| format_client_info(&info.client_info))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.client_info.clone())
    }
}

fn format_client_info(info: &Implementation) -> String {
    if info.version.is_empty() {
        info.name.clone()
    } else {
        format!("{}@{}", info.name, info.version)
    }
}

fn clamp_limit(limit: Option<i32>, default: i32) -> i32 {
    limit.unwrap_or(default).clamp(1, 50)
}

/// Run the MCP server over stdio until the client disconnects (stdin EOF).
///
/// Starts even if the daemon is unreachable — connection is deferred to the
/// first tool call, and `memory_health` reports a dead daemon rather than
/// failing to start. Diagnostics go to stderr; stdout carries MCP frames.
pub async fn serve(
    socket_path: PathBuf,
    project: String,
    client_info: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = MemoryServer::new(socket_path, project, client_info);
    let running = server.serve(rmcp::transport::io::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[tool_router(vis = "pub")]
impl MemoryServer {
    /// Search stored project memory. Modes: "bm25" (default) or "hybrid"
    /// (BM25 + dense, RRF-fused). The daemon auto-starts on first use, so do
    /// not call memory_health first; only call memory_health if a tool errors.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn memory_search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        if args.query.trim().is_empty() {
            return Err(McpError::BadArgs {
                message: "query must not be empty".into(),
            }
            .into());
        }
        let req = memlayer_proto::SearchObservationsRequest {
            project_name: project,
            query: args.query,
            r#type: args.type_,
            scope: args.scope,
            all_projects: false,
            limit: clamp_limit(args.limit, 10),
            mode: Some(args.mode.unwrap_or_else(|| "bm25".into())),
            rerank: args.rerank,
        };
        let resp = self
            .client
            .call(|mut c| {
                let req = req.clone();
                async move { c.search_observations(req).await.map(|r| r.into_inner()) }
            })
            .await?;
        Ok(CallToolResult::structured(json!({
            "observations": resp.observations.iter().map(render::observation_hit).collect::<Vec<_>>(),
            "warning": resp.warning,
        })))
    }

    /// Save an observation (decision/fix/pattern/note/feedback) to project
    /// memory. The daemon auto-starts on first use; the calling agent is
    /// recorded automatically. Returns the new observation id.
    #[tool(annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false))]
    async fn memory_add(
        &self,
        Parameters(args): Parameters<AddArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        if args.title.trim().is_empty() || args.content.trim().is_empty() {
            return Err(McpError::BadArgs {
                message: "title and content must not be empty".into(),
            }
            .into());
        }
        let created_by = self.effective_created_by(&peer);
        // Spec P6: optional anchor → tool_name hint (file::symbol style).
        let req = memlayer_proto::SaveObservationRequest {
            project_name: project,
            sync_id: None,
            session_id: args.session.unwrap_or_default(),
            r#type: args.type_,
            title: args.title,
            content: args.content,
            tool_name: args.anchor,
            scope: "project".into(),
            created_by: Some(created_by),
            topic_key: None,
        };
        let resp = self
            .client
            .call(|mut c| {
                let req = req.clone();
                async move { c.save_observation(req).await.map(|r| r.into_inner()) }
            })
            .await?;
        let obs = resp.observation.as_ref();
        Ok(CallToolResult::structured(json!({
            "id": obs.map(|o| o.id),
            "type": obs.map(|o| o.r#type.clone()),
            "title": obs.map(|o| o.title.clone()),
            "created_by": obs.and_then(|o| o.created_by.clone()),
            "superseded": resp.similar_observations.iter().map(|o| o.id).collect::<Vec<_>>(),
        })))
    }

    /// Get session context from project memory. With no query, returns the
    /// session briefing (recent + pending); with a query, returns topic-ranked
    /// context. The daemon auto-starts on first use.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn memory_context(
        &self,
        Parameters(args): Parameters<ContextArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        let req = memlayer_proto::ContextRequest {
            project_name: project,
            recent_limit: clamp_limit(args.limit, 10),
            mode: Some(args.mode.unwrap_or_else(|| "bm25".into())),
            rerank: args.rerank,
            query: args.query.filter(|q| !q.trim().is_empty()),
        };
        let resp = self
            .client
            .call(|mut c| {
                let req = req.clone();
                async move { c.context(req).await.map(|r| r.into_inner()) }
            })
            .await?;
        let snapshot = resp.snapshot.unwrap_or_default();
        Ok(CallToolResult::structured(json!({
            "recent": snapshot.recent_observations.iter().map(render::observation_brief).collect::<Vec<_>>(),
            "topics": snapshot.active_topics.iter().map(|t| json!({
                "topic_key": t.topic_key,
                "scope": t.scope,
            })).collect::<Vec<_>>(),
        })))
    }

    /// List the atomic facts extracted from a given observation. Returns an
    /// empty list (not an error) when the observation has no extracted facts.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn memory_facts(
        &self,
        Parameters(args): Parameters<FactsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        if args.observation_id <= 0 {
            return Err(McpError::BadArgs {
                message: "observation_id must be a positive integer".into(),
            }
            .into());
        }
        let req = memlayer_proto::GetFactsRequest {
            project_name: project,
            observation_id: args.observation_id,
        };
        let resp = self
            .client
            .call(|mut c| {
                let req = req.clone();
                async move { c.get_facts(req).await.map(|r| r.into_inner()) }
            })
            .await?;
        Ok(CallToolResult::structured(json!({
            "observation_id": args.observation_id,
            "facts": resp.facts.iter().map(render::fact_row).collect::<Vec<_>>(),
        })))
    }

    /// List the most recent observations for the project. The daemon
    /// auto-starts on first use.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn memory_recent(
        &self,
        Parameters(args): Parameters<RecentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        let req = memlayer_proto::RecentObservationsRequest {
            project_name: project,
            limit: clamp_limit(args.limit, 10),
            scope: args.scope,
        };
        let resp = self
            .client
            .call(|mut c| {
                let req = req.clone();
                async move { c.recent_observations(req).await.map(|r| r.into_inner()) }
            })
            .await?;
        Ok(CallToolResult::structured(json!({
            "observations": resp.observations.iter().map(render::observation_hit).collect::<Vec<_>>(),
        })))
    }

    /// Check daemon health and get keyed remediation. Call this only when
    /// another tool has reported an error; it reports whether the daemon is
    /// running and, if not, exactly how to fix it.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn memory_health(
        &self,
        Parameters(args): Parameters<HealthArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        let socket = self.client.socket_path();

        if !socket.exists() {
            return Ok(CallToolResult::structured(json!({
                "status": "unhealthy",
                "checks": [{
                    "key": "socket_missing",
                    "ok": false,
                    "remedy": McpError::SocketMissing { path: socket.display().to_string() }.remedy(),
                    "path": socket.display().to_string(),
                }],
            })));
        }

        let status = match self
            .client
            .call(|mut c| async move {
                c.daemon_status(memlayer_proto::DaemonStatusRequest {})
                    .await
                    .map(|r| r.into_inner())
            })
            .await
        {
            Ok(s) => s,
            Err(err) => {
                return Ok(CallToolResult::structured(json!({
                    "status": "unhealthy",
                    "checks": [{
                        "key": err.key(),
                        "ok": false,
                        "remedy": err.remedy(),
                        "message": err.to_string(),
                    }],
                })));
            }
        };

        let doctor = self
            .client
            .call(|mut c| {
                let project = project.clone();
                async move {
                    c.doctor(memlayer_proto::DoctorRequest {
                        project_name: Some(project),
                        auto_repair: false,
                    })
                    .await
                    .map(|r| r.into_inner())
                }
            })
            .await;

        let mut checks = vec![json!({
            "key": "daemon_running",
            "ok": true,
            "version": status.version,
            "pid": status.pid,
            "started_at": status.started_at,
            "read_only_mode": status.read_only_mode,
        })];

        match doctor {
            Ok(doc) => {
                for f in doc.findings {
                    checks.push(json!({
                        "key": f.code,
                        "ok": f.severity != "error",
                        "severity": f.severity,
                        "message": f.message,
                        "remedy": f.remedy,
                    }));
                }
            }
            Err(err) => {
                checks.push(json!({
                    "key": "doctor",
                    "ok": false,
                    "remedy": err.remedy(),
                    "message": err.to_string(),
                }));
            }
        }

        let healthy = checks.iter().all(|c| c["ok"] == true);
        Ok(CallToolResult::structured(json!({
            "status": if healthy { "healthy" } else { "degraded" },
            "project": project,
            "checks": checks,
        })))
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "memlayer-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "memlayer local memory tools. Prefer memory_search / memory_recent / \
                 memory_context for reads and memory_add for writes. Call memory_health \
                 only after another tool errors — the daemon auto-starts on first use.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> MemoryServer {
        MemoryServer::new(
            PathBuf::from("/nonexistent/memlayer-test.sock"),
            "test-project".to_string(),
            "test-client@0".to_string(),
        )
    }

    #[test]
    fn serve_starts_without_daemon() {
        let s = server();
        assert_eq!(s.base_project, "test-project");
        assert_eq!(s.client_info, "test-client@0");
    }

    #[test]
    fn format_client_info_joins_name_version() {
        assert_eq!(
            format_client_info(&Implementation::new("claude-code", "1.2.3")),
            "claude-code@1.2.3"
        );
        assert_eq!(
            format_client_info(&Implementation::new("windsurf", "")),
            "windsurf"
        );
    }

    #[test]
    fn clamp_limit_bounds() {
        assert_eq!(clamp_limit(None, 10), 10);
        assert_eq!(clamp_limit(Some(0), 10), 1);
        assert_eq!(clamp_limit(Some(100), 10), 50);
    }
}
