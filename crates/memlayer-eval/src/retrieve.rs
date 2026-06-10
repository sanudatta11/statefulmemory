// Generated with AI Coding Rules Hub
//! Retrieval wrapper: call `memlayer_storage::read::search` and measure latency.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use memlayer_core::paths;
use memlayer_storage::{read, ProjectRegistry};

pub struct RetrieveResult {
    /// Top-k observations as formatted strings (title + "\n" + content).
    pub hits: Vec<String>,
    /// Raw SQLite FTS5 retrieval time.
    pub latency: Duration,
}

/// Open a read connection to `project` inside `data_dir` and run FTS5 BM25
/// search for `query`, returning up to `k` results.
pub fn retrieve(
    data_dir: &Path,
    project: &str,
    query: &str,
    k: i32,
) -> Result<RetrieveResult> {
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);
    paths::ensure_dirs(data_dir).ok();

    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project_state = registry.get_or_open(project)
        .with_context(|| format!("open project '{project}' for retrieval"))?;
    let conn = project_state.open_read_conn()
        .context("open read connection")?;

    let t0 = Instant::now();
    let observations = read::search(&conn, query, None, None, k)
        .with_context(|| format!("FTS5 search for '{query}'"))?;
    let latency = t0.elapsed();

    let hits = observations
        .into_iter()
        .map(|o| format!("{}\n{}", o.title, o.content))
        .collect();

    Ok(RetrieveResult { hits, latency })
}
