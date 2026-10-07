//! Lazily-connected, cached daemon client with retry + optional recovery.
//!
//! The MCP server may start before the daemon is reachable (so
//! `memory_health` can report a dead daemon). We therefore defer the gRPC
//! connection until the first tool call that needs it, then cache the client.
//!
//! The target is an [`Endpoint`] — local UDS (default) or a remote TCP+TLS
//! team daemon (Phase 4.1). Both resolve to one uniform [`AuthClient`] type.
//!
//! Failure handling per [`Self::call`]:
//! 1. `SocketMissing` / dial failure → one [`Recovery::recover`] then re-dial
//!    (LOCAL endpoints only — a remote daemon can't be auto-respawned).
//! 2. RPC `UNAVAILABLE` → invalidate cache, one [`Recovery::recover`] (local),
//!    re-dial, retry once.
//!
//! Recover runs at most once per `call` so a permanently dead daemon cannot loop.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use statefulmemory_client::{connect, AuthClient, ClientError, Endpoint};
use tokio::sync::Mutex;

use crate::error::McpError;

/// Best-effort mid-session daemon repair (clear stale socket + respawn).
///
/// Implementations must be idempotent and should bound their runtime —
/// `statefulmemory_client::ensure_running` already has a 5 s spawn budget.
// See `statefulmemory-extract::claude_cli::ClaudeClient` for why
// `double_must_use` is allowed on an `async_trait` trait.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait Recovery: Send + Sync {
    async fn recover(&self) -> Result<(), McpError>;
}

/// Holds the daemon [`Endpoint`] and a cached client, built on first use.
pub struct LazyClient {
    endpoint: Endpoint,
    cached: Mutex<Option<AuthClient>>,
    recover: Option<Arc<dyn Recovery>>,
}

