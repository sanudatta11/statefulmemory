//! `tonic::transport::Channel` builders for the two supported transports.
//!
//! Spec sections: FR2, §11.1.
//!
//! - [`connect_uds`] is synchronous and lazy: it pre-checks the socket file
//!   exists (so callers fail fast in EH-5/SC-2 paths), then returns a Channel
//!   that establishes the actual stream on first RPC.
//! - [`connect_tcp`] is async and eager: it performs the TLS handshake during
//!   build so an invalid CA bundle is surfaced before any RPC is issued.

use std::path::{Path, PathBuf};

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tower::service_fn;

use crate::ClientError;

// tonic requires a syntactically-valid http(s) URI even when a custom
// connector is used. The host portion is never resolved — UnixStream
// short-circuits to the path provided to the connector.
const UDS_PLACEHOLDER_URI: &str = "http://memlayer.local";

/// Build a lazy `Channel` connected to the daemon over UDS.
///
/// Returns `Err(SocketNotFound)` immediately if the path does not exist on
/// disk. This is the FR2.3 fast-fail before auto-spawn: callers detect the
/// missing socket synchronously and decide whether to acquire the spawn lock.
///
/// On success, the returned Channel is lazy: the first RPC opens the
/// `UnixStream`. RPC errors (including the daemon dying mid-call) surface as
/// `tonic::Status::Unavailable` and can be retried via [`crate::with_retry`].
pub fn connect_uds(socket: impl AsRef<Path>) -> Result<Channel, ClientError> {
    let socket: PathBuf = socket.as_ref().to_path_buf();
    if !socket.exists() {
        return Err(ClientError::SocketNotFound(socket));
    }
    let endpoint = Endpoint::from_static(UDS_PLACEHOLDER_URI);
    let channel = endpoint.connect_with_connector_lazy(service_fn(move |_uri: tonic::transport::Uri| {
        let path = socket.clone();
        async move { UnixStream::connect(&path).await.map(TokioIo::new) }
    }));
    Ok(channel)
}

/// Build a `Channel` connected over TCP+TLS.
///
/// The CA PEM is the file produced by `memlayer team init-ca`. `domain` must
/// match the leaf cert's SAN (typically `localhost` for a self-signed CA).
/// The bearer token is applied at the *client* level (not the channel) — see
/// [`crate::interceptor::bearer_interceptor`].
pub async fn connect_tcp(
    addr: &str,
    ca_pem: &[u8],
    domain: &str,
) -> Result<Channel, ClientError> {
    let endpoint_url = if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("https://{addr}")
    };
    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca_pem))
        .domain_name(domain.to_string());
    let endpoint = Endpoint::from_shared(endpoint_url)
        .map_err(ClientError::Endpoint)?
        .tls_config(tls)
        .map_err(ClientError::Tls)?;
    endpoint.connect().await.map_err(ClientError::Connect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn uds_connect_fails_fast_when_absent() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("does-not-exist.sock");
        let err = connect_uds(&nonexistent).expect_err("expected SocketNotFound");
        match err {
            ClientError::SocketNotFound(p) => assert_eq!(p, nonexistent),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn uds_connect_succeeds_when_socket_present() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("daemon.sock");
        // The fast-fail check in `connect_uds` only verifies the path exists.
        // We materialize a placeholder file rather than binding a real
        // UnixListener — sandboxed test environments routinely deny `bind(2)`
        // on arbitrary paths, and the lazy connector never touches the file
        // during channel construction. End-to-end UDS connectivity is covered
        // by the integration suite (TS-1). The test runs inside a tokio
        // runtime because tonic's lazy channel initializes a `TokioExecutor`
        // for the eventual h2 driver.
        std::fs::File::create(&sock).expect("create placeholder socket file");
        let _channel = connect_uds(&sock).expect("channel build should succeed");
    }
}
