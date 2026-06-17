// Generated with AI Coding Rules Hub
//! Structured JSON logging via `tracing-subscriber` + `tracing-appender`.
//!
//! Spec sections: FR8, NFR8, SC-23.
//!
//! Log file: `~/.memlayer/daemon.log` (JSON, one record per line).
//! Level:    `MEMLAYER_LOG` env var (default: `debug`).
//!           Examples: `MEMLAYER_LOG=trace`, `MEMLAYER_LOG=info`.
//!
//! Read the log:
//!   memlayer logs --lines 100
//!   tail -f ~/.memlayer/daemon.log | jq .
//!   cat ~/.memlayer/daemon.log | jq 'select(.level=="ERROR")'

use std::path::Path;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;

use memlayer_core::error::{Error, Result};

/// Initialize global tracing subscriber. Returns the `WorkerGuard` so the
/// caller can keep it alive for the lifetime of the daemon (drop = flush).
///
/// Writes structured JSON to `log_path`. Level defaults to `debug` so that
/// the log file contains enough detail to diagnose FK errors, project
/// detection failures, and other operational issues without needing a
/// restart. Override with `MEMLAYER_LOG=info` (or trace/warn/error).
pub fn init(
    log_path: &Path,
    env_filter: &str,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let dir = log_path
        .parent()
        .ok_or_else(|| Error::internal(format!("invalid log path: {}", log_path.display())))?;
    let stem = log_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("daemon.log")
        .to_string();
    std::fs::create_dir_all(dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::NEVER)
        .filename_prefix(stem)
        .build(dir)
        .map_err(|e| Error::internal(format!("init log appender: {e}")))?;
    let (nb, guard) = tracing_appender::non_blocking(appender);

    // Default to "debug" instead of "info" so the log file is useful for
    // diagnosing issues without needing a restart or env-var override.
    // Callers pass the value from Config.log_level (which reads MEMLAYER_LOG).
    let effective = if env_filter.trim().is_empty() { "debug" } else { env_filter };
    let filter = EnvFilter::try_new(effective).unwrap_or_else(|_| EnvFilter::new("debug"));

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_writer(nb)
                .with_target(true)
                .with_ansi(false),
        );
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(guard)
}
