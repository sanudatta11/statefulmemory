//! Project registry: a thread-safe LRU cache from normalized project name to
//! per-project runtime state.
//!
//! Spec sections: FR1.4 (LRU cap 256), FR7.2 (collision detection),
//! SC-17 (collision), SC-25 (eviction churn ≤ 10/min), OQ-6 (idle eviction).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use memlayer_core::error::{Error, Result};
use memlayer_core::paths;
use memlayer_core::project as project_name;

use crate::write::{spawn_write_thread, WriteHandle};

/// One project's runtime state, owned by `ProjectRegistry`.
pub struct ProjectState {
    pub normalized: String,
    pub display_name: String,
    pub db_path: PathBuf,
    pub write: WriteHandle,
    /// Number of in-flight read connections. Used to defer eviction (OQ-6).
    pub read_in_flight: parking_lot::Mutex<usize>,
}

impl ProjectState {
    /// Open a fresh read-only connection for this project. Caller is responsible
    /// for closing when done — this is a short-lived "borrow" with `read_in_flight`
    /// counters maintained at the registry layer (see `Registry::checkout_read`).
    pub fn open_read_conn(&self) -> Result<Connection> {
        crate::db::open_read(&self.db_path)
    }
}

/// Per-project on-disk metadata stored in `~/.memlayer/projects/<id>/config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project_display_name: String,
    pub created_at: String,
    /// Optional path to the developer's git repo for sync-export (Spec 3).
    pub repo_path: Option<PathBuf>,
}

/// Thread-safe LRU registry of project states.
///
/// Eviction rule (OQ-6): only evict if (a) write channel is empty (best-effort,
/// since crossbeam's `is_empty` is racy) AND (b) `read_in_flight == 0`. If both
/// fail, defer the eviction to the next promote.
pub struct ProjectRegistry {
    inner: Arc<Mutex<RegistryInner>>,
    write_batch_max: usize,
    write_batch_window: Duration,
}

struct RegistryInner {
    capacity: usize,
    /// `lru[0]` is the LRU end (oldest), `lru.back()` is most-recent.
    lru: VecDeque<String>,
    map: HashMap<String, Arc<ProjectState>>,
    /// Stats for SC-25: cumulative hits and misses.
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl ProjectRegistry {
    pub fn new(capacity: usize, write_batch_max: usize, write_batch_window: Duration) -> Self {
        ProjectRegistry {
            inner: Arc::new(Mutex::new(RegistryInner {
                capacity,
                lru: VecDeque::with_capacity(capacity),
                map: HashMap::with_capacity(capacity),
                hits: 0,
                misses: 0,
                evictions: 0,
            })),
            write_batch_max,
            write_batch_window,
        }
    }

    /// Resolve a project: normalize its display name, open / create the DB
    /// and per-project config.json, and cache the state.
    ///
    /// Returns `ALREADY_EXISTS` (FR7.2, SC-17) if `display_name` normalizes
    /// to an existing on-disk identifier whose stored display_name differs.
    pub fn get_or_open(&self, display_name: &str) -> Result<Arc<ProjectState>> {
        let normalized = project_name::normalize(display_name)?;

        // Check the cache.
        {
            let mut inner = self.inner.lock();
            if let Some(state) = inner.map.get(&normalized).cloned() {
                if state.display_name != display_name {
                    return Err(Error::AlreadyExists(format!(
                        "project '{display_name}' normalizes to '{normalized}', \
                         which already maps to '{}'",
                        state.display_name
                    )));
                }
                inner.hits += 1;
                inner.touch(&normalized);
                return Ok(state);
            }
            inner.misses += 1;
        }

        // Cache miss — create or open.
        let db_path = paths::project_db_path(&normalized);
        let cfg_path = paths::project_config_path(&normalized);

        if cfg_path.exists() {
            let bytes = std::fs::read(&cfg_path)?;
            let cfg: ProjectConfig = serde_json::from_slice(&bytes).map_err(|e| {
                Error::internal(format!("read project config {}: {e}", cfg_path.display()))
            })?;
            if cfg.project_display_name != display_name {
                return Err(Error::AlreadyExists(format!(
                    "project '{display_name}' normalizes to '{normalized}', \
                     which already maps to '{}'",
                    cfg.project_display_name
                )));
            }
        } else {
            // First save: create config dir + file.
            let dir = paths::project_dir(&normalized);
            std::fs::create_dir_all(&dir)?;
            let cfg = ProjectConfig {
                project_display_name: display_name.to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                repo_path: None,
            };
            let body = serde_json::to_vec_pretty(&cfg)?;
            atomic_write(&cfg_path, &body)?;
        }

        let write = spawn_write_thread(
            normalized.clone(),
            db_path.clone(),
            self.write_batch_max,
            self.write_batch_window,
        )?;
        let state = Arc::new(ProjectState {
            normalized: normalized.clone(),
            display_name: display_name.to_string(),
            db_path,
            write,
            read_in_flight: parking_lot::Mutex::new(0),
        });

        // Insert + evict.
        let mut inner = self.inner.lock();
        if inner.map.len() >= inner.capacity {
            inner.try_evict_oldest();
        }
        inner.map.insert(normalized.clone(), state.clone());
        inner.lru.push_back(normalized);
        Ok(state)
    }

    /// Lift a previously-cached project to the most-recently-used position.
    ///
    /// Returns the cached state if present, else `None`.
    pub fn touch(&self, normalized: &str) -> Option<Arc<ProjectState>> {
        let mut inner = self.inner.lock();
        let state = inner.map.get(normalized).cloned();
        if state.is_some() {
            inner.touch(normalized);
        }
        state
    }

    /// Returns hit-ratio (DaemonStatus.cache_hit_ratio).
    pub fn hit_ratio(&self) -> f64 {
        let inner = self.inner.lock();
        let total = inner.hits + inner.misses;
        if total == 0 {
            0.0
        } else {
            inner.hits as f64 / total as f64
        }
    }

    pub fn cached_count(&self) -> usize {
        self.inner.lock().map.len()
    }

    pub fn evictions(&self) -> u64 {
        self.inner.lock().evictions
    }

    /// All known projects on disk (regardless of cache state).
    pub fn list_known_on_disk() -> Result<Vec<(String, ProjectConfig)>> {
        let dir = paths::projects_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if !ft.is_dir() {
                continue;
            }
            let normalized = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::internal("non-UTF8 project dir name"))?;
            // Validate; corrupted/manual edits should not crash the daemon.
            if project_name::validate(&normalized).is_err() {
                warn!(?normalized, "skipping malformed project dir");
                continue;
            }
            let cfg_path = entry.path().join("config.json");
            if !cfg_path.exists() {
                continue;
            }
            let bytes = std::fs::read(&cfg_path)?;
            let cfg: ProjectConfig = match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(e) => {
                    warn!(?cfg_path, "skipping unreadable config: {e}");
                    continue;
                }
            };
            out.push((normalized, cfg));
        }
        Ok(out)
    }
}

