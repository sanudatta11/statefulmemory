//! Transport endpoint resolution: dial the daemon over local UDS or a remote
//! TCP+TLS team daemon, behind one uniform client type.
//!
//! Phase 4.1. The daemon already supports both transports (UDS default; TCP +
//! TLS + bearer token in "team mode"), and [`channel::connect_uds`] /
//! [`channel::connect_tcp`] already exist. This module ties them together so a
//! single consumer (the MCP server, and later the CLI) can target either a
//! local or a shared/remote knowledge base.
//!
//! The returned client is ALWAYS wrapped with [`AuthInterceptor`], so UDS and
//! TCP share one concrete type ([`AuthClient`]). For UDS the interceptor is a
//! no-op (filesystem permissions are the trust boundary); for TCP it injects
//! `authorization: Bearer <token>`.

use std::path::PathBuf;

use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::{Request, Status};

use crate::channel;
use crate::{ClientError, StatefulMemoryClient};

/// Uniform daemon client type across transports — a `StatefulMemoryClient`
/// whose channel is wrapped with [`AuthInterceptor`] (a no-op for UDS).
pub type AuthClient = StatefulMemoryClient<InterceptedService<Channel, AuthInterceptor>>;

/// tonic interceptor that optionally injects a bearer `authorization` header.
/// `None` (UDS) is a no-op; `Some` (TCP team mode) adds the token on every call.
/// Replaces the standalone `bearer_interceptor` closure for callers that need a
/// single concrete type regardless of transport.
#[derive(Clone)]
pub struct AuthInterceptor {
    header: Option<MetadataValue<Ascii>>,
}

impl AuthInterceptor {
    /// No-op interceptor (UDS): never touches request metadata.
    pub fn none() -> Self {
        Self { header: None }
    }

    /// Bearer interceptor (TCP): injects `authorization: Bearer <token>`.
    ///
    /// # Panics
    /// Panics if `token` is not a valid HTTP header value. Tokens minted by
    /// `statefulmemory team token-create` are 64-char hex and never trigger this.
    pub fn bearer(token: &str) -> Self {
        let header: MetadataValue<Ascii> = format!("Bearer {token}")
            .parse()
            .expect("bearer token must be a valid HTTP header value");
        Self {
            header: Some(header),
        }
    }
}

impl Interceptor for AuthInterceptor {
    #[allow(clippy::result_large_err)]
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        if let Some(h) = &self.header {
            req.metadata_mut().insert("authorization", h.clone());
        }
        Ok(req)
    }
}

/// Where to reach the daemon. [`Endpoint::Uds`] is local single-user; the
/// [`Endpoint::Tcp`] variant is a remote/shared team daemon.
#[derive(Clone, Debug)]
pub enum Endpoint {
    Uds(PathBuf),
    Tcp {
        /// `host:port` or `https://host:port`.
        addr: String,
        /// CA bundle PEM (from `statefulmemory team init-ca`).
        ca_pem: Vec<u8>,
        /// Bearer token (from `statefulmemory team token-create`).
        token: String,
        /// TLS SNI / cert SAN to validate against (default `localhost`).
        domain: String,
    },
}

impl Endpoint {
    /// `true` for a remote TCP endpoint — callers skip local daemon
    /// auto-spawn / recovery for these (can't respawn a remote process).
    pub fn is_remote(&self) -> bool {
        matches!(self, Endpoint::Tcp { .. })
    }

    /// Resolve from client-side env, falling back to local UDS at
    /// `default_socket`:
    /// - `STATEFULMEMORY_ADDR`      set & non-empty → TCP (requires the rest).
    /// - `STATEFULMEMORY_CA`        path to the CA PEM (required for TCP).
    /// - `STATEFULMEMORY_TOKEN`     bearer token, or `STATEFULMEMORY_TOKEN_FILE`
    ///   a file to read it from (file wins if both set).
    /// - `STATEFULMEMORY_TLS_DOMAIN` cert SAN to validate (default `localhost`).
    pub fn from_env_or_uds(default_socket: PathBuf) -> Result<Self, ClientError> {
        match std::env::var("STATEFULMEMORY_ADDR") {
            Ok(addr) if !addr.trim().is_empty() => {
                let ca_path = std::env::var("STATEFULMEMORY_CA").map_err(|_| {
                    ClientError::Config(
                        "STATEFULMEMORY_ADDR is set but STATEFULMEMORY_CA (CA PEM path) is not".into(),
                    )
                })?;
                let ca_pem = std::fs::read(&ca_path).map_err(|e| {
                    ClientError::Config(format!("read STATEFULMEMORY_CA {ca_path}: {e}"))
                })?;
                let token = resolve_token()?;
                let domain = std::env::var("STATEFULMEMORY_TLS_DOMAIN")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "localhost".to_string());
                Ok(Endpoint::Tcp {
                    addr: addr.trim().to_string(),
                    ca_pem,
                    token,
                    domain,
                })
            }
            _ => Ok(Endpoint::Uds(default_socket)),
        }
    }
}

