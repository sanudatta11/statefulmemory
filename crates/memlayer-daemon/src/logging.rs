//! Structured JSON logging via `tracing-subscriber` + `tracing-appender`.
//!
//! Spec sections: FR8, NFR8, SC-23.

use std::path::Path;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;

use memlayer_core::error::{Error, Result};

/// Initialize global tracing subscriber. Returns the `WorkerGuard` so the caller
/// can keep it alive for the lifetime of the daemon (drop = flush).
///
/// Implementation notes:
/// - Single rolling file at `log_path`. External rotation (size + count) is
///   handled in `signals::on_sighup` (FR8.4).
/// - JSON formatter; no ANSI escapes.
/// - Filter from env var (`MEMLAYER_LOG`); defaults to `info` if invalid.
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

    let filter = EnvFilter::try_new(env_filter).unwrap_or_else(|_| EnvFilter::new("info"));
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