impl LazyClient {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            cached: Mutex::new(None),
            recover: None,
        }
    }

    pub fn with_recovery(endpoint: Endpoint, recover: Arc<dyn Recovery>) -> Self {
        Self {
            endpoint,
            cached: Mutex::new(None),
            recover: Some(recover),
        }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Drop any cached client so the next [`Self::get`] re-dials.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    /// Run recover at most once for this call path. No-op for remote
    /// endpoints — a remote daemon isn't locally respawnable.
    async fn try_recover(&self) -> Result<(), McpError> {
        if self.endpoint.is_remote() {
            return Ok(());
        }
        match &self.recover {
            Some(r) => r.recover().await,
            None => Ok(()),
        }
    }

    /// Return a connected client, building and caching it on first call.
    ///
    /// On dial failure for a LOCAL endpoint, invokes [`Recovery::recover`] at
    /// most once (when configured) and re-dials. For a REMOTE endpoint the
    /// dial error is surfaced directly (no local respawn).
    pub async fn get(&self) -> Result<AuthClient, McpError> {
        let mut guard = self.cached.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        match dial(&self.endpoint).await {
            Ok(client) => {
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(first) => {
                if self.recover.is_none() || self.endpoint.is_remote() {
                    return Err(first);
                }
                drop(guard);
                let recovery_error = self.try_recover().await.err();
                let mut guard = self.cached.lock().await;
                if let Some(client) = guard.as_ref() {
                    return Ok(client.clone());
                }
                let client = dial(&self.endpoint)
                    .await
                    .map_err(|dial_error| recovery_error.unwrap_or(dial_error))?;
                *guard = Some(client.clone());
                Ok(client)
            }
        }
    }

    /// Run an RPC against a cloned client, recovering the daemon at most once
    /// on `UNAVAILABLE` or a missing socket (local endpoints only).
    pub async fn call<F, Fut, T>(&self, mut op: F) -> Result<T, McpError>
    where
        F: FnMut(AuthClient) -> Fut,
        Fut: Future<Output = Result<T, tonic::Status>>,
    {
        let client = match self.get().await {
            Ok(c) => c,
            Err(e) => return Err(e),
        };
        match op(client.clone()).await {
            Ok(v) => Ok(v),
            Err(status) if status.code() == tonic::Code::Unavailable => {
                // Daemon died mid-session (or restarted): repair once, re-dial, retry.
                self.invalidate().await;
                let recovery_error = self.try_recover().await.err();
                let client = self.get_after_recover().await?;
                match op(client).await {
                    Ok(value) => Ok(value),
                    Err(status)
                        if status.code() == tonic::Code::Unavailable
                            && recovery_error.is_some() =>
                    {
                        Err(recovery_error.expect("recovery error checked"))
                    }
                    Err(status) => Err(McpError::from(status)),
                }
            }
            Err(status) => Err(McpError::from(status)),
        }
    }

    /// Dial without invoking recover again (already attempted for this call).
    async fn get_after_recover(&self) -> Result<AuthClient, McpError> {
        let mut guard = self.cached.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        let client = dial(&self.endpoint).await?;
        *guard = Some(client.clone());
        Ok(client)
    }
}

async fn dial(endpoint: &Endpoint) -> Result<AuthClient, McpError> {
    connect(endpoint).await.map_err(|e| match e {
        ClientError::SocketNotFound(path) => McpError::SocketMissing {
            path: path.display().to_string(),
        },
        _ => McpError::DaemonNotRunning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn uds(path: &str) -> Endpoint {
        Endpoint::Uds(PathBuf::from(path))
    }

    struct CountingRecovery {
        hits: AtomicUsize,
        fail: bool,
    }

    #[async_trait]
    impl Recovery for CountingRecovery {
        async fn recover(&self) -> Result<(), McpError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(McpError::Recovery {
                    message: "test recovery failure".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn lazy_client_defers_connection() {
        let lc = LazyClient::new(uds("/nonexistent/statefulmemory-test.sock"));
        assert!(matches!(lc.endpoint(), Endpoint::Uds(_)));
        let err = lc.get().await.expect_err("missing socket should error");
        assert!(matches!(err, McpError::SocketMissing { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn get_attempts_recovery_on_missing_socket() {
        let rec = Arc::new(CountingRecovery {
            hits: AtomicUsize::new(0),
            fail: true,
        });
        let lc = LazyClient::with_recovery(uds("/nonexistent/statefulmemory-test.sock"), rec.clone());
        let err = lc.get().await.expect_err("still missing after recover");
        assert!(matches!(err, McpError::Recovery { .. }), "{err:?}");
        assert_eq!(rec.hits.load(Ordering::SeqCst), 1, "recover once per get");
    }

    #[tokio::test]
    async fn get_without_recovery_does_not_recover() {
        let lc = LazyClient::new(uds("/nonexistent/statefulmemory-test.sock"));
        let err = lc.get().await.expect_err("missing socket");
        assert!(matches!(err, McpError::SocketMissing { .. }));
    }

    #[tokio::test]
    async fn call_recovers_once_on_unavailable() {
        let rec = Arc::new(CountingRecovery {
            hits: AtomicUsize::new(0),
            fail: false,
        });
        let lc = LazyClient::with_recovery(uds("/nonexistent/statefulmemory-test.sock"), rec.clone());
        let _ = lc.call(|_c| async { Ok::<_, tonic::Status>(1_i32) }).await;
        assert_eq!(
            rec.hits.load(Ordering::SeqCst),
            1,
            "exactly one recover attempt per call when dial keeps failing"
        );
    }

    #[tokio::test]
    async fn remote_endpoint_skips_recovery() {
        // A remote TCP endpoint with a bad CA can't be locally respawned —
        // recover must NOT fire, and the dial error surfaces directly.
        let rec = Arc::new(CountingRecovery {
            hits: AtomicUsize::new(0),
            fail: false,
        });
        let ep = Endpoint::Tcp {
            addr: "127.0.0.1:59999".into(),
            ca_pem: b"-----BEGIN CERTIFICATE-----\nbad\n-----END CERTIFICATE-----\n".to_vec(),
            token: "t".into(),
            domain: "localhost".into(),
        };
        let lc = LazyClient::with_recovery(ep, rec.clone());
        let _ = lc.get().await.expect_err("bad CA → dial fails");
        assert_eq!(rec.hits.load(Ordering::SeqCst), 0, "no recovery for remote");
    }
}
