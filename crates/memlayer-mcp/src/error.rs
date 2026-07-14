//! Error type for the memlayer MCP server and its mapping to MCP wire errors.
//!
//! Every tool returns `Result<_, rmcp::ErrorData>`. Internally we use
//! [`McpError`], a keyed enum, and convert it into an `ErrorData` whose
//! `data` payload carries a stable `key` (and, where useful, a `remedy`) so
//! agents can branch on the failure class — the same idea as Forecast's
//! keyed health remediation.

use rmcp::ErrorData;
use serde_json::json;

/// Keyed failure classes surfaced by MCP tools.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// The daemon socket is present but the daemon refused/closed the
    /// connection (gRPC `Unavailable`).
    #[error("memlayer daemon is not running")]
    DaemonNotRunning,

    /// No daemon socket file exists at the expected path.
    #[error("daemon socket not found at {path}")]
    SocketMissing { path: String },

    /// A `project` override that could not be used.
    #[error("unknown project: {name}")]
    UnknownProject { name: String },

    /// A daemon RPC returned an error status.
    #[error("daemon RPC failed ({code}): {message}")]
    Rpc { code: String, message: String },

    /// Tool arguments were malformed / unusable.
    #[error("invalid arguments: {message}")]
    BadArgs { message: String },
}

impl McpError {
    /// Stable machine key for this failure class. Also embedded in the MCP
    /// error `data` payload so agents (and `memory_health`) can branch on it.
    pub fn key(&self) -> &'static str {
        match self {
            McpError::DaemonNotRunning => "daemon_not_running",
            McpError::SocketMissing { .. } => "socket_missing",
            McpError::UnknownProject { .. } => "unknown_project",
            McpError::Rpc { .. } => "rpc_error",
            McpError::BadArgs { .. } => "bad_args",
        }
    }

    /// A one-line remediation hint, when one applies.
    pub fn remedy(&self) -> Option<&'static str> {
        match self {
            McpError::DaemonNotRunning => {
                Some("start the daemon with `memlayer daemon start`")
            }
            McpError::SocketMissing { .. } => {
                Some("start the daemon with `memlayer daemon start` to create the socket")
            }
            McpError::UnknownProject { .. } => {
                Some("list known projects with `memlayer project list`")
            }
            _ => None,
        }
    }
}

impl From<tonic::Status> for McpError {
    fn from(status: tonic::Status) -> Self {
        match status.code() {
            tonic::Code::Unavailable => McpError::DaemonNotRunning,
            code => McpError::Rpc {
                code: format!("{code:?}"),
                message: status.message().to_string(),
            },
        }
    }
}

impl From<McpError> for ErrorData {
    fn from(err: McpError) -> Self {
        let data = json!({ "key": err.key(), "remedy": err.remedy() });
        let message = err.to_string();
        match err {
            // Caller-fixable input problems map to invalid_params.
            McpError::BadArgs { .. } | McpError::UnknownProject { .. } => {
                ErrorData::invalid_params(message, Some(data))
            }
            // Environment / daemon problems map to internal_error.
            McpError::DaemonNotRunning
            | McpError::SocketMissing { .. }
            | McpError::Rpc { .. } => ErrorData::internal_error(message, Some(data)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tonic_status_maps_to_mcp_error() {
        let status = tonic::Status::new(tonic::Code::NotFound, "no such observation");
        let err: McpError = status.into();
        match err {
            McpError::Rpc { code, message } => {
                assert_eq!(code, "NotFound");
                assert_eq!(message, "no such observation");
            }
            other => panic!("expected Rpc, got {other:?}"),
        }
        // And it converts into a structured MCP error carrying the key.
        let data: ErrorData = McpError::Rpc {
            code: "NotFound".into(),
            message: "no such observation".into(),
        }
        .into();
        let payload = data.data.expect("data payload");
        assert_eq!(payload["key"], "rpc_error");
    }

    #[test]
    fn unavailable_maps_to_daemon_not_running() {
        let status = tonic::Status::new(tonic::Code::Unavailable, "connection refused");
        let err: McpError = status.into();
        assert!(matches!(err, McpError::DaemonNotRunning));
        assert_eq!(err.key(), "daemon_not_running");
        assert!(err.remedy().is_some());
    }
}
