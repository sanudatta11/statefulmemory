// Generated with AI Coding Rules Hub
//! Retrieval wrapper: run a tokenized FTS5 BM25 query and measure latency.
//!
//! We bypass `memlayer_storage::read::search` because its sanitizer wraps the
//! whole query in double-quotes (phrase search) — fine for ad-hoc UI queries
//! but useless for benchmark questions like "When did Caroline go to the LGBTQ
//! support group?" which never appear verbatim in any memory.
//!
//! Strategy (3-tier, spec-task-2 / TS-1):
//!   * Tier-1 (phrase): if ≥2 surviving tokens, emit the full quoted phrase
//!     in input order. BM25 ranks exact-phrase matches highest.
//!   * Tier-2 (bigrams): adjacent quoted bigrams over surviving tokens.
//!     Catches near-phrase matches with intervening stopwords.
//!   * Tier-3 (OR fallback): each individual token, ORed together. Preserves
//!     recall against memories that only share scattered tokens.
//! All three tiers are joined with OR. Per-token quote-escape and the
//! existing stopword set are preserved.
//!
//! Field-weighted BM25 (spec-task-3 / TS-14): the SELECT applies per-column
//! weights matching `observations_fts` order (title, content, tool_name,
//! type, topic_key) — title=5, content=1, tool_name=0.5, type=0.5,
//! topic_key=2. Title and topic_key carry the strongest semantic signal
//! per benchmark question, so BM25 ranks them above raw conversational
//! content while keeping tool_name/type as low-weight tiebreakers.
//!
//! Evidence expansion (spec-task-3 / TS-14): `expand_evidence` walks ±N
//! observations around a seed obs id, clamped to the same `session_id`
//! as the seed. Used by the runner to give the judge surrounding turns
//! without bleeding across sessions.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

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
    // Survive stopword/length filter, preserve input order for phrase emission.
    let raw_tokens: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_lowercase())
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect();

    if raw_tokens.is_empty() {
        return "\"\"".to_string();
    }

    // Per-token quote-escape (FTS5 grammar safety).
    let escape = |t: &str| t.replace('"', "");

    // Single-token query: skip phrase/bigram tiers, just quote it (Tier-3 only,
    // no OR needed). Matches the original single-token behaviour byte-for-byte.
    if raw_tokens.len() == 1 {
        return format!("\"{}\"", escape(&raw_tokens[0]));
    }

    let mut clauses: Vec<String> = Vec::new();

    // Tier-1: full phrase over surviving tokens, input order preserved.
    let phrase = raw_tokens
        .iter()
        .map(|t| escape(t))
        .collect::<Vec<_>>()
        .join(" ");
    clauses.push(format!("\"{phrase}\""));

    // Tier-2: adjacent bigrams.
    for win in raw_tokens.windows(2) {
        clauses.push(format!("\"{} {}\"", escape(&win[0]), escape(&win[1])));
    }

    // Tier-3: per-token OR fallback (dedup so repeated words don't bloat the
    // query; order doesn't matter for OR clauses).
    let mut seen = std::collections::HashSet::new();
    for tok in &raw_tokens {
        if seen.insert(tok.clone()) {
            clauses.push(format!("\"{}\"", escape(tok)));
        }
    }

    clauses.join(" OR ")
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
    // Field-weighted BM25 weights match observations_fts column order:
    //   title=5, content=1, tool_name=0.5, type=0.5, topic_key=2.
    // See V1__init.sql:54.
    let mut stmt = conn
        .prepare(
            "SELECT title, content FROM observations
              WHERE id IN (
                  SELECT rowid FROM observations_fts
                   WHERE observations_fts MATCH ?1
                   ORDER BY bm25(observations_fts, 5.0, 1.0, 0.5, 0.5, 2.0) ASC
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

/// Expand a retrieval seed by returning the ±`window` surrounding raw
/// observation rows (title, content), clamped to the same `session_id`
/// as the seed and filtered to non-deleted rows. Returns rows in
/// ascending `id` order.
///
/// Behaviour:
///   * Looks up the seed's `session_id`. If the seed doesn't exist or is
///     soft-deleted, returns an empty vector (not an error).
///   * `window=0` returns just the seed row (or empty if missing).
///   * May return fewer than `2*window+1` rows when the window straddles
///     session boundaries or holes in the id sequence.
///
/// Used by the runner (spec-task-3 / TS-14) to feed the judge a small
/// neighbourhood of conversational context around each retrieved hit
/// without bleeding across session boundaries.
pub fn expand_evidence(
    conn: &rusqlite::Connection,
    obs_id: i64,
    window: u8,
) -> Result<Vec<(String, String)>> {
    // 1. Read seed obs's session_id (skip soft-deleted rows).
    let seed_session: Option<String> = conn
        .query_row(
            "SELECT session_id FROM observations
              WHERE id = ?1 AND deleted_at IS NULL",
            params![obs_id],
            |row| row.get(0),
        )
        .optional()
        .context("look up seed observation session_id")?;

    let Some(session_id) = seed_session else {
        return Ok(Vec::new());
    };

    // 2. Walk ±window observations clamped to the same session.
    let lo = obs_id.saturating_sub(window as i64);
    let hi = obs_id.saturating_add(window as i64);

    let mut stmt = conn
        .prepare(
            "SELECT title, content FROM observations
              WHERE id BETWEEN ?1 AND ?2
                AND session_id = ?3
                AND deleted_at IS NULL
              ORDER BY id ASC",
        )
        .context("prepare expand_evidence query")?;
    let rows = stmt
        .query_map(params![lo, hi, session_id], |row| {
            let title: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((title, content))
        })
        .context("run expand_evidence query")?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.context("read expand_evidence row")?);
    }
    Ok(out)
}
