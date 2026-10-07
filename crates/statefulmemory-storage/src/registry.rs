// Generated with AI Coding Rules Hub
//! Project registry: a thread-safe LRU cache from normalized project name to
//! per-project runtime state.
//!
//! Spec sections: FR1.4 (LRU cap 256), FR7.2 (collision detection),
//! SC-17 (collision), SC-25 (eviction churn ≤ 10/min), OQ-6 (idle eviction).

use std::collections::{HashMap, VecDeque};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use statefulmemory_core::error::{Error, Result};
use statefulmemory_core::paths;
use statefulmemory_core::project as project_name;

use crate::write::{spawn_write_thread, WriteHandle};

/// Default number of warm read connections retained per project when the
/// registry is built without [`ProjectRegistry::with_read_pool_size`]. Matches
/// the daemon `Config::read_pool_size` default so tests/eval and the daemon
/// behave the same.
pub const DEFAULT_READ_POOL_SIZE: usize = 4;

/// One project's runtime state, owned by `ProjectRegistry`.
pub struct ProjectState {
    pub normalized: String,
    pub display_name: String,
    pub db_path: PathBuf,
    pub write: WriteHandle,
    /// Pool of warm read-only connections (bounded). Checked out per read RPC
    /// and returned on guard drop, so pragmas + SQL compilation amortize across
    /// requests instead of being paid on every `open_read`.
    read_pool: Arc<ReadPool>,
}

impl ProjectState {
    /// Build a project state with a bounded warm read-connection pool.
    pub fn new(
        normalized: String,
        display_name: String,
        db_path: PathBuf,
        write: WriteHandle,
        read_pool_size: usize,
    ) -> Self {
        let read_pool = ReadPool::new(db_path.clone(), read_pool_size.max(1));
        ProjectState {
            normalized,
            display_name,
            db_path,
            write,
            read_pool,
        }
    }

    /// Open a fresh (unpooled) read-only connection for this project. Used by
    /// callers that need to own the `Connection` (eval harness, background
    /// workers). Hot read RPC paths should prefer [`Self::checkout_read`].
    pub fn open_read_conn(&self) -> Result<Connection> {
        crate::db::open_read(&self.db_path)
    }

    /// Borrow a warm read connection from the pool. The returned guard derefs
    /// to `&Connection` and returns the connection to the pool when dropped.
    /// While a guard is live, `read_in_flight` is non-zero so eviction defers
    /// (OQ-6).
    pub fn checkout_read(&self) -> Result<PooledReadConn> {
        self.read_pool.checkout()
    }

    /// Number of read connections currently checked out of the pool. Used by
    /// the registry to defer eviction while reads are in flight.
    pub fn read_in_flight(&self) -> usize {
        self.read_pool.in_flight()
    }
}

/// Bounded pool of warm read-only connections for one project DB.
///
/// `checkout` reuses an idle connection or opens a fresh one; the guard returns
/// it to `idle` on drop (dropping it if `idle` is already at `max`, so the pool
/// never retains more than `max` connections but also never blocks a read).
struct ReadPool {
    db_path: PathBuf,
    idle: Mutex<Vec<Connection>>,
    max: usize,
    in_flight: AtomicUsize,
}

impl ReadPool {
    fn new(db_path: PathBuf, max: usize) -> Arc<Self> {
        Arc::new(ReadPool {
            db_path,
            idle: Mutex::new(Vec::with_capacity(max)),
            max,
            in_flight: AtomicUsize::new(0),
        })
    }

    fn checkout(self: &Arc<Self>) -> Result<PooledReadConn> {
        let reused = self.idle.lock().pop();
        let conn = match reused {
            Some(c) => c,
            None => crate::db::open_read(&self.db_path)?,
        };
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        Ok(PooledReadConn {
            pool: Arc::clone(self),
            conn: Some(conn),
        })
    }

