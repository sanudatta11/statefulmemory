//! Typed argument structs for the seven `memory_*` MCP tools.
//!
//! Each derives `serde::Deserialize` (for decoding tool-call arguments) and
//! `schemars::JsonSchema` (so rmcp can advertise a JSON-Schema to the agent).
//! The `#[schemars(crate = "rmcp::schemars")]` attribute points the derive at
//! rmcp's re-exported schemars so we need no direct schemars dependency.
//!
//! Every struct with a `project` field allows a per-call scope override; when
//! omitted the server's cwd-derived base project is used
//! (see [`crate::scope::resolve_project`]).

use rmcp::schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchArgs {
    /// Full-text query to search stored memory for.
    pub query: String,
    /// Retrieval mode: "hybrid" (default from config) or "bm25".
    pub mode: Option<String>,
    /// Optional reranker. Omit to use the invoking agent's current model.
    /// Pass a concrete id (e.g. `opencode/glm-5.3`) to pin one.
    pub rerank: Option<String>,
    /// Maximum number of results (default 10).
    pub limit: Option<i32>,
    /// Soft token budget (estimated chars/4). Omit for unlimited.
    pub max_tokens: Option<i32>,
    /// Filter by observation type (e.g. "decision", "fix", "pattern").
    #[serde(rename = "type")]
    pub type_: Option<String>,
    /// Filter by scope: "project", "personal", or "team".
    pub scope: Option<String>,
    /// Project to search; defaults to the server's working-directory project.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AddArgs {
    /// Short title for the observation.
    pub title: String,
    /// Full observation body.
    pub content: String,
    /// Observation type: "decision", "fix", "pattern", "note", "feedback".
    #[serde(rename = "type")]
    pub type_: String,
    /// Session id to associate the observation with (optional).
    pub session: Option<String>,
    /// Optional anchor (e.g. "file::symbol") recorded as the tool_name hint.
    pub anchor: Option<String>,
    /// Project to write to; defaults to the server's working-directory project.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ContextArgs {
    /// Optional topic to rank context by; omit for the session briefing.
    pub query: Option<String>,
    /// Retrieval mode: "bm25" (default) or "hybrid".
    pub mode: Option<String>,
    /// Optional reranker. Omit to use the invoking agent's current model.
    /// Pass a concrete id (e.g. `opencode/glm-5.3`) to pin one.
    pub rerank: Option<String>,
    /// Maximum number of recent observations to include (default 10).
    pub limit: Option<i32>,
    /// Soft token budget (estimated chars/4). Omit for unlimited.
    pub max_tokens: Option<i32>,
    /// Project to read; defaults to the server's working-directory project.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FactsArgs {
    /// Observation id whose extracted atomic facts to return.
    pub observation_id: i64,
    /// Project the observation belongs to; defaults to the cwd project.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RecentArgs {
    /// Maximum number of recent observations to return (default 10).
    pub limit: Option<i32>,
    /// Filter by scope: "project", "personal", or "team".
    pub scope: Option<String>,
    /// Project to read; defaults to the server's working-directory project.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct HealthArgs {
    /// Project to scope the health/doctor check to; defaults to the cwd project.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DecideArgs {
    /// Decision question to answer from stored memories.
    pub question: String,
    /// Maximum memories to retrieve (default 12).
    pub limit: Option<i32>,
    /// Project to read; defaults to the cwd project.
    pub project: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::server::MemoryServer;

    const EXPECTED: [&str; 7] = [
        "memory_search",
        "memory_add",
        "memory_context",
        "memory_facts",
        "memory_recent",
        "memory_health",
        "memory_decide",
    ];

    #[test]
    fn registry_lists_exactly_seven_tools() {
        let names: Vec<String> = MemoryServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(names.len(), 7, "got {names:?}");
        for want in EXPECTED {
            assert!(names.iter().any(|n| n == want), "missing tool {want}");
        }
    }

    #[test]
    fn tool_descriptions_contain_behavior_hints() {
        let tools = MemoryServer::tool_router().list_all();
        let search = tools
            .iter()
            .find(|t| t.name == "memory_search")
            .expect("memory_search registered");
        let desc = search
            .description
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            desc.contains("auto-start"),
            "memory_search description should steer the agent about daemon auto-start: {desc}"
        );
    }

    #[test]
    fn arg_schemas_parse_valid_json() {
        let args: super::SearchArgs =
            serde_json::from_str(r#"{"query":"auth","mode":"hybrid","limit":5}"#).unwrap();
        assert_eq!(args.query, "auth");
        assert_eq!(args.mode.as_deref(), Some("hybrid"));
        assert_eq!(args.limit, Some(5));

        let add: super::AddArgs =
            serde_json::from_str(r#"{"title":"t","content":"c","type":"decision"}"#).unwrap();
        assert_eq!(add.type_, "decision");
    }
}
