//! memlayer local stdio MCP server.
//!
//! Thin adapter that speaks the Model Context Protocol over stdio and
//! translates the seven `memory_*` tools into gRPC calls against the local
//! memlayer daemon (via `memlayer-client`). No new daemon RPCs are
//! introduced; every tool maps onto an existing one.
//!
//! Module layout:
//! - `error`  — `McpError` + `tonic::Status` mapping
//! - `scope`  — cwd/base project + per-call override resolution
//! - `tools`  — typed arg structs for the seven tools
//! - `client` — lazily-connected, retrying daemon client
//! - `server` — rmcp stdio server + tool registry + `serve()` entry point
//! - `render` — `Observation` / `Fact` → structured MCP JSON content

pub mod client;
pub mod error;
pub mod render;
pub mod scope;
pub mod server;
pub mod tools;

pub use server::serve;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds_and_exports_serve() {
        // Compile-time check that `serve` exists and is referenceable.
        let _ = super::serve;
    }
}
