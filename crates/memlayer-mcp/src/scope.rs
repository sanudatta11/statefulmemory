//! Project-scope resolution for MCP tool calls.
//!
//! Every tool is scoped to a project. By default that is the server's base
//! project (derived from the launch cwd by the `memlayer mcp` subcommand).
//! A tool may pass an explicit `project` to override it for that call only.

use crate::error::McpError;

/// Resolve the effective project name for a tool call.
///
/// `base` is the server's cwd-derived project. `over` is an optional
/// per-call override. A present-but-empty override is a caller error rather
/// than a silent fallback (so a typo is not masked).
pub fn resolve_project(base: &str, over: Option<&str>) -> Result<String, McpError> {
    match over {
        Some(p) if !p.trim().is_empty() => Ok(p.trim().to_string()),
        Some(_) => Err(McpError::BadArgs {
            message: "project override must not be empty".to_string(),
        }),
        None => Ok(base.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_beats_base() {
        assert_eq!(
            resolve_project("base-repo", Some("other-repo")).unwrap(),
            "other-repo"
        );
        // Whitespace is trimmed.
        assert_eq!(resolve_project("base-repo", Some("  x ")).unwrap(), "x");
        // No override falls back to base.
        assert_eq!(resolve_project("base-repo", None).unwrap(), "base-repo");
    }

    #[test]
    fn empty_override_is_error() {
        assert!(resolve_project("base-repo", Some("")).is_err());
        assert!(resolve_project("base-repo", Some("   ")).is_err());
    }
}
