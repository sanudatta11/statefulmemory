//! Lazily-connected, cached daemon client.
//!
//! The MCP server may start before the daemon is reachable (so
//! `memory_health` can report a dead daemon). We therefore defer the gRPC
//! connection until the first tool call that needs it, then cache the
//! channel. `connect_uds` returns a lazy tonic `Channel`, so a successful
//! `get()` does not guarantee the daemon is live — transient failures on the
//! actual RPC are retried once via `memlayer_client::with_retry`.

use std::path::{Path, PathBuf};

use memlayer_client::channel::connect_uds;
use memlayer_client::ClientError;
use memlayer_proto::memlayer_client::MemlayerClient;
use tokio::sync::Mutex;
use tonic::transport::Channel;

use crate::error::McpError;

/// Holds the daemon socket path and a cached client, built on first use.
pub struct LazyClient {
    socket_path: PathBuf,
    cached: Mutex<Option<MemlayerClient<Channel>>>,
}

impl LazyClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            cached: Mutex::new(None),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Return a connected client, building and caching it on first call.
    ///
    /// Maps connection failures to keyed [`McpError`]s so callers (and
    /// `memory_health`) can classify the problem.
    pub async fn get(&self) -> Result<MemlayerClient<Channel>, McpError> {
        let mut guard = self.cached.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        let channel = connect_uds(&self.socket_path).map_err(|e| match e {
            ClientError::SocketNotFound(path) => McpError::SocketMissing {
                path: path.display().to_string(),
            },
            _ => McpError::DaemonNotRunning,
        })?;
        let client = MemlayerClient::new(channel);
        *guard = Some(client.clone());
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lazy_client_defers_connection() {
        // Constructing against a nonexistent socket must not fail...
        let lc = LazyClient::new(PathBuf::from("/nonexistent/memlayer-test.sock"));
        assert_eq!(
            lc.socket_path(),
            Path::new("/nonexistent/memlayer-test.sock")
        );
        // ...the missing socket only surfaces when we actually try to connect.
        let err = lc.get().await.expect_err("missing socket should error");
        assert!(matches!(err, McpError::SocketMissing { .. }), "{err:?}");
    }
}
