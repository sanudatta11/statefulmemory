//! The MCP server struct and its tool registry.
//!
//! `MemoryServer` hosts the six `memory_*` tools via rmcp's `#[tool_router]`.
//! Tool bodies are stubs here (mcp-t3); the daemon client wiring lands in
//! mcp-t4 and the per-tool RPC logic in mcp-t6..t9.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};

use crate::tools::{AddArgs, ContextArgs, FactsArgs, HealthArgs, RecentArgs, SearchArgs};

/// Local stdio MCP server exposing memlayer memory operations as tools.
#[derive(Clone)]
pub struct MemoryServer {
    /// cwd-derived project used when a tool call omits `project`.
    pub(crate) base_project: String,
    /// MCP client identity ("name@version"), recorded as `created_by` on writes.
    pub(crate) client_info: String,
}

impl MemoryServer {
    pub fn new(base_project: String, client_info: String) -> Self {
        Self {
            base_project,
            client_info,
        }
    }
}

#[tool_router(vis = "pub")]
impl MemoryServer {
    /// Search stored project memory. Modes: "bm25" (default) or "hybrid"
    /// (BM25 + dense, RRF-fused). The daemon auto-starts on first use, so do
    /// not call memory_health first; only call memory_health if a tool errors.
    #[tool]
    async fn memory_search(
        &self,
        Parameters(_args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Err(ErrorData::internal_error(
            "memory_search not yet implemented (mcp-t6)",
            None,
        ))
    }

    /// Save an observation (decision/fix/pattern/note/feedback) to project
    /// memory. The daemon auto-starts on first use; the calling agent is
    /// recorded automatically. Returns the new observation id.
    #[tool]
    async fn memory_add(
        &self,
        Parameters(_args): Parameters<AddArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Err(ErrorData::internal_error(
            "memory_add not yet implemented (mcp-t8)",
            None,
        ))
    }

    /// Get session context from project memory. With no query, returns the
    /// session briefing (recent + pending); with a query, returns topic-ranked
    /// context. The daemon auto-starts on first use.
    #[tool]
    async fn memory_context(
        &self,
        Parameters(_args): Parameters<ContextArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Err(ErrorData::internal_error(
            "memory_context not yet implemented (mcp-t7)",
            None,
        ))
    }

    /// List the atomic facts extracted from a given observation. Returns an
    /// empty list (not an error) when the observation has no extracted facts.
    #[tool]
    async fn memory_facts(
        &self,
        Parameters(_args): Parameters<FactsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Err(ErrorData::internal_error(
            "memory_facts not yet implemented (mcp-t7)",
            None,
        ))
    }

    /// List the most recent observations for the project. The daemon
    /// auto-starts on first use.
    #[tool]
    async fn memory_recent(
        &self,
        Parameters(_args): Parameters<RecentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Err(ErrorData::internal_error(
            "memory_recent not yet implemented (mcp-t6)",
            None,
        ))
    }

    /// Check daemon health and get keyed remediation. Call this only when
    /// another tool has reported an error; it reports whether the daemon is
    /// running and, if not, exactly how to fix it.
    #[tool]
    async fn memory_health(
        &self,
        Parameters(_args): Parameters<HealthArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Err(ErrorData::internal_error(
            "memory_health not yet implemented (mcp-t9)",
            None,
        ))
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> MemoryServer {
        MemoryServer::new("test-project".to_string(), "test-client@0".to_string())
    }

    #[test]
    fn serve_starts_without_daemon() {
        // Constructing the server must not touch the daemon.
        let s = server();
        assert_eq!(s.base_project, "test-project");
    }
}
