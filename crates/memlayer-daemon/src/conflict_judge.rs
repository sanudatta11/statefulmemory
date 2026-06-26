// Generated with AI Coding Rules Hub
//! LLM-backed implementation of [`ConflictClassifier`] (Spec 4).
//!
//! [`ClaudeConflictClassifier`] shells out to the `claude` CLI (same
//! proxy-strip pattern as `memlayer-extract`). It runs synchronously from
//! the per-project write thread by spinning up a per-thread
//! `tokio::runtime::Runtime` on the first call and reusing it thereafter.
//!
//! On any error (timeout, model unavailable, parse failure) the write path
//! falls back to the existing BM25 heuristic and logs a warning — LLM
//! classification is best-effort, never on the critical path.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use memlayer_core::config::ModelKind;
use memlayer_extract::claude_cli::ClaudeClient;
use memlayer_storage::conflict_judge::{ConflictClassifier, ConflictVerdict};

/// Maximum chars of content shown to the judge per observation. Keeps the
/// prompt under ~3 KB even for the longest observations.
const CONTENT_PREVIEW_CHARS: usize = 600;

thread_local! {
    /// Per-thread tokio runtime, initialised lazily on first classify call.
    static JUDGE_RT: RefCell<Option<tokio::runtime::Runtime>> = const { RefCell::new(None) };
}

fn with_rt<F, T>(f: F) -> anyhow::Result<T>
where
    F: FnOnce(&tokio::runtime::Runtime) -> anyhow::Result<T>,
{
    JUDGE_RT.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| anyhow::anyhow!("conflict judge RT init: {e}"))?;
            *opt = Some(rt);
        }
        f(opt.as_ref().unwrap())
    })
}

/// Production conflict classifier that shells out to the `claude` CLI.
pub struct ClaudeConflictClassifier {
    claude: Arc<dyn ClaudeClient>,
    model_id: String,
    timeout: Duration,
}

impl ClaudeConflictClassifier {
    pub fn new(claude: Arc<dyn ClaudeClient>, kind: ModelKind) -> Self {
        Self {
            claude,
            model_id: kind.cli_model_id().to_string(),
            timeout: Duration::from_secs(5),
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs);
        self
    }
}

impl ConflictClassifier for ClaudeConflictClassifier {
    fn classify(
        &self,
        old_title: &str,
        old_content: &str,
        new_title: &str,
        new_content: &str,
    ) -> anyhow::Result<ConflictVerdict> {
        let prompt = build_prompt(old_title, old_content, new_title, new_content);
        let model_id = self.model_id.clone();
        let client = self.claude.clone();
        let timeout = self.timeout;

        let raw = with_rt(|rt| {
            rt.block_on(async move {
                let fut = client.ask(&prompt, &model_id);
                tokio::time::timeout(timeout, fut)
                    .await
                    .map_err(|_| anyhow::anyhow!("conflict judge timed out after {}s", timeout.as_secs()))?
                    .map_err(|e| anyhow::anyhow!("conflict judge LLM call: {e}"))
            })
        })?;

        parse_verdict(&raw)
    }
}

fn build_prompt(old_title: &str, old_content: &str, new_title: &str, new_content: &str) -> String {
    let old_preview: String = old_content.chars().take(CONTENT_PREVIEW_CHARS).collect();
    let new_preview: String = new_content.chars().take(CONTENT_PREVIEW_CHARS).collect();
    format!(
        "You are classifying the relationship between two observations in a memory store.\n\
         \n\
         OLDER observation:\n\
         Title: {old_title}\n\
         Content: {old_preview}\n\
         \n\
         NEWER observation:\n\
         Title: {new_title}\n\
         Content: {new_preview}\n\
         \n\
         Output exactly one of:\n\
         SUPERSEDES        — the newer replaces the older entirely\n\
         CONFLICTS_WITH    — the two are mutually incompatible\n\
         COMPATIBLE        — both can coexist; the newer adds information\n\
         NOT_CONFLICT      — they are about different things despite similar titles\n\
         \n\
         Respond with only the label, nothing else."
    )
}

fn parse_verdict(raw: &str) -> anyhow::Result<ConflictVerdict> {
    let upper = raw.trim().to_ascii_uppercase();
    let first_word = upper.split_whitespace().next().unwrap_or("");
    match first_word {
        "SUPERSEDES"     => Ok(ConflictVerdict::Supersedes),
        "CONFLICTS_WITH" => Ok(ConflictVerdict::ConflictsWith),
        "COMPATIBLE"     => Ok(ConflictVerdict::Compatible),
        "NOT_CONFLICT"   => Ok(ConflictVerdict::NotConflict),
        other            => Err(anyhow::anyhow!(
            "unexpected verdict {:?}; expected SUPERSEDES|CONFLICTS_WITH|COMPATIBLE|NOT_CONFLICT",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_supersedes() {
        assert_eq!(parse_verdict("SUPERSEDES").unwrap(), ConflictVerdict::Supersedes);
        assert_eq!(parse_verdict("supersedes\n").unwrap(), ConflictVerdict::Supersedes);
    }

    #[test]
    fn parse_conflicts_with() {
        assert_eq!(parse_verdict("CONFLICTS_WITH").unwrap(), ConflictVerdict::ConflictsWith);
    }

    #[test]
    fn parse_compatible() {
        assert_eq!(parse_verdict("COMPATIBLE").unwrap(), ConflictVerdict::Compatible);
    }

    #[test]
    fn parse_not_conflict() {
        assert_eq!(parse_verdict("NOT_CONFLICT").unwrap(), ConflictVerdict::NotConflict);
        assert_eq!(parse_verdict("not_conflict extra text").unwrap(), ConflictVerdict::NotConflict);
    }

    #[test]
    fn parse_unknown_errors() {
        assert!(parse_verdict("YES").is_err());
        assert!(parse_verdict("").is_err());
        assert!(parse_verdict("idk").is_err());
    }

    #[test]
    fn build_prompt_includes_both_observations() {
        let p = build_prompt("old-title", "old-body", "new-title", "new-body");
        assert!(p.contains("old-title"));
        assert!(p.contains("old-body"));
        assert!(p.contains("new-title"));
        assert!(p.contains("new-body"));
        assert!(p.contains("SUPERSEDES"));
    }
}
