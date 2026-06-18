//! Tonic server boot: UDS or TCP+TLS.
//!
//! Spec sections: FR1, FR2, FR8 (logging interplay), FR10 (signals interplay),
//! SC-1, SC-3, SC-16.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tracing::info;

use memlayer_core::config::Config;
use memlayer_core::error::{Error, Result};
use memlayer_core::paths;
use memlayer_proto::memlayer_server::MemlayerServer;
use memlayer_storage::diskmon::{self, DiskMonitor};
use memlayer_storage::{pragmas, ProjectRegistry};

use crate::auth;
use crate::lifecycle::{self, LifecycleGuard};
use crate::logging;
use crate::service::{DaemonState, MemlayerService};
use crate::signals;
use crate::tls;
use crate::tokens::TokenStore;

/// Run the daemon to completion: bind, serve, drain on shutdown.
pub async fn run(cfg: Config) -> Result<()> {
    paths::ensure_dirs(&cfg.data_dir)?;
    let _log_guard = logging::init(&paths::log_path(), &cfg.log_level)?;
    info!(version = env!("CARGO_PKG_VERSION"), data_dir = %cfg.data_dir.display(), "daemon starting");

    // Lock + pidfile.
    let lock_file =
        lifecycle::acquire_lock_and_pid(&paths::lock_path(), &paths::pid_path())?;
    lifecycle::raise_fd_limit();

    // Project registry + disk monitor.
    let registry = Arc::new(ProjectRegistry::new(
        cfg.project_lru_capacity,
        cfg.write_batch_max,
        cfg.write_batch_window,
    ));
    let dm = DiskMonitor::new();
    diskmon::spawn(dm.clone(), cfg.data_dir.clone(), cfg.disk_full_threshold, cfg.disk_poll_interval);

    // Embed-model compatibility guard (SC-11). Iterate over project DBs known
    // on disk and refuse to start if any of them stored embeddings under a
    // different model than the one the daemon will use. Pre-existing projects
    // with no embeddings yet pass.
    check_embed_model_on_disk(&registry)?;

    // Token store. Opened in both UDS and TCP modes: UDS mode lets the
    // local admin pre-provision tokens that will later authenticate TCP
    // clients (the typical bootstrap flow). The TCP auth interceptor is
    // wired below only when binding TCP, so UDS callers reach the
    // admin-only RPCs (CreateToken/ListTokens/RevokeToken) without an
    // AuthCtx — admin_guard::require_admin pass-through covers that case
    // (filesystem permissions are the UDS trust boundary).
    let token_store = Some(Arc::new(TokenStore::open(paths::tokens_db_path())?));

    // Cross-project global mirror DB. We only fail-soft: if the global DB
    // cannot be opened (disk full, permissions, schema corruption), the
    // daemon keeps running with `global_db: None` so per-project saves
    // continue to work. `--all-projects` search will return empty in that
    // case rather than blocking the daemon.
    let global_db = match memlayer_storage::GlobalDb::open(&cfg.data_dir) {
        Ok(db) => Some(Arc::new(Mutex::new(db))),
        Err(e) => {
            tracing::warn!(error = %e, "global mirror DB unavailable — --all-projects search disabled");
            None
        }
    };

    // Daemon shared state.
    let in_flight = Arc::new(AtomicU64::new(0));
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let state = Arc::new(DaemonState {
        registry: registry.clone(),
        disk_monitor: dm.clone(),
        token_store: token_store.clone(),
        in_flight: in_flight.clone(),
        started_at: chrono::Utc::now(),
        shutdown_tx: shutdown_tx.clone(),
        max_content_chars: cfg.max_content_chars,
        dedupe_window: cfg.dedupe_window,
        export_mutexes: Arc::new(Mutex::new(HashMap::new())),
        last_sync_errors: Arc::new(Mutex::new(HashMap::new())),
        last_export_at: Arc::new(Mutex::new(HashMap::new())),
        global_db,
    });
    let svc = MemlayerService::new(state.clone());

    // Signals.
    let mut shutdown_rx = signals::install(registry.clone(), dm.clone(), in_flight.clone(), shutdown_tx);

    // Bind + serve.
    let serve_result = if cfg.is_tcp_mode() {
        serve_tcp(cfg.clone(), svc, token_store, &mut shutdown_rx).await
    } else {
        serve_uds(svc, &mut shutdown_rx).await
    };

    // Cleanup.
    let _guard = LifecycleGuard {
        lock_file,
        pid_path: paths::pid_path(),
        socket_path: if cfg.is_tcp_mode() { None } else { Some(paths::socket_path()) },
    };

    serve_result
}

