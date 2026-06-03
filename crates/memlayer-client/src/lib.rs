//! memlayer gRPC client utilities used by every CLI subcommand.
//!
//! Spec sections: FR2, §11.1.
//!
//! Two transports are supported:
//!
//! - **UDS** (`~/.memlayer/daemon.sock`): default for single-user mode. The
//!   filesystem permissions on the socket are the auth surface — no bearer
//!   token. See [`channel::connect_uds`].
//! - **TCP + TLS**: team mode. A self-signed CA (`team init-ca`) issues a leaf
//!   cert for the server. The client validates against the CA and presents a
//!   bearer token via the [`interceptor::bearer_interceptor`] middleware.
//!
//! Higher-level CLI command modules build a [`MemlayerClient`] from the
//! returned [`tonic::transport::Channel`] and (for TCP) wrap it with the
//! interceptor via `MemlayerClient::with_interceptor`.

pub mod channel;
pub mod interceptor;

use std::path::PathBuf;
use thiserror::Error;

pub use memlayer_proto::memlayer_client::MemlayerClient;

/// Errors surfaced by channel construction. RPC-level errors flow through as
/// `tonic::Status` and are mapped to exit codes by the CLI's exit module.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("daemon socket not found at {0}: is the daemon running?")]
    SocketNotFound(PathBuf),

    #[error("endpoint configuration: {0}")]
    Endpoint(#[source] tonic::transport::Error),

    #[error("TLS configuration: {0}")]
    Tls(#[source] tonic::transport::Error),

    #[error("connect: {0}")]
    Connect(#[source] tonic::transport::Error),
}

/// Run an RPC closure once, retrying a single time if the first attempt fails
/// with `UNAVAILABLE` (FR2: idempotent reconnect across a daemon restart).
///
/// The closure is `FnMut` so the caller can re-clone a `Channel` or `Request`
/// inside it on each attempt — `tonic` clients take `&mut self`, and bodies
/// like `prost`-encoded messages are not re-readable after a failed call.
pub async fn with_retry<F, Fut, T>(mut op: F) -> Result<T, tonic::Status>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, tonic::Status>>,
{
    match op().await {
        Ok(v) => Ok(v),
        Err(s) if s.code() == tonic::Code::Unavailable => op().await,
        Err(s) => Err(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tonic::Status;

    #[tokio::test]
    async fn retry_on_unavailable_succeeds_second_attempt() {
        let count = AtomicUsize::new(0);
        let result = with_retry(|| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err(Status::unavailable("first attempt fails"))
                } else {
                    Ok(42_i32)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_does_not_retry_on_other_errors() {
        let count = AtomicUsize::new(0);
        let result: Result<i32, _> = with_retry(|| {
            count.fetch_add(1, Ordering::SeqCst);
            async move { Err(Status::not_found("nope")) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_returns_last_unavailable_after_two_attempts() {
        let count = AtomicUsize::new(0);
        let result: Result<i32, _> = with_retry(|| {
            count.fetch_add(1, Ordering::SeqCst);
            async move { Err(Status::unavailable("still down")) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
}
