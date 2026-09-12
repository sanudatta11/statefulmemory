//! The MCP server struct and its tool registry.
//!
//! `MemoryServer` hosts the seven `memory_*` tools via rmcp's `#[tool_router]`.
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
use crate::tools::{AddArgs, ContextArgs, DecideArgs, FactsArgs, HealthArgs, RecentArgs, SearchArgs};

fn default_search_mode() -> String {
    memlayer_core::config::load_resolved(None).search.mode
}

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
///
/// Antigravity CLI opens with `server/discover` (often without 2026-07-28
/// `_meta`). rmcp 3 answers `-32602` then aborts `serve()`, so the client's
/// follow-up `initialize` hits EOF. We bridge stdio and answer meta-less
/// discover locally so the connection stays open for legacy initialize.
pub async fn serve(
    socket_path: PathBuf,
    project: String,
    client_info: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = MemoryServer::new(socket_path, project, client_info);

    let (to_server, server_read) = tokio::io::duplex(64 * 1024);
    let (server_write, from_server) = tokio::io::duplex(64 * 1024);

    let bridge = tokio::spawn(async move {
        if let Err(e) = bridge_stdio_for_antigravity(to_server, from_server).await {
            // stdin/stdout closed mid-flight is normal shutdown; log others.
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                tracing::debug!("mcp stdio bridge ended: {e}");
            }
        }
    });

    let result = async {
        let running = server.serve((server_read, server_write)).await?;
        running.waiting().await?;
        Ok(())
    }
    .await;

    bridge.abort();
    result
}

/// Required `_meta` keys for a modern `server/discover` (MCP 2026-07-28).
const DISCOVER_META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const DISCOVER_META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

/// True when this line is `server/discover` missing required `_meta` — answer
/// locally instead of forwarding into rmcp's fatal pre-init path.
fn is_meta_less_discover(line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return false;
    };
    if v.get("method").and_then(|m| m.as_str()) != Some("server/discover") {
        return false;
    }
    let Some(meta) = v.pointer("/params/_meta").and_then(|m| m.as_object()) else {
        return true;
    };
    let version_ok = meta.get(DISCOVER_META_PROTOCOL_VERSION).is_some_and(|v| {
        v.as_str().is_some_and(|s| !s.is_empty())
            || v.get("name").and_then(|n| n.as_str()).is_some_and(|s| !s.is_empty())
    });
    let caps_ok = meta
        .get(DISCOVER_META_CLIENT_CAPABILITIES)
        .is_some_and(|v| v.is_object());
    !(version_ok && caps_ok)
}

fn discover_missing_meta_error(id: serde_json::Value) -> String {
    let resp = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": "request _meta is missing or has malformed required fields: io.modelcontextprotocol/protocolVersion, io.modelcontextprotocol/clientCapabilities"
        }
    });
    format!("{resp}\n")
}

fn jsonrpc_id(line: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(line.trim())
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(serde_json::Value::Null)
}

async fn bridge_stdio_for_antigravity(
    mut to_server: tokio::io::DuplexStream,
    mut from_server: tokio::io::DuplexStream,
) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line_buf = String::new();
    let mut server_buf = vec![0u8; 64 * 1024];

    loop {
        tokio::select! {
            read = stdin.read_line(&mut line_buf) => {
                let n = read?;
                if n == 0 {
                    break;
                }
                let line = std::mem::take(&mut line_buf);
                if is_meta_less_discover(&line) {
                    let resp = discover_missing_meta_error(jsonrpc_id(&line));
                    stdout.write_all(resp.as_bytes()).await?;
                    stdout.flush().await?;
                    continue;
                }
                to_server.write_all(line.as_bytes()).await?;
                // DuplexStream has no flush; write is enough for in-memory.
            }
            read = from_server.read(&mut server_buf) => {
                let n = read?;
                if n == 0 {
                    break;
                }
                stdout.write_all(&server_buf[..n]).await?;
                stdout.flush().await?;
            }
        }
    }
    Ok(())
}

