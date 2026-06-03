//! POSIX signal handling.
//!
//! Spec sections: FR10, SC-24.
//!
//! - SIGTERM / SIGINT → graceful shutdown (5s drain).
//! - SIGHUP → reopen log file.
//! - SIGUSR1 → write `~/.memlayer/diagnostics-<ts>.json`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;
use tracing::{info, warn};

use memlayer_core::paths;
use memlayer_storage::{diskmon::DiskMonitor, ProjectRegistry};

/// Spawn signal-handling tasks. Returns the watch receiver that the server
/// loop should use to detect shutdown.
pub fn install(
    registry: Arc<ProjectRegistry>,
    diskmon: DiskMonitor,
    in_flight: Arc<AtomicU64>,
    shutdown_tx: watch::Sender<bool>,
) -> watch::Receiver<bool> {
    let rx = shutdown_tx.subscribe();
    spawn_term_handler(shutdown_tx.clone());
    spawn_int_handler(shutdown_tx);
    spawn_hup_handler();
    spawn_usr1_handler(registry, diskmon, in_flight);
    rx
}

fn spawn_term_handler(tx: watch::Sender<bool>) {
    tokio::spawn(async move {
        let mut sig = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error=%e, "could not install SIGTERM handler");
                return;
            }
        };
        if sig.recv().await.is_some() {
            info!("SIGTERM received; initiating graceful shutdown");
            let _ = tx.send(true);
        }
    });
}

fn spawn_int_handler(tx: watch::Sender<bool>) {
    tokio::spawn(async move {
        let mut sig = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error=%e, "could not install SIGINT handler");
                return;
            }
        };
        if sig.recv().await.is_some() {
            info!("SIGINT received; initiating graceful shutdown");
            let _ = tx.send(true);
        }
    });
}

fn spawn_hup_handler() {
    tokio::spawn(async move {
        let mut sig = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error=%e, "could not install SIGHUP handler");
                return;
            }
        };
        loop {
            if sig.recv().await.is_none() {
                return;
            }
            info!("SIGHUP received; rotating log");
            // External rotation: rename current log to .1, .2, … and let the
            // file-appender open a fresh file on next write. tracing-appender
            // doesn't expose a reopen API, so we move the existing file out
            // of the way and let the appender's lazy creation handle the new
            // file. This is best-effort; production deploys can rely on
            // logrotate.
            let log = paths::log_path();
            if log.exists() {
                let backup = log.with_extension("log.1");
                let _ = std::fs::rename(&log, &backup);
            }
        }
    });
}

fn spawn_usr1_handler(
    registry: Arc<ProjectRegistry>,
    diskmon: DiskMonitor,
    in_flight: Arc<AtomicU64>,
) {
    tokio::spawn(async move {
        let mut sig = match signal(SignalKind::user_defined1()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error=%e, "could not install SIGUSR1 handler");
                return;
            }
        };
        loop {
            if sig.recv().await.is_none() {
                return;
            }
            let now = chrono::Utc::now();
            let path: PathBuf = paths::diagnostics_path(&now);
            let body = serde_json::json!({
                "ts": now.to_rfc3339(),
                "cached_projects": registry.cached_count(),
                "cache_hit_ratio": registry.hit_ratio(),
                "evictions": registry.evictions(),
                "read_only_mode": diskmon.read_only(),
                "in_flight_rpcs": in_flight.load(Ordering::Relaxed),
            });
            if let Err(e) = std::fs::write(&path, body.to_string()) {
                warn!(error=%e, "could not write diagnostics dump");
            } else {
                info!(path=%path.display(), "wrote diagnostics dump");
            }
        }
    });
}
