//! Lazily-connected, cached daemon client with one-shot Unavailable retry.
//!
//! The MCP server may start before the daemon is reachable (so
//! `memory_health` can report a dead daemon). We therefore defer the gRPC
//! connection until the first tool call that needs it, then cache the
//! channel. Transient `UNAVAILABLE` (daemon restart) is retried once via
//! [`memlayer_client::with_retry`], then the cache is invalidated and a
//! fresh dial is attempted.

use std::future::Future;
use std::path::{Path, PathBuf};

use memlayer_client::channel::connect_uds;
use memlayer_client::{with_retry, ClientError};
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

    /// Drop any cached client so the next [`Self::get`] re-dials.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    /// Return a connected client, building and caching it on first call.
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

    /// Run an RPC against a cloned client, retrying once on `UNAVAILABLE`.
    ///
    /// If retries still fail with `UNAVAILABLE`, invalidate the cache and
    /// attempt one fresh dial + call so a daemon restart mid-session recovers.
    pub async fn call<F, Fut, T>(&self, mut op: F) -> Result<T, McpError>
    where
        F: FnMut(MemlayerClient<Channel>) -> Fut,
        Fut: Future<Output = Result<T, tonic::Status>>,
    {
        let client = self.get().await?;
        match with_retry(|| op(client.clone())).await {
            Ok(v) => Ok(v),
            Err(status) if status.code() == tonic::Code::Unavailable => {
                self.invalidate().await;
                let client = self.get().await?;
                op(client).await.map_err(McpError::from)
            }
            Err(status) => Err(McpError::from(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lazy_client_defers_connection() {
        let lc = LazyClient::new(PathBuf::from("/nonexistent/memlayer-test.sock"));
        assert_eq!(
            lc.socket_path(),
            Path::new("/nonexistent/memlayer-test.sock")
        );
        let err = lc.get().await.expect_err("missing socket should error");
        assert!(matches!(err, McpError::SocketMissing { .. }), "{err:?}");
    }
}
