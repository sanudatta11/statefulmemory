//! Per-project I/O lock for eval retrieval.
//!
//! Concurrent LoCoMo queries often hit the same `locomo-conv-*` project.
//! Each `ProjectRegistry::get_or_open` spawns a write thread that runs
//! `PRAGMA journal_mode=WAL` on the project SQLite file; racing those opens
//! (and concurrent wipe/rebuild of `<project>.vec.db`) surfaces as
//! `pragma journal_mode: disk I/O error`.
//!
//! Hold [`with_project_lock`] around any open/migrate/rebuild for a given
//! project name so hybrid retrieve stays safe under `MEMLAYER_EVAL_CONCURRENCY`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static PROJECT_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn project_mutex(project: &str) -> Arc<Mutex<()>> {
    let map = PROJECT_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(project.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Run `f` while holding the per-project I/O lock.
pub fn with_project_lock<R>(project: &str, f: impl FnOnce() -> R) -> R {
    let mutex = project_mutex(project);
    let _guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
    f()
}
