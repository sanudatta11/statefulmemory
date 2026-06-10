// Generated with AI Coding Rules Hub
//! Retrieval wrapper: run a tokenized FTS5 BM25 query and measure latency.
//!
//! We bypass `memlayer_storage::read::search` because its sanitizer wraps the
//! whole query in double-quotes (phrase search) — fine for ad-hoc UI queries
//! but useless for benchmark questions like "When did Caroline go to the LGBTQ
//! support group?" which never appear verbatim in any memory.
//!
//! Strategy: split the question into alphanumeric tokens, drop stopwords/short
//! tokens, lowercase, OR them together. Each token is itself wrapped in
//! double-quotes so embedded punctuation can't break the FTS5 grammar.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::params;

use memlayer_core::paths;
use memlayer_storage::ProjectRegistry;

pub struct RetrieveResult {
    pub hits: Vec<String>,
    pub latency: Duration,
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
    "of", "in", "on", "at", "to", "for", "with", "by", "from", "as",
    "and", "or", "but", "not", "do", "does", "did", "have", "has", "had",
    "what", "when", "where", "who", "whom", "why", "how", "which", "that",
    "this", "these", "those", "it", "its", "they", "them", "their",
    "i", "me", "my", "you", "your", "we", "us", "our",
    "would", "could", "should", "will", "shall", "may", "might", "can",
    "go", "goes", "went", "going", "gone",
];

fn tokenize(query: &str) -> String {
    let mut tokens: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_lowercase())
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect();
    tokens.sort();
    tokens.dedup();
    if tokens.is_empty() {
        return "\"\"".to_string();
    }
    // Quote each token to neutralise FTS5 specials, OR them together.
    tokens
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

pub fn retrieve(
    data_dir: &Path,
    project: &str,
    query: &str,
    k: i32,
) -> Result<RetrieveResult> {
    std::env::set_var("MEMLAYER_DATA_DIR", data_dir);
    paths::ensure_dirs(data_dir).ok();

    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project_state = registry
        .get_or_open(project)
        .with_context(|| format!("open project '{project}' for retrieval"))?;
    let conn = project_state
        .open_read_conn()
        .context("open read connection")?;

    let fts_query = tokenize(query);
    let limit = k.clamp(1, 100);

    let t0 = Instant::now();
    let mut stmt = conn
        .prepare(
            "SELECT title, content FROM observations
              WHERE id IN (
                  SELECT rowid FROM observations_fts
                   WHERE observations_fts MATCH ?1
                   ORDER BY bm25(observations_fts) ASC
                   LIMIT ?2
              )
                AND deleted_at IS NULL
              ORDER BY id DESC",
        )
        .context("prepare FTS5 search")?;
    let rows = stmt
        .query_map(params![fts_query, limit], |row| {
            let title: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok(format!("{title}\n{content}"))
        })
        .context("run FTS5 search")?;
    let mut hits = Vec::new();
    for r in rows {
        hits.push(r.context("read FTS5 row")?);
    }
    let latency = t0.elapsed();

    Ok(RetrieveResult { hits, latency })
}