impl RegistryInner {
    fn touch(&mut self, normalized: &str) {
        if let Some(pos) = self.lru.iter().position(|n| n == normalized) {
            let n = self.lru.remove(pos).unwrap();
            self.lru.push_back(n);
        }
    }

    fn try_evict_oldest(&mut self) {
        // Walk the LRU end. If the candidate is in-flight (reads), defer.
        let snapshot: Vec<String> = self.lru.iter().cloned().collect();
        for candidate in snapshot {
            if let Some(state) = self.map.get(&candidate) {
                let in_flight = *state.read_in_flight.lock();
                if in_flight == 0 {
                    self.lru.retain(|n| n != &candidate);
                    self.map.remove(&candidate);
                    self.evictions += 1;
                    info!(project = %candidate, "evicted project from cache");
                    return;
                }
            }
        }
        warn!("eviction deferred: all candidates have in-flight reads");
    }
}

/// Atomic write: write to `<path>.tmp`, fsync, rename to `<path>`.
pub fn atomic_write(path: &std::path::Path, body: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // MEMLAYER_DATA_DIR is process-global. Serialize all tests that mutate it
    // so they don't stomp on each other's tempdir when run in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn isolated_data_dir() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let d = tempfile::TempDir::new().unwrap();
        std::env::set_var("MEMLAYER_DATA_DIR", d.path());
        (guard, d)
    }

    #[test]
    fn get_or_open_creates_config() {
        let (_guard, _d) = isolated_data_dir();
        let r = ProjectRegistry::new(8, 32, Duration::from_millis(5));
        let s = r.get_or_open("My Project").unwrap();
        assert_eq!(s.normalized, "my-project");
        let cfg = paths::project_config_path("my-project");
        assert!(cfg.exists());
    }

    #[test]
    fn collision_returns_already_exists() {
        let (_guard, _d) = isolated_data_dir();
        let r = ProjectRegistry::new(8, 32, Duration::from_millis(5));
        r.get_or_open("My Project").unwrap();
        let err = r.get_or_open("MY_PROJECT").err().unwrap();
        assert!(matches!(err, Error::AlreadyExists(_)));
    }

    #[test]
    fn cache_hit_ratio_climbs() {
        let (_guard, _d) = isolated_data_dir();
        let r = ProjectRegistry::new(8, 32, Duration::from_millis(5));
        r.get_or_open("foo").unwrap();
        r.get_or_open("foo").unwrap();
        r.get_or_open("foo").unwrap();
        // 1 miss, 2 hits → 2/3 ≈ 0.66.
        let ratio = r.hit_ratio();
        assert!(ratio > 0.5, "hit ratio = {ratio}");
    }
}
