//! memlayer local stdio MCP server.
//!
//! Thin adapter that speaks the Model Context Protocol over stdio and
//! translates the six `memory_*` tools into gRPC calls against the local
//! memlayer daemon (via `memlayer-client`). No new daemon RPCs are
//! introduced; every tool maps onto an existing one.
//!
//! Module layout (populated across implementation tasks):
//! - `error`  — `McpError` + `tonic::Status` mapping (mcp-t2)
//! - `scope`  — cwd/base project + per-call override resolution (mcp-t2)
//! - `tools`  — typed arg structs, registry, tool handlers (mcp-t3+)
//! - `client` — lazily-connected, retrying daemon client (mcp-t4)
//! - `server` — rmcp stdio server + event loop (mcp-t4)
//! - `render` — `Observation` -> structured MCP content (mcp-t6)

pub mod error;
pub mod scope;
pub mod server;
pub mod tools;

/// Entry point invoked by the `memlayer mcp` subcommand.
///
/// Starts the stdio MCP server for the given daemon `socket_path`, scoped by
/// default to `project`, tagging writes with `client_info` (the MCP
/// `clientInfo` `name@version`). Runs until stdin closes.
///
/// Currently a stub; the rmcp-backed implementation lands in mcp-t4 once the
/// `rmcp` dependency is fetched.
pub async fn serve(
    _socket_path: std::path::PathBuf,
    _project: String,
    _client_info: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    unimplemented!("rmcp stdio server implemented in task mcp-t4")
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds_and_exports_serve() {
        // Compile-time check that `serve` exists and is referenceable.
        let _ = super::serve;
    }
}