#[tool_router(vis = "pub")]
impl MemoryServer {
    /// Search stored project memory. Default mode is hybrid (config
    /// `search.mode`); pass "bm25" to force lexical-only.
    /// The daemon auto-starts on first use, so do not call memory_health
    /// first; only call memory_health if a tool errors.
    /// A result carrying `supersedes_ids` is the in-force value; superseded
    /// values are intentionally withheld from search results.
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
            mode: Some(args.mode.unwrap_or_else(default_search_mode)),
            rerank: args.rerank,
            max_tokens: args.max_tokens,
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
            "tokens_used": resp.tokens_used,
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
            tool_name: args.anchor.clone(),
            scope: "project".into(),
            created_by: Some(created_by),
            topic_key: None,
            code_anchor: args.anchor.clone(),
            anchors: args.anchor.iter().cloned().collect(),
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
            "warnings": resp.warnings,
        })))
    }

    /// Get session context from project memory. With no query, returns the
    /// session briefing (recent + pending); with a query, returns topic-ranked
    /// context. The daemon auto-starts on first use.
    /// A result carrying `supersedes_ids` is the in-force value; superseded
    /// values are intentionally withheld.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn memory_context(
        &self,
        Parameters(args): Parameters<ContextArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        let req = memlayer_proto::ContextRequest {
            project_name: project,
            recent_limit: clamp_limit(args.limit, 10),
            mode: Some(args.mode.unwrap_or_else(default_search_mode)),
            rerank: args.rerank,
            query: args.query.filter(|q| !q.trim().is_empty()),
            anchor: None,
            include_stale: false,
            max_tokens: args.max_tokens,
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
            "tokens_used": resp.tokens_used,
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

    /// Recommend a decision from stored memories; auto-runs the resolution
    /// judge on open conflicts. Use after memory_search when the user must
    /// choose between conflicting notes. The daemon auto-starts on first use.
    #[tool(annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false))]
    async fn memory_decide(
        &self,
        Parameters(args): Parameters<DecideArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project = resolve_project(&self.base_project, args.project.as_deref())?;
        if args.question.trim().is_empty() {
            return Err(McpError::BadArgs {
                message: "question must not be empty".into(),
            }
            .into());
        }
        let req = memlayer_proto::DecideRequest {
            project_name: project,
            question: args.question,
            limit: clamp_limit(args.limit, 12),
            mode: None,
        };
        let resp = self
            .client
            .call(|mut c| {
                let req = req.clone();
                async move { c.decide(req).await.map(|r| r.into_inner()) }
            })
            .await?;
        Ok(CallToolResult::structured(json!({
            "recommendation": resp.recommendation,
            "rationale": resp.rationale,
            "confidence": resp.confidence,
            "evidence": resp.evidence.iter().map(|e| json!({
                "id": e.observation_id,
                "title": e.title,
                "role": e.role,
            })).collect::<Vec<_>>(),
            "conflicts": resp.conflicts.iter().map(|c| json!({
                "a_id": c.a_id,
                "b_id": c.b_id,
                "status": c.status,
            })).collect::<Vec<_>>(),
            "wrote_resolution": resp.wrote_resolution,
            "resolution_observation_id": resp.resolution_observation_id,
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
                 memory_context for reads, memory_add for writes, and memory_decide \
                 when the user must choose between conflicting notes. Call memory_health \
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

    #[test]
    fn meta_less_discover_detection() {
        assert!(is_meta_less_discover(
            r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}"#
        ));
        assert!(is_meta_less_discover(
            r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{}}}"#
        ));
        assert!(!is_meta_less_discover(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#
        ));
        let with_meta = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;
        assert!(!is_meta_less_discover(with_meta));
    }

    /// Antigravity: meta-less discover must not kill the session — answer
    /// locally, then accept legacy `initialize` on the same connection.
    #[tokio::test]
    async fn meta_less_discover_then_initialize_survives() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (mut client_wr, bridge_rd) = tokio::io::duplex(64 * 1024);
        let (mut bridge_wr, client_rd) = tokio::io::duplex(64 * 1024);
        let (mut to_server, server_rd) = tokio::io::duplex(64 * 1024);
        let (server_wr, mut from_server) = tokio::io::duplex(64 * 1024);

        // Bridge: client <-> (filter) <-> server duplexes, mirroring stdio bridge.
        let bridge = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut line_buf = String::new();
            let mut server_buf = vec![0u8; 64 * 1024];
            let mut bridge_rd = BufReader::new(bridge_rd);
            loop {
                tokio::select! {
                    read = bridge_rd.read_line(&mut line_buf) => {
                        let n = read.expect("client read");
                        if n == 0 { break; }
                        let line = std::mem::take(&mut line_buf);
                        if is_meta_less_discover(&line) {
                            let resp = discover_missing_meta_error(jsonrpc_id(&line));
                            bridge_wr.write_all(resp.as_bytes()).await.unwrap();
                            continue;
                        }
                        to_server.write_all(line.as_bytes()).await.unwrap();
                    }
                    read = from_server.read(&mut server_buf) => {
                        let n = read.expect("server read");
                        if n == 0 { break; }
                        bridge_wr.write_all(&server_buf[..n]).await.unwrap();
                    }
                }
            }
        });

        let server = server();
        let serve = tokio::spawn(async move {
            use rmcp::ServiceExt;
            let running = server.serve((server_rd, server_wr)).await.expect("serve");
            let _ = running.waiting().await;
        });

        // 1) Antigravity probe — must get JSON-RPC error, not EOF.
        client_wr
            .write_all(
                br#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}
"#,
            )
            .await
            .unwrap();

        let mut lines = BufReader::new(client_rd).lines();
        let line1 = tokio::time::timeout(std::time::Duration::from_secs(3), lines.next_line())
            .await
            .expect("timeout")
            .unwrap()
            .expect("reply");
        let v1: serde_json::Value = serde_json::from_str(&line1).unwrap();
        assert_eq!(v1["error"]["code"], -32602);

        // 2) Legacy initialize on the same connection — must succeed.
        client_wr
            .write_all(
                br#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"antigravity-cli","version":"1"}}}
"#,
            )
            .await
            .unwrap();

        let line2 = tokio::time::timeout(std::time::Duration::from_secs(3), lines.next_line())
            .await
            .expect("timeout")
            .unwrap()
            .expect("initialize reply");
        let v2: serde_json::Value = serde_json::from_str(&line2).unwrap();
        assert!(
            v2.get("result").is_some(),
            "expected initialize result, got {line2}"
        );
        assert!(v2["result"]["serverInfo"]["name"] == "memlayer-mcp" || v2["result"].get("capabilities").is_some());

        drop(client_wr);
        let _ = serve.await;
        bridge.abort();
    }
}