fn resolve_token() -> Result<String, ClientError> {
    if let Ok(path) = std::env::var("STATEFULMEMORY_TOKEN_FILE") {
        if !path.trim().is_empty() {
            let raw = std::fs::read_to_string(&path).map_err(|e| {
                ClientError::Config(format!("read STATEFULMEMORY_TOKEN_FILE {path}: {e}"))
            })?;
            return Ok(raw.trim().to_string());
        }
    }
    match std::env::var("STATEFULMEMORY_TOKEN") {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        _ => Err(ClientError::Config(
            "STATEFULMEMORY_ADDR is set but no STATEFULMEMORY_TOKEN / STATEFULMEMORY_TOKEN_FILE provided".into(),
        )),
    }
}

/// Dial `endpoint`, returning a uniform [`AuthClient`]. UDS is lazy (first RPC
/// opens the stream); TCP eagerly completes the TLS handshake here so a bad CA
/// surfaces before any RPC.
pub async fn connect(endpoint: &Endpoint) -> Result<AuthClient, ClientError> {
    match endpoint {
        Endpoint::Uds(path) => {
            let ch = channel::connect_uds(path)?;
            Ok(StatefulMemoryClient::with_interceptor(
                ch,
                AuthInterceptor::none(),
            ))
        }
        Endpoint::Tcp {
            addr,
            ca_pem,
            token,
            domain,
        } => {
            let ch = channel::connect_tcp(addr, ca_pem, domain).await?;
            Ok(StatefulMemoryClient::with_interceptor(
                ch,
                AuthInterceptor::bearer(token),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env vars are process-global; serialize the env-mutating tests.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear() {
        for k in [
            "STATEFULMEMORY_ADDR",
            "STATEFULMEMORY_CA",
            "STATEFULMEMORY_TOKEN",
            "STATEFULMEMORY_TOKEN_FILE",
            "STATEFULMEMORY_TLS_DOMAIN",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn auth_interceptor_none_is_noop() {
        let mut i = AuthInterceptor::none();
        let req = i.call(Request::new(())).unwrap();
        assert!(req.metadata().get("authorization").is_none());
    }

    #[test]
    fn auth_interceptor_bearer_injects_header() {
        let mut i = AuthInterceptor::bearer("deadbeef");
        let req = i.call(Request::new(())).unwrap();
        assert_eq!(
            req.metadata().get("authorization").unwrap().to_str().unwrap(),
            "Bearer deadbeef"
        );
    }

    #[test]
    fn env_absent_defaults_to_uds() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let ep = Endpoint::from_env_or_uds(PathBuf::from("/tmp/x.sock")).unwrap();
        assert!(matches!(&ep, Endpoint::Uds(p) if p == &PathBuf::from("/tmp/x.sock")));
        assert!(!ep.is_remote());
    }

    #[test]
    fn addr_without_ca_errors() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        std::env::set_var("STATEFULMEMORY_ADDR", "example.com:9090");
        let err = Endpoint::from_env_or_uds(PathBuf::from("/tmp/x.sock")).unwrap_err();
        assert!(matches!(err, ClientError::Config(_)), "{err:?}");
        clear();
    }

    #[test]
    fn addr_with_ca_and_token_file_builds_tcp() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, b"-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----\n").unwrap();
        let tok = dir.path().join("token");
        std::fs::write(&tok, "  abc123\n").unwrap();
        std::env::set_var("STATEFULMEMORY_ADDR", "example.com:9090");
        std::env::set_var("STATEFULMEMORY_CA", ca.to_str().unwrap());
        std::env::set_var("STATEFULMEMORY_TOKEN_FILE", tok.to_str().unwrap());
        let ep = Endpoint::from_env_or_uds(PathBuf::from("/tmp/x.sock")).unwrap();
        assert!(ep.is_remote());
        match ep {
            Endpoint::Tcp {
                addr, token, domain, ..
            } => {
                assert_eq!(addr, "example.com:9090");
                assert_eq!(token, "abc123", "token trimmed from file");
                assert_eq!(domain, "localhost", "default SAN");
            }
            other => panic!("expected Tcp, got {other:?}"),
        }
        clear();
    }
}