    fn checkin(&self, conn: Connection) {
        {
            let mut idle = self.idle.lock();
            if idle.len() < self.max {
                idle.push(conn);
            }
            // else: pool is full — drop the overflow connection.
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }
}

/// RAII guard for a pooled read connection. Derefs to the underlying
/// `rusqlite::Connection` (so existing `&conn` call sites coerce unchanged) and
/// returns the connection to its pool on drop.
pub struct PooledReadConn {
    pool: Arc<ReadPool>,
    conn: Option<Connection>,
}

impl Deref for PooledReadConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("pooled connection present until drop")
    }
}

impl DerefMut for PooledReadConn {
    fn deref_mut(&mut self) -> &mut Connection {
        self.conn
            .as_mut()
            .expect("pooled connection present until drop")
    }
}

impl Drop for PooledReadConn {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.checkin(conn);
        }
    }
}

/// Per-project on-disk metadata stored in `~/.statefulmemory/projects/<id>/config.json`.
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
    /// Warm read connections retained per project (see [`ReadPool`]).
    read_pool_size: usize,
    /// Optional LLM-based conflict classifier injected at daemon startup.
    /// Passed into each project's write thread when the thread is spawned.
    conflict_classifier: Option<std::sync::Arc<dyn crate::conflict_judge::ConflictClassifier>>,
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
            read_pool_size: DEFAULT_READ_POOL_SIZE,
            conflict_classifier: None,
        }
    }

    /// Set the per-project warm read-connection pool size. Must be called
    /// before any `get_or_open` for projects opened afterward to take effect.
    pub fn with_read_pool_size(mut self, size: usize) -> Self {
        self.read_pool_size = size.max(1);
        self
    }

    /// Attach a conflict classifier to all new project write threads.
    /// Must be called before any `get_or_open` call for the classifier to take effect.
    pub fn with_conflict_classifier(
        mut self,
        classifier: std::sync::Arc<dyn crate::conflict_judge::ConflictClassifier>,
    ) -> Self {
        self.conflict_classifier = Some(classifier);
        self
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
            self.conflict_classifier.clone(),
        )?;
        let state = Arc::new(ProjectState::new(
            normalized.clone(),
            display_name.to_string(),
            db_path,
            write,
            self.read_pool_size,
        ));

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

    /// Persist `repo_path` for the given project's `config.json`.
    ///
    /// Spec 3 §11 / OQ-3: when `SyncExport` is invoked with a `repo_path`
    /// (CLI `--repo-path` or auto-detected from cwd), the daemon stores it
    /// in the per-project config so subsequent calls don't need to re-pass
    /// it. Atomic-write via [`atomic_write`].
    ///
    /// `display_name` is required because reading the config requires the
    /// normalized id, which is derived from the display name. Pass the same
    /// `display_name` that the caller used with [`Self::get_or_open`].
    pub fn set_repo_path(&self, display_name: &str, repo_path: &Path) -> Result<()> {
        let normalized = project_name::normalize(display_name)?;
        let cfg_path = paths::project_config_path(&normalized);
        if !cfg_path.exists() {
            return Err(Error::not_found(format!(
                "project config missing for '{display_name}' at {}",
                cfg_path.display()
            )));
        }
        let bytes = std::fs::read(&cfg_path)?;
        let mut cfg: ProjectConfig = serde_json::from_slice(&bytes).map_err(|e| {
            Error::internal(format!("read project config {}: {e}", cfg_path.display()))
        })?;
        cfg.repo_path = Some(repo_path.to_path_buf());
        let body = serde_json::to_vec_pretty(&cfg)?;
        atomic_write(&cfg_path, &body)?;
        info!(project = %normalized, repo = %repo_path.display(), "set repo_path");
        Ok(())
    }

    /// Read the stored `repo_path` for a project, if any.
    ///
    /// Returns `Ok(None)` when:
    /// - the project has no `config.json` on disk, or
    /// - the config exists but has `repo_path: null`.
    ///
    /// Returns an error only on I/O or parse failures.
    pub fn get_repo_path(&self, display_name: &str) -> Result<Option<PathBuf>> {
        let normalized = project_name::normalize(display_name)?;
        let cfg_path = paths::project_config_path(&normalized);
        if !cfg_path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&cfg_path)?;
        let cfg: ProjectConfig = serde_json::from_slice(&bytes).map_err(|e| {
            Error::internal(format!("read project config {}: {e}", cfg_path.display()))
        })?;
        Ok(cfg.repo_path)
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
                let in_flight = state.read_in_flight();
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

    // STATEFULMEMORY_DATA_DIR is process-global. Serialize all tests that mutate it
    // so they don't stomp on each other's tempdir when run in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn isolated_data_dir() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let d = tempfile::TempDir::new().unwrap();
        std::env::set_var("STATEFULMEMORY_DATA_DIR", d.path());
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

    #[test]
    fn set_and_get_repo_path_round_trip() {
        let (_guard, _d) = isolated_data_dir();
        let r = ProjectRegistry::new(8, 32, Duration::from_millis(5));
        r.get_or_open("My Project").unwrap();

        // Initially unset.
        assert!(r.get_repo_path("My Project").unwrap().is_none());

        // Set + read back.
        let repo = std::path::PathBuf::from("/tmp/some/repo");
        r.set_repo_path("My Project", &repo).unwrap();
        let got = r.get_repo_path("My Project").unwrap();
        assert_eq!(got, Some(repo));

        // Unknown project (no config) returns Ok(None).
        let unknown = r.get_repo_path("does-not-exist").unwrap();
        assert!(unknown.is_none());
    }

    #[test]
    fn checkout_read_returns_usable_connection_and_tracks_in_flight() {
        let (_guard, _d) = isolated_data_dir();
        let r = ProjectRegistry::new(8, 32, Duration::from_millis(5)).with_read_pool_size(2);
        let s = r.get_or_open("pool-proj").unwrap();

        let conn = s.checkout_read().unwrap();
        // A trivial query proves the connection is live and pragmas applied
        // (open_read runs the full pragma set).
        let n: i64 = conn.query_row("SELECT 1", [], |row| row.get(0)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(s.read_in_flight(), 1, "one checkout in flight");

        drop(conn);
        assert_eq!(s.read_in_flight(), 0, "returned to pool on drop");
        assert_eq!(s.read_pool.idle.lock().len(), 1, "connection retained as idle");
    }

    #[test]
    fn checkout_read_reuses_idle_connection() {
        let (_guard, _d) = isolated_data_dir();
        let r = ProjectRegistry::new(8, 32, Duration::from_millis(5)).with_read_pool_size(2);
        let s = r.get_or_open("reuse-proj").unwrap();

        // First checkout opens fresh; dropping returns it to idle.
        drop(s.checkout_read().unwrap());
        assert_eq!(s.read_pool.idle.lock().len(), 1);

        // Second checkout drains the idle pool (reuse) rather than opening anew.
        let c = s.checkout_read().unwrap();
        assert_eq!(s.read_pool.idle.lock().len(), 0, "idle conn was reused");
        let n: i64 = c.query_row("SELECT 1", [], |row| row.get(0)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(s.read_in_flight(), 1);
    }

    #[test]
    fn pool_is_bounded_but_never_blocks() {
        let (_guard, _d) = isolated_data_dir();
        let r = ProjectRegistry::new(8, 32, Duration::from_millis(5)).with_read_pool_size(1);
        let s = r.get_or_open("bound-proj").unwrap();

        // Two simultaneous checkouts exceed max=1 — the pool opens an overflow
        // connection rather than blocking the second reader.
        let c1 = s.checkout_read().unwrap();
        let c2 = s.checkout_read().unwrap();
        assert_eq!(s.read_in_flight(), 2, "both live, neither blocked");

        drop(c1); // idle 0 -> 1 (retained)
        drop(c2); // idle already at max=1 -> overflow conn dropped
        assert_eq!(s.read_in_flight(), 0);
        assert_eq!(
            s.read_pool.idle.lock().len(),
            1,
            "pool retains at most read_pool_size idle connections"
        );
    }
}
