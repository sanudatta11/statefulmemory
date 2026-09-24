//! Mid-session daemon recovery used by [`crate::client::LazyClient`].
//!
//! Re-runs `statefulmemory_client::ensure_running` (probe → clear stale
//! socket → spawn) when a tool call sees a missing socket or `UNAVAILABLE`.

use statefulmemory_client::ensure::{ensure_running, AutoSpawnConfig};

use crate::client::Recovery;
use crate::error::McpError;

/// Spawns / repairs the daemon via the shared ensure path.
pub struct EnsureRecovery {
    cfg: AutoSpawnConfig,
}

impl EnsureRecovery {
    pub fn new(cfg: AutoSpawnConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait::async_trait]
impl Recovery for EnsureRecovery {
    async fn recover(&self) -> Result<(), McpError> {
        ensure_running(&self.cfg)
            .await
            .map(|_| ())
            .map_err(|e| McpError::Recovery {
                message: e.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn recover_maps_spawn_failure_to_daemon_not_running() {
        // Binary that never creates the socket → ensure times out quickly.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("noop.sh");
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = AutoSpawnConfig::defaults(
            dir.path().join("daemon.sock"),
            dir.path().join("daemon.lock"),
            bin,
            dir.path().join("daemon.log"),
        );
        cfg.budget = Duration::from_millis(150);

        let rec = EnsureRecovery::new(cfg);
        let err = rec.recover().await.expect_err("spawn must fail");
        assert!(matches!(err, McpError::Recovery { .. }), "{err:?}");
        assert_eq!(err.key(), "daemon_recovery_failed");
    }

    #[tokio::test]
    async fn recovery_is_object_safe_for_lazy_client() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AutoSpawnConfig::defaults(
            dir.path().join("daemon.sock"),
            dir.path().join("daemon.lock"),
            dir.path().join("missing-bin"),
            dir.path().join("daemon.log"),
        );
        let r: Arc<dyn Recovery> = Arc::new(EnsureRecovery::new(cfg));
        // Existence check only — actual spawn failure covered above.
        let _ = r;
    }
}
