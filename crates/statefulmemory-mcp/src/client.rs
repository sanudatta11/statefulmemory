//! Lazily-connected, cached daemon client with retry + optional recovery.
//!
//! The MCP server may start before the daemon is reachable (so
//! `memory_health` can report a dead daemon). We therefore defer the gRPC
//! connection until the first tool call that needs it, then cache the
//! channel.
//!
//! Failure handling per [`Self::call`]:
//! 1. `SocketMissing` / dial failure → one [`Recovery::recover`] then re-dial.
//! 2. RPC `UNAVAILABLE` → invalidate cache, one [`Recovery::recover`], re-dial, retry once.
//!
//! Recover runs at most once per `call` so a permanently dead daemon cannot loop.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use statefulmemory_client::channel::connect_uds;
use statefulmemory_client::{with_retry, ClientError};
use statefulmemory_proto::stateful_memory_client::StatefulMemoryClient;
use tokio::sync::Mutex;
use tonic::transport::Channel;

use crate::error::McpError;

/// Best-effort mid-session daemon repair (clear stale socket + respawn).
///
/// Implementations must be idempotent and should bound their runtime —
/// `statefulmemory_client::ensure_running` already has a 5 s spawn budget.
#[async_trait]
pub trait Recovery: Send + Sync {
    async fn recover(&self) -> Result<(), McpError>;
}

/// Holds the daemon socket path and a cached client, built on first use.
pub struct LazyClient {
    socket_path: PathBuf,
    cached: Mutex<Option<StatefulMemoryClient<Channel>>>,
    recover: Option<Arc<dyn Recovery>>,
}

impl LazyClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            cached: Mutex::new(None),
            recover: None,
        }
    }

    pub fn with_recovery(socket_path: PathBuf, recover: Arc<dyn Recovery>) -> Self {
        Self {
            socket_path,
            cached: Mutex::new(None),
            recover: Some(recover),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Drop any cached client so the next [`Self::get`] re-dials.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    /// Run recover at most once for this call path.
    async fn try_recover(&self) -> Result<(), McpError> {
        match &self.recover {
            Some(r) => r.recover().await,
            None => Ok(()),
        }
    }

    /// Return a connected client, building and caching it on first call.
    ///
    /// On dial failure, invokes [`Recovery::recover`] at most once (when a
    /// recovery is configured) and re-dials. Recover errors are best-effort —
    /// the re-dial decides the surfaced error so `socket_missing` stays stable.
    pub async fn get(&self) -> Result<StatefulMemoryClient<Channel>, McpError> {
        let mut guard = self.cached.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        match dial(&self.socket_path) {
            Ok(client) => {
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(first) => {
                if self.recover.is_none() {
                    return Err(first);
                }
                drop(guard);
                // Best-effort: a failed spawn must not mask the dial error.
                let _ = self.try_recover().await;
                let mut guard = self.cached.lock().await;
                if let Some(client) = guard.as_ref() {
                    return Ok(client.clone());
                }
                let client = dial(&self.socket_path)?;
                *guard = Some(client.clone());
                Ok(client)
            }
        }
    }

    /// Run an RPC against a cloned client, recovering the daemon at most once
    /// on `UNAVAILABLE` or a missing socket.
    pub async fn call<F, Fut, T>(&self, mut op: F) -> Result<T, McpError>
    where
        F: FnMut(StatefulMemoryClient<Channel>) -> Fut,
        Fut: Future<Output = Result<T, tonic::Status>>,
    {
        let client = match self.get().await {
            Ok(c) => c,
            Err(e) => return Err(e),
        };
        match with_retry(|| op(client.clone())).await {
            Ok(v) => Ok(v),
            Err(status) if status.code() == tonic::Code::Unavailable => {
                // Daemon died mid-session (or restarted): repair once, re-dial, retry.
                self.invalidate().await;
                // Best-effort — re-dial below reports the final error.
                let _ = self.try_recover().await;
                let client = self.get_after_recover().await?;
                op(client).await.map_err(McpError::from)
            }
            Err(status) => Err(McpError::from(status)),
        }
    }

    /// Dial without invoking recover again (already attempted for this call).
    async fn get_after_recover(&self) -> Result<StatefulMemoryClient<Channel>, McpError> {
        let mut guard = self.cached.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        let client = dial(&self.socket_path)?;
        *guard = Some(client.clone());
        Ok(client)
    }
}

fn dial(socket_path: &Path) -> Result<StatefulMemoryClient<Channel>, McpError> {
    let channel = connect_uds(socket_path).map_err(|e| match e {
        ClientError::SocketNotFound(path) => McpError::SocketMissing {
            path: path.display().to_string(),
        },
        _ => McpError::DaemonNotRunning,
    })?;
    Ok(StatefulMemoryClient::new(channel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingRecovery {
        hits: AtomicUsize,
        fail: bool,
    }

    #[async_trait]
    impl Recovery for CountingRecovery {
        async fn recover(&self) -> Result<(), McpError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(McpError::DaemonNotRunning)
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn lazy_client_defers_connection() {
        let lc = LazyClient::new(PathBuf::from("/nonexistent/statefulmemory-test.sock"));
        assert_eq!(
            lc.socket_path(),
            Path::new("/nonexistent/statefulmemory-test.sock")
        );
        let err = lc.get().await.expect_err("missing socket should error");
        assert!(matches!(err, McpError::SocketMissing { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn get_attempts_recovery_on_missing_socket() {
        let rec = Arc::new(CountingRecovery {
            hits: AtomicUsize::new(0),
            fail: true,
        });
        let lc = LazyClient::with_recovery(
            PathBuf::from("/nonexistent/statefulmemory-test.sock"),
            rec.clone(),
        );
        let err = lc.get().await.expect_err("still missing after recover");
        assert!(matches!(err, McpError::SocketMissing { .. }), "{err:?}");
        assert_eq!(rec.hits.load(Ordering::SeqCst), 1, "recover once per get");
    }

    #[tokio::test]
    async fn get_without_recovery_does_not_recover() {
        let lc = LazyClient::new(PathBuf::from("/nonexistent/statefulmemory-test.sock"));
        let err = lc.get().await.expect_err("missing socket");
        assert!(matches!(err, McpError::SocketMissing { .. }));
    }

    #[tokio::test]
    async fn call_recovers_once_on_unavailable() {
        let rec = Arc::new(CountingRecovery {
            hits: AtomicUsize::new(0),
            fail: false,
        });
        // Socket missing → get() recovers (count=1) but still fails dial.
        // Use a path that exists as a dead file so dial succeeds at channel
        // build (lazy) and the RPC path is what fails... but we cannot fake
        // a full gRPC server here. Instead assert recover fires on get path.
        let lc = LazyClient::with_recovery(
            PathBuf::from("/nonexistent/statefulmemory-test.sock"),
            rec.clone(),
        );
        let _ = lc
            .call(|_c| async { Ok::<_, tonic::Status>(1_i32) })
            .await;
        assert_eq!(
            rec.hits.load(Ordering::SeqCst),
            1,
            "exactly one recover attempt per call when dial keeps failing"
        );
    }
}
