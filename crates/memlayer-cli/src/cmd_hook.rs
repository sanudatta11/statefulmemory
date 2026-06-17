//! `memlayer hook pre-tool` — Claude Code PreToolUse hook handler.
//!
//! Spec coverage: SC-1 (Grep mode markdown stdout), SC-2 (Read mode stderr
//! suggestion only). Always exits 0 so the agent's tool call is never
//! blocked. 500 ms hard timeout — slow / down daemons silently no-op.
//!
//! Hybrid behavior by tool:
//!   --tool Grep --pattern "<text>" → if matches exist, print markdown
//!     block to stdout that Claude sees as additional context.
//!   --tool Read --path "<path>"     → if matches exist, print a
//!     stderr-only hint suggesting the relevant `obs search`. Bodies
//!     never injected (path-only signal is too weak to inject blindly).

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use memlayer_proto as p;
use tonic::transport::Channel;

use crate::audit::{self, AuditEntry};
use crate::cli::PreToolArgs;

type Client = p::memlayer_client::MemlayerClient<Channel>;

/// 500 ms hard cap on the entire hook (RPC + render). On timeout, exit
/// silently — the agent's tool call MUST NOT be slowed down.
const HOOK_TIMEOUT: Duration = Duration::from_millis(500);

/// Maximum hits surfaced to the agent. Smaller than `obs search` default
/// because hooks are noisy when they paste 20 results.
const HOOK_HIT_LIMIT: i32 = 5;

/// Stopwords skipped during keyword extraction in Read mode. Short
/// path-segment tokens that appear in nearly every codebase (e.g.
/// `src`, `lib`, `tests`) carry no signal — letting them through would
/// match against every prior observation.
const STOPWORDS: &[&str] = &[
    "src", "crates", "lib", "mod", "main", "test", "tests",
    "index", "util", "utils", "common", "shared", "internal",
    "bin", "pkg", "cmd", "core", "api",
];

pub async fn dispatch(client: &mut Client, project_name: &str, args: PreToolArgs) -> ExitCode {
    let started = Instant::now();
    let tool = args.tool.as_str();

    let query = match tool {
        "Grep" => match args.pattern.as_deref() {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => {
                // No pattern → nothing to search. Silent no-op.
                return ExitCode::SUCCESS;
            }
        },
        "Read" => match args.path.as_deref() {
            Some(p) if !p.trim().is_empty() => keywords_from_path(Path::new(p)).join(" OR "),
            _ => return ExitCode::SUCCESS,
        },
        _ => {
            // Unknown tool — fail-silent. Future tools (Edit, Write, …) can
            // opt in by adding their own branch here.
            return ExitCode::SUCCESS;
        }
    };

    if query.is_empty() {
        return ExitCode::SUCCESS;
    }

    let req = p::SearchObservationsRequest {
        project_name: project_name.to_string(),
        query: query.clone(),
        r#type: None,
        scope: None,
        all_projects: false,
        limit: HOOK_HIT_LIMIT,
    };

    let result = tokio::time::timeout(HOOK_TIMEOUT, client.search_observations(req)).await;

    let observations = match result {
        Ok(Ok(resp)) => resp.into_inner().observations,
        // Timeout, transport error, daemon unavailable: fail-silent.
        _ => return record_and_exit(project_name, tool, &query, 0, started),
    };

    if observations.is_empty() {
        return record_and_exit(project_name, tool, &query, 0, started);
    }

    match tool {
        "Grep" => render_grep_markdown(&query, &observations),
        "Read" => render_read_hint(&query, observations.len()),
        _ => {}
    }

    record_and_exit(project_name, tool, &query, observations.len(), started)
}

fn record_and_exit(
    project: &str,
    tool: &str,
    query: &str,
    hits: usize,
    started: Instant,
) -> ExitCode {
    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "hook.pre-tool",
        project: Some(project),
        result_count: Some(hits),
        duration_ms: started.elapsed().as_millis(),
        query: audit::full_mode_enabled().then_some(format!("{tool}: {query}")),
        top_hits: None,
    });
    ExitCode::SUCCESS
}

/// Inject a markdown block to stdout. Claude Code routes hook stdout into
/// the model's next-turn context, so this adds memlayer-resident memory
/// before the Grep result appears.
fn render_grep_markdown(query: &str, observations: &[p::Observation]) {
    println!("## memlayer prior context for \"{query}\"");
    println!();
    for o in observations {
        let one_line = first_meaningful_line(&o.content);
        println!("- [#{}] {} ({}) — {}", o.id, o.title, o.r#type, one_line);
    }
}

/// Stderr-only hint: too weak a signal to inject the bodies, but worth
/// telling the agent to follow up with an explicit search.
fn render_read_hint(query: &str, hit_count: usize) {
    eprintln!(
        "memlayer hint: {hit_count} prior observations match \"{query}\" — \
         run `memlayer obs search \"{query}\"` for context",
    );
}

/// Take the first non-empty trimmed line, trimmed to 120 chars.
fn first_meaningful_line(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= 120 {
        line.to_string()
    } else {
        let mut s: String = line.chars().take(117).collect();
        s.push('…');
        s
    }
}

/// Naive path-keyword extraction: take the file stem + parent dir name,
/// split on `-_./`, filter to ≥3-char non-stopword tokens, lowercase.
/// Returns at most 4 tokens to keep the FTS5 query bounded.
pub fn keywords_from_path(path: &Path) -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        sources.push(stem.to_string());
    }
    if let Some(parent) = path.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()) {
        sources.push(parent.to_string());
    }

    let mut out: Vec<String> = Vec::new();
    for s in sources {
        for token in s.split(|c: char| c == '-' || c == '_' || c == '.' || c == '/') {
            if token.len() < 3 {
                continue;
            }
            let lower = token.to_lowercase();
            if STOPWORDS.contains(&lower.as_str()) {
                continue;
            }
            if !out.contains(&lower) {
                out.push(lower);
            }
            if out.len() >= 4 {
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_skip_stopwords_and_short_tokens() {
        let kw = keywords_from_path(Path::new("crates/memlayer-storage/src/write.rs"));
        // 'src' and 'crates' are stopwords; <3-char tokens skipped.
        assert!(!kw.contains(&"src".to_string()));
        assert!(!kw.contains(&"crates".to_string()));
        // 'memlayer-storage' splits into 'memlayer' + 'storage'; stem 'write'.
        assert!(kw.contains(&"write".to_string()), "missing 'write' in {kw:?}");
        assert!(
            kw.contains(&"memlayer".to_string()) || kw.contains(&"storage".to_string()),
            "expected token from parent dir, got {kw:?}",
        );
    }

    #[test]
    fn keywords_capped_at_4_tokens() {
        let kw = keywords_from_path(Path::new("a/b-c-d-e-f-g-h.rs"));
        assert!(kw.len() <= 4, "expected ≤4 keywords, got {}: {kw:?}", kw.len());
    }

    #[test]
    fn keywords_empty_for_path_with_only_stopwords() {
        let kw = keywords_from_path(Path::new("src/lib.rs"));
        assert!(kw.is_empty(), "expected empty, got {kw:?}");
    }

    #[test]
    fn first_meaningful_line_trims_long_input() {
        let body = "  \n\n   first content line that goes on for a while ".to_string()
            + &"x".repeat(200);
        let line = first_meaningful_line(&body);
        assert!(line.chars().count() <= 120);
        assert!(line.starts_with("first content"));
    }
}
