//! `ClaudeCliExtractor` — a generic extractor that shells out to the
//! `claude` CLI for any supported model id (`claude-haiku-4-5`,
//! `claude-sonnet-4-6`). Replaces the per-model wrappers; the daemon's
//! extract worker pool (rp-t6) selects between Haiku and Sonnet purely by
//! constructor arg.
//!
//! Pipeline per call:
//!   1. Build the extraction prompt from the turn(s).
//!   2. Send to `claude --model <id>` via the [`ClaudeClient`] trait.
//!   3. Parse the JSON-ish response into `Vec<Fact>` via [`parse_facts`].
//!
//! Errors at any stage are returned to the caller; the daemon worker
//! drops failed tasks with a tracing warning (SC-7 gating means extract
//! failures are best-effort).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::claude_cli::ClaudeClient;
use crate::prompt::{build_extraction_prompt, parse_facts};
use crate::{Fact, Turn};

/// Extractor that produces facts via a `claude` shell-out using a
/// caller-supplied model id and timeout. Cheaply cloneable (Arc inside).
#[derive(Clone)]
pub struct ClaudeCliExtractor {
    client: Arc<dyn ClaudeClient>,
    model_id: String,
    /// Per-call timeout. Currently informational (`ClaudeClient::ask`
    /// enforces its own internal timeout); kept on the struct so callers
    /// can surface `cfg.extract.timeout_secs` consistently.
    pub timeout: Duration,
}

impl ClaudeCliExtractor {
    pub fn new(client: Arc<dyn ClaudeClient>, model_id: impl Into<String>, timeout: Duration) -> Self {
        Self {
            client,
            model_id: model_id.into(),
            timeout,
        }
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Build prompt → ask → parse for one turn. Returns `Vec<Fact>` (may be
    /// empty if the model returns no facts; that's not an error).
    pub async fn extract(&self, turn: &Turn) -> Result<Vec<Fact>> {
        let turns = std::slice::from_ref(turn);
        let prompt = build_extraction_prompt(turns, None);
        let raw = self.client.ask(&prompt, &self.model_id).await?;
        parse_facts(&raw, turns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_cli::MockClaudeClient;

    fn turn() -> Turn {
        Turn {
            speaker: "system".into(),
            text: "team picked pgx for raw SQL access".into(),
            obs_id: 7,
            session_id: Some("s1".into()),
        }
    }

    #[tokio::test]
    async fn extract_returns_empty_when_model_says_no_facts() {
        // The Haiku format expected by parse_facts uses {"facts": [...]}.
        // An empty array is valid; should produce zero facts, not error.
        let client = Arc::new(MockClaudeClient::with_responses(vec![
            r#"{"facts":[]}"#.to_string(),
        ]));
        let ex = ClaudeCliExtractor::new(client, "claude-haiku-4-5", Duration::from_secs(5));
        let facts = ex.extract(&turn()).await.unwrap();
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn extract_propagates_claude_error() {
        // No queued responses → MockClaudeClient errors → extract bubbles up.
        let client = Arc::new(MockClaudeClient::with_responses(Vec::<String>::new()));
        let ex = ClaudeCliExtractor::new(client, "claude-sonnet-4-6", Duration::from_secs(5));
        assert!(ex.extract(&turn()).await.is_err());
    }

    #[test]
    fn model_id_is_preserved() {
        let client = Arc::new(MockClaudeClient::with_responses(Vec::<String>::new()));
        let ex = ClaudeCliExtractor::new(client, "claude-sonnet-4-6", Duration::from_secs(30));
        assert_eq!(ex.model_id(), "claude-sonnet-4-6");
        assert_eq!(ex.timeout, Duration::from_secs(30));
    }
}
