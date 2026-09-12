// Generated with AI Coding Rules Hub
//! LLM client for answer generation and judge scoring.
//!
//! Shells out to the detected agent CLI via [`memlayer_extract::claude_cli`] so
//! eval uses the same mapping as the daemon (role → provider id, else agent
//! default). Override with `MEMLAYER_LLM_MODEL`.

use anyhow::Result;
use tracing::debug;

use memlayer_extract::claude_cli::{ClaudeCliClient, ClaudeClient, HAIKU_MODEL, SONNET_MODEL};

pub struct JudgeClient {
    inner: ClaudeCliClient,
}

impl JudgeClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: ClaudeCliClient::new(),
        })
    }

    /// Call the answer model with the assembled system + user prompt.
    pub async fn answer(&self, system: &str, question: &str) -> Result<String> {
        let full_prompt = format!("{system}\n\n{question}");
        self.inner.ask(&full_prompt, SONNET_MODEL).await
    }

    /// Call the judge model; returns `true` if verdict is YES.
    pub async fn judge(&self, judge_prompt: &str) -> Result<bool> {
        let prompt = format!("You are a strict judge. Reply only YES or NO.\n\n{judge_prompt}");
        let verdict = self.inner.ask(&prompt, HAIKU_MODEL).await?;
        let verdict = verdict.trim().to_uppercase();
        debug!(verdict = %verdict, "judge verdict");
        Ok(verdict.starts_with("YES"))
    }
}
