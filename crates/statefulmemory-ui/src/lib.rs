#![forbid(unsafe_code)]

mod backend;
mod http;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use subtle::ConstantTimeEq;
use thiserror::Error;
use tiny_http::Server;

pub use backend::{BackendError, GrpcBackend, UiBackend};

pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_URL_BYTES: usize = 2048;
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const SESSION_TTL_SECS: u64 = 60 * 60;

pub(crate) const SESSION_COOKIE: &str = "statefulmemory_ui_session";

const TOKEN_BYTES: usize = 32;
const WORKERS: usize = 8;

#[derive(Debug, Error)]
pub enum UiError {
    #[error("host must be exactly 127.0.0.1, localhost, or ::1")]
    InvalidHost,
    #[error("could not bind local UI at {address}: {message}")]
    Bind { address: String, message: String },
    #[error("could not create local UI runtime: {message}")]
    Runtime { message: String },
    #[error("could not create local UI credentials: {message}")]
    Credentials { message: String },
    #[error("local UI worker failed: {message}")]
    Worker { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConfig {
    host: String,
    port: u16,
}

impl UiConfig {
    pub fn new(host: impl Into<String>, port: u16) -> Result<Self, UiError> {
        let host = host.into();
        if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
            return Err(UiError::InvalidHost);
        }
        Ok(Self { host, port })
    }

    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.authority())
    }

    pub fn bind_address(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn with_port(&self, port: u16) -> Self {
        Self {
            host: self.host.clone(),
            port,
        }
    }
}

pub struct UiAssets {
    pub html: &'static str,
    pub script: &'static str,
    pub style: &'static str,
}

pub struct RunningUi {
    server: Server,
    state: Arc<UiState>,
    launch_url: String,
}

impl RunningUi {
    pub fn launch_url(&self) -> &str {
        &self.launch_url
    }

    pub fn serve(self) -> Result<(), UiError> {
        let RunningUi {
            server,
            state,
            launch_url: _,
        } = self;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| UiError::Runtime {
                message: error.to_string(),
            })?;
        let handle = runtime.handle().clone();
        let server = Arc::new(server);
        let mut workers = Vec::with_capacity(WORKERS);
        for index in 0..WORKERS {
            let server = Arc::clone(&server);
            let state = Arc::clone(&state);
            let handle = handle.clone();
            let worker = std::thread::Builder::new()
                .name(format!("statefulmemory-ui-{index}"))
                .spawn(move || loop {
                    let mut request = match server.recv() {
                        Ok(request) => request,
                        Err(_) => break,
                    };
                    let response =
                        handle.block_on(http::handle_request(Arc::clone(&state), &mut request));
                    if request.respond(response).is_err() {
                        break;
                    }
                })
                .map_err(|error| UiError::Worker {
                    message: error.to_string(),
                })?;
            workers.push(worker);
        }
        for worker in workers {
            let _ = worker.join();
        }
        Ok(())
    }
}

pub fn bind(
    config: UiConfig,
    project_name: impl Into<String>,
    backend: Arc<dyn UiBackend>,
    html: &'static str,
) -> Result<RunningUi, UiError> {
    bind_with_assets(
        config,
        project_name,
        backend,
        UiAssets {
            html,
            script: "",
            style: "",
        },
    )
}

pub fn bind_with_assets(
    mut config: UiConfig,
    project_name: impl Into<String>,
    backend: Arc<dyn UiBackend>,
    assets: UiAssets,
) -> Result<RunningUi, UiError> {
    let address = config.bind_address();
    let server = Server::http(&address).map_err(|error| UiError::Bind {
        address: address.clone(),
        message: error.to_string(),
    })?;
    if let Some(socket_addr) = server.server_addr().to_ip() {
        if !socket_addr.ip().is_loopback() {
            return Err(UiError::InvalidHost);
        }
        config = config.with_port(socket_addr.port());
    }
    let auth = AuthState::new()?;
    let authority = config.authority();
    let project_name = project_name.into();
    let launch_url = format!(
        "http://{authority}?token={}&project={}",
        auth.launch_token,
        encode_component(&project_name)
    );
    let state = Arc::new(UiState {
        config,
        project_name,
        auth,
        backend,
        html: assets.html,
        script: assets.script,
        style: assets.style,
    });
    Ok(RunningUi {
        server,
        state,
        launch_url,
    })
}

pub(crate) struct UiState {
    pub(crate) config: UiConfig,
    pub(crate) project_name: String,
    pub(crate) auth: AuthState,
    pub(crate) backend: Arc<dyn UiBackend>,
    pub(crate) html: &'static str,
    pub(crate) script: &'static str,
    pub(crate) style: &'static str,
}

pub(crate) struct AuthState {
    pub(crate) launch_token: String,
    session_token: String,
    launch_used: AtomicBool,
    session: RwLock<SessionState>,
}

#[derive(Default)]
struct SessionState {
    active: bool,
    expires_at: Option<Instant>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum AuthFailure {
    InvalidLaunch,
    LaunchUsed,
}

impl AuthState {
    fn new() -> Result<Self, UiError> {
        let launch_token = random_hex(TOKEN_BYTES).map_err(|error| UiError::Credentials {
            message: error.to_string(),
        })?;
        let session_token = random_hex(TOKEN_BYTES).map_err(|error| UiError::Credentials {
            message: error.to_string(),
        })?;
        Ok(Self {
            launch_token,
            session_token,
            launch_used: AtomicBool::new(false),
            session: RwLock::new(SessionState::default()),
        })
    }

    pub(crate) fn redeem_launch(&self, candidate: &str) -> Result<String, AuthFailure> {
        if !tokens_equal(candidate, &self.launch_token) {
            return Err(AuthFailure::InvalidLaunch);
        }
        self.launch_used
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AuthFailure::LaunchUsed)?;
        let mut session = self
            .session
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session.active = true;
        session.expires_at = Some(Instant::now() + Duration::from_secs(SESSION_TTL_SECS));
        Ok(self.session_token.clone())
    }

    pub(crate) fn session_valid(&self, candidate: &str) -> bool {
        if !tokens_equal(candidate, &self.session_token) {
            return false;
        }
        let session = self
            .session
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session.active
            && session
                .expires_at
                .is_some_and(|expires_at| Instant::now() < expires_at)
    }

    pub(crate) fn session_ttl_secs(&self) -> u64 {
        SESSION_TTL_SECS
    }
}

fn tokens_equal(left: &str, right: &str) -> bool {
    bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn encode_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn random_hex(length: usize) -> Result<String, getrandom::Error> {
    let mut bytes = vec![0_u8; length];
    getrandom::getrandom(&mut bytes)?;
    let mut output = String::with_capacity(length * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_loopback_hosts_are_accepted() {
        assert!(UiConfig::new("127.0.0.1", 4687).is_ok());
        assert!(UiConfig::new("localhost", 4687).is_ok());
        assert!(UiConfig::new("::1", 4687).is_ok());
        assert!(UiConfig::new("0.0.0.0", 4687).is_err());
        assert!(UiConfig::new("example.com", 4687).is_err());
    }

    #[test]
    fn ipv6_authority_is_bracketed() {
        let config = UiConfig::new("::1", 4687).unwrap();
        assert_eq!(config.authority(), "[::1]:4687");
        assert_eq!(config.origin(), "http://[::1]:4687");
    }
}