/// Walk every project DB on disk and refuse to start if any of them stored
/// embeddings under a model id different from the one this daemon binary will
/// produce (SC-11). Empty / fresh DBs pass; the check only fires once an
/// observation has actually been embedded under some other model.
fn check_embed_model_on_disk(_registry: &ProjectRegistry) -> Result<()> {
    use memlayer_storage::registry::ProjectRegistry as _Reg;
    // Skip projects that have a config.json but no DB file yet — nothing
    // could have been embedded there.
    let projects = match _Reg::list_known_on_disk() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "could not enumerate known projects for embed-model check");
            return Ok(());
        }
    };

    // Auto-extension must be live before opening — open_read does this too,
    // but doing it once here keeps the order explicit.
    pragmas::ensure_sqlite_vec_extension();

    for (name, _cfg) in projects {
        let db_path = paths::project_db_path(&name);
        if !db_path.exists() {
            continue;
        }
        // Use a raw read connection: open_read in storage runs pragmas. The
        // model check itself tolerates pre-V4 databases (table missing → Ok).
        let conn = match memlayer_storage::db::open_read(&db_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(project = %name, error = %e, "skipping embed-model check for project (open failed)");
                continue;
            }
        };
        pragmas::check_embed_model_compat(&conn, pragmas::STORED_EMBED_MODEL)
            .map_err(|e| {
                Error::FailedPrecondition(format!(
                    "project \"{name}\": {e}"
                ))
            })?;
    }
    Ok(())
}

async fn serve_uds(
    svc: MemlayerService,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let sock = paths::socket_path();
    lifecycle::unlink_stale_socket(&sock)?;
    let listener = UnixListener::bind(&sock).map_err(|e| {
        Error::internal(format!("bind UDS {}: {e}", sock.display()))
    })?;
    lifecycle::chmod_socket_0600(&sock)?;
    info!(path=%sock.display(), "bound UDS");

    let stream = UnixListenerStream::new(listener);

    let mut rx = shutdown_rx.clone();
    Server::builder()
        .http2_keepalive_interval(Some(Duration::from_secs(30)))
        .http2_keepalive_timeout(Some(Duration::from_secs(20)))
        .concurrency_limit_per_connection(64)
        .max_concurrent_streams(Some(256))
        .add_service(MemlayerServer::new(svc))
        .serve_with_incoming_shutdown(stream, async move {
            let _ = rx.changed().await;
            info!("UDS server: drain triggered");
            // 5s drain window — tonic stops accepting and waits for in-flight.
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await
        .map_err(|e| Error::internal(format!("UDS serve: {e}")))?;
    Ok(())
}

async fn serve_tcp(
    cfg: Config,
    svc: MemlayerService,
    token_store: Option<Arc<TokenStore>>,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let listen = cfg
        .listen
        .as_ref()
        .ok_or_else(|| Error::FailedPrecondition("MEMLAYER_LISTEN missing".into()))?;
    let addr_str = listen.strip_prefix("tcp://").unwrap_or(listen);
    let addr: std::net::SocketAddr = addr_str
        .parse()
        .map_err(|e| Error::invalid(format!("MEMLAYER_LISTEN parse: {e}")))?;

    let cert = cfg.tls_cert_path.as_ref().expect("validated in config");
    let key = cfg.tls_key_path.as_ref().expect("validated in config");
    let tls_cfg = tls::server_tls_config(cert, key)?;

    let store = token_store.expect("TCP mode opens token_store above");
    let interceptor = auth::make_interceptor(store);

    let mut rx = shutdown_rx.clone();
    Server::builder()
        .tls_config(tls_cfg)
        .map_err(|e| Error::FailedPrecondition(format!("TLS config: {e}")))?
        .http2_keepalive_interval(Some(Duration::from_secs(30)))
        .http2_keepalive_timeout(Some(Duration::from_secs(20)))
        .concurrency_limit_per_connection(64)
        .max_concurrent_streams(Some(256))
        .tcp_nodelay(true)
        .accept_http1(false)
        .add_service(MemlayerServer::with_interceptor(svc, interceptor))
        .serve_with_shutdown(addr, async move {
            let _ = rx.changed().await;
            info!("TCP server: drain triggered");
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await
        .map_err(|e| Error::internal(format!("TCP serve: {e}")))?;
    Ok(())
}
