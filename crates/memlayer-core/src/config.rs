//! Daemon-wide configuration loaded from env vars and an optional JSON file.
//!
//! Resolution order (PRD §13):
//! 1. Environment variables (highest priority).
//! 2. `~/.memlayer/config.json` (if present).
//! 3. Built-in defaults.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths;

/// All tunables resolved at daemon-start time.
///
/// Field defaults match PRD §13.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub data_dir: PathBuf,
    /// `tracing-subscriber` env-filter syntax. e.g., `info`, `memlayer=debug`.
    pub log_level: String,
    /// Dedupe window for the normalized-hash content match (PRD §5.8).
    /// `0` disables hash-based dedupe entirely.
    pub dedupe_window: Duration,
    /// Listen target. `None` = UDS at `~/.memlayer/daemon.sock`.
    /// `Some("tcp://host:port")` = TCP+TLS+token (FR2.2).
    pub listen: Option<String>,
    /// PEM cert path. Required when `listen` is TCP.
    pub tls_cert_path: Option<PathBuf>,
    /// PEM private-key path. Required when `listen` is TCP.
    pub tls_key_path: Option<PathBuf>,
    /// Maximum content length (chars) for `SaveObservation` (EC-2).
    pub max_content_chars: usize,
    /// Disk-full threshold in bytes (PRD §5.7).
    pub disk_full_threshold: u64,
    /// Disk-monitor poll interval.
    pub disk_poll_interval: Duration,
    /// Per-project connection LRU capacity (PRD §15.2 / §11.2).
    pub project_lru_capacity: usize,
    /// Read-pool max concurrency per project.
    pub read_pool_size: usize,
    /// Write-thread batch ceiling (rows).
    pub write_batch_max: usize,
    /// Write-thread batch ceiling (time).
    pub write_batch_window: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: paths::data_dir(),
            log_level: "info".into(),
            dedupe_window: Duration::from_secs(60 * 60 * 24 * 30), // 30 days
            listen: None,
            tls_cert_path: None,
            tls_key_path: None,
            max_content_chars: 50_000,
            disk_full_threshold: 50 * 1024 * 1024, // 50 MB
            disk_poll_interval: Duration::from_secs(30),
            project_lru_capacity: 256,
            read_pool_size: 4,
            write_batch_max: 32,
            write_batch_window: Duration::from_millis(5),
        }
    }
}

/// Subset of `Config` deserialized from `~/.memlayer/config.json`. All fields optional.
#[derive(Debug, Default, Deserialize)]
struct FileOverrides {
    data_dir: Option<PathBuf>,
    log_level: Option<String>,
    dedupe_window_seconds: Option<u64>,
    listen: Option<String>,
    tls_cert_path: Option<PathBuf>,
    tls_key_path: Option<PathBuf>,
    max_content_chars: Option<usize>,
    disk_full_threshold_bytes: Option<u64>,
    disk_poll_interval_seconds: Option<u64>,
    project_lru_capacity: Option<usize>,
    read_pool_size: Option<usize>,
}

impl Config {
    /// Load config: defaults → file → env, last writer wins.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::default();

        // File-level overrides (optional).
        let file_path = cfg.data_dir.join("config.json");
        if file_path.exists() {
            let bytes = std::fs::read(&file_path)?;
            let f: FileOverrides = serde_json::from_slice(&bytes)?;
            if let Some(v) = f.data_dir {
                cfg.data_dir = v;
            }
            if let Some(v) = f.log_level {
                cfg.log_level = v;
            }
            if let Some(v) = f.dedupe_window_seconds {
                cfg.dedupe_window = Duration::from_secs(v);
            }
            if let Some(v) = f.listen {
                cfg.listen = Some(v);
            }
            if let Some(v) = f.tls_cert_path {
                cfg.tls_cert_path = Some(v);
            }
            if let Some(v) = f.tls_key_path {
                cfg.tls_key_path = Some(v);
            }
            if let Some(v) = f.max_content_chars {
                cfg.max_content_chars = v;
            }
            if let Some(v) = f.disk_full_threshold_bytes {
                cfg.disk_full_threshold = v;
            }
            if let Some(v) = f.disk_poll_interval_seconds {
                cfg.disk_poll_interval = Duration::from_secs(v);
            }
            if let Some(v) = f.project_lru_capacity {
                cfg.project_lru_capacity = v;
            }
            if let Some(v) = f.read_pool_size {
                cfg.read_pool_size = v;
            }
        }

        // Environment overrides (PRD §13.2).
        if let Ok(v) = std::env::var("MEMLAYER_DATA_DIR") {
            cfg.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("MEMLAYER_LOG") {
            cfg.log_level = v;
        }
        if let Ok(v) = std::env::var("MEMLAYER_DEDUPE_WINDOW") {
            let secs: u64 = v
                .parse()
                .map_err(|e| Error::invalid(format!("MEMLAYER_DEDUPE_WINDOW: {e}")))?;
            cfg.dedupe_window = Duration::from_secs(secs);
        }
        if let Ok(v) = std::env::var("MEMLAYER_LISTEN") {
            cfg.listen = Some(v);
        }
        if let Ok(v) = std::env::var("MEMLAYER_TLS_CERT") {
            cfg.tls_cert_path = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("MEMLAYER_TLS_KEY") {
            cfg.tls_key_path = Some(PathBuf::from(v));
        }

        cfg.validate()?;
        Ok(cfg)
    }

    /// Returns `true` if the daemon should bind TCP rather than UDS.
    pub fn is_tcp_mode(&self) -> bool {
        self.listen.as_deref().is_some_and(|s| s.starts_with("tcp://"))
    }

    fn validate(&self) -> Result<()> {
        if self.is_tcp_mode() {
            if self.tls_cert_path.is_none() || self.tls_key_path.is_none() {
                return Err(Error::FailedPrecondition(
                    "TCP mode requires both MEMLAYER_TLS_CERT and MEMLAYER_TLS_KEY".into(),
                ));
            }
        }
        if self.project_lru_capacity == 0 {
            return Err(Error::invalid("project_lru_capacity must be > 0"));
        }
        if self.read_pool_size == 0 {
            return Err(Error::invalid("read_pool_size must be > 0"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.max_content_chars, 50_000);
        assert_eq!(c.project_lru_capacity, 256);
        assert!(!c.is_tcp_mode());
    }

    #[test]
    fn tcp_mode_requires_tls() {
        let mut c = Config::default();
        c.listen = Some("tcp://0.0.0.0:9090".into());
        assert!(c.validate().is_err());
        c.tls_cert_path = Some(PathBuf::from("/x"));
        c.tls_key_path = Some(PathBuf::from("/y"));
        assert!(c.validate().is_ok());
    }
}
