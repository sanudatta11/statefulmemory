//! Disk-full read-only mode.
//!
//! Spec sections: FR9, SC-15, EC-16.
//!
//! `DiskMonitor` polls free space on the data directory's filesystem every
//! 30 seconds (configurable). When free space drops below 50 MB (configurable)
//! the daemon enters read-only mode: write RPCs return `RESOURCE_EXHAUSTED`,
//! reads continue. Recovery is symmetric — when free space rises back above
//! the threshold, the next poll lifts the flag.
//!
//! Transient `statvfs` errors (EC-16) are logged at WARN level and do *not*
//! flip the mode (we keep the previous state).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct DiskMonitor {
    pub flag: Arc<AtomicBool>,
}

impl DiskMonitor {
    pub fn new() -> Self {
        DiskMonitor {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn read_only(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

impl Default for DiskMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn the polling task. Returns a join handle the caller can abort during
/// graceful shutdown.
pub fn spawn(
    monitor: DiskMonitor,
    data_dir: PathBuf,
    threshold_bytes: u64,
    interval: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick; the first reading happens after `interval`.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match check_free_space(&data_dir) {
                Ok(free) => {
                    let read_only = free < threshold_bytes;
                    let prev = monitor.flag.swap(read_only, Ordering::Relaxed);
                    if prev != read_only {
                        if read_only {
                            warn!(
                                free_bytes = free,
                                threshold_bytes, "entering read-only mode"
                            );
                        } else {
                            debug!(free_bytes = free, "exiting read-only mode");
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "diskmon statvfs failed; mode unchanged");
                }
            }
        }
    })
}

/// Wraps `nix::sys::statvfs` so the rest of the crate doesn't depend on libc.
fn check_free_space(path: &std::path::Path) -> std::io::Result<u64> {
    let stat = nix::sys::statvfs::statvfs(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    // bavail × frsize is bytes available to non-root.
    Ok(stat.blocks_available() as u64 * stat.fragment_size())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn statvfs_works_on_temp_dir() {
        let d = TempDir::new().unwrap();
        let n = check_free_space(d.path()).unwrap();
        assert!(n > 0, "tempdir should have nonzero free space");
    }

    #[test]
    fn flag_starts_false() {
        let m = DiskMonitor::new();
        assert!(!m.read_only());
    }

    #[tokio::test]
    async fn flag_flips_when_threshold_above_capacity() {
        // We can't truly fill a tmpdir to test the threshold, but we can pass
        // a threshold above the available capacity. That should flip the flag
        // on the first poll cycle.
        let d = TempDir::new().unwrap();
        let m = DiskMonitor::new();
        let h = spawn(
            m.clone(),
            d.path().to_path_buf(),
            u64::MAX,
            Duration::from_millis(20),
        );
        // Wait long enough for one tick.
        tokio::time::sleep(Duration::from_millis(60)).await;
        h.abort();
        assert!(m.read_only());
    }
}
