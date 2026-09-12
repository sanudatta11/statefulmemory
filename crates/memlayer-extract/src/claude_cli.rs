//! LLM completion client used by extract / rerank / conflict / Decide.
//!
//! Production impl shells out to whichever coding-agent CLI is installed
//! (see [`crate::agent_cli`]). Tests use [`MockClaudeClient`].

use anyhow::{bail, Result};
use async_trait::async_trait;
use std::sync::Arc;

use crate::agent_cli;

/// Role aliases kept so existing callers compile. The agent CLI maps these
/// to a provider-specific id, or omits `--model` when the provider has no
/// mapping (Cursor, Copilot, Codex, …).
pub const HAIKU_MODEL: &str = "fast";
pub const SONNET_MODEL: &str = "capable";

/// Abstract completion client. Real impl shells out; mock returns canned output.
#[async_trait]
pub trait ClaudeClient: Send + Sync {
    /// Send a prompt. `model` is a role (`fast` / `capable`), a legacy
    /// alias (`haiku` / `sonnet`), a vendor id, or empty (agent default).
    async fn ask(&self, prompt: &str, model: &str) -> Result<String>;
}

/// Production client: detects Cursor, Copilot, Gemini, Codex, Claude, and
/// peers. Override with `MEMLAYER_LLM_BIN` / `MEMLAYER_LLM_PROVIDER`.
/// `MEMLAYER_LLM_MODEL` (or legacy `MEMLAYER_CLAUDE_MODEL`) forces a model id.
pub struct ClaudeCliClient;

impl ClaudeCliClient {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeCliClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ClaudeClient for ClaudeCliClient {
    async fn ask(&self, prompt: &str, model: &str) -> Result<String> {
        let requested = agent_cli::effective_requested(model);

        let Some(provider) = agent_cli::detect_provider() else {
            bail!(
                "no agent CLI found for LLM calls (looked for {}). \
                 Install your agent's CLI or set MEMLAYER_LLM_BIN.",
                agent_cli::known_binaries().join(", ")
            );
        };
        agent_cli::invoke(&provider, prompt, &requested).await
    }
}

pub use agent_cli::{looks_like_unavailable_model, map_model};

/// Test double: deque of canned responses, popped in FIFO order.
pub struct MockClaudeClient {
    responses: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>>,
    call_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl MockClaudeClient {
    pub fn with_responses<I, S>(responses: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let q: std::collections::VecDeque<String> =
            responses.into_iter().map(|s| s.into()).collect();
        Self {
            responses: Arc::new(parking_lot::Mutex::new(q)),
            call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl ClaudeClient for MockClaudeClient {
    async fn ask(&self, _prompt: &str, _model: &str) -> Result<String> {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut q = self.responses.lock();
        match q.pop_front() {
            Some(s) => Ok(s),
            None => bail!("MockClaudeClient: response queue exhausted"),
        }
    }
}
