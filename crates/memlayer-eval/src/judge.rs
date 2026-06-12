// Generated with AI Coding Rules Hub
//! Claude client for answer generation and LLM-as-judge scoring.
//!
//! Shells out to the `claude` CLI (Claude Code) instead of calling the HTTP
//! API directly, so it uses whatever auth the current Claude Code session has.
//! Invocation: `claude -p "<prompt>" --model <model>`

use anyhow::{bail, Context, Result};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

const CLAUDE_TIMEOUT: Duration = Duration::from_secs(120);

const ANSWER_MODEL: &str = "claude-sonnet-4-6";
const JUDGE_MODEL: &str = "claude-haiku-4-5-20251001";

pub struct JudgeClient;

impl JudgeClient {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Call the answer model with the assembled system + user prompt.
    pub async fn answer(&self, system: &str, question: &str) -> Result<String> {
        let full_prompt = format!("{system}\n\n{question}");
        call_claude(&full_prompt, ANSWER_MODEL, 256).await
    }

    /// Call the judge model; returns `true` if verdict is YES.
    pub async fn judge(&self, judge_prompt: &str) -> Result<bool> {
        let prompt = format!(
            "You are a strict judge. Reply only YES or NO.\n\n{judge_prompt}"
        );
        let verdict = call_claude(&prompt, JUDGE_MODEL, 8).await?;
        let verdict = verdict.trim().to_uppercase();
        debug!(verdict = %verdict, "judge verdict");
        Ok(verdict.starts_with("YES"))
    }
}

async fn call_claude(prompt: &str, model: &str, _max_tokens: u32) -> Result<String> {
    let child = Command::new("claude")
        .args(["-p", prompt, "--model", model])
        // The Bedrock SDK used by Claude Code doesn't support socks5h:// proxies,
        // and Capital One's awsproxy sets ALL_PROXY/FTP_PROXY/GRPC_PROXY=socks5h://...
        // HTTPS_PROXY is an HTTP proxy and works fine; just clear the SOCKS ones.
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env_remove("FTP_PROXY")
        .env_remove("ftp_proxy")
        .env_remove("GRPC_PROXY")
        .env_remove("grpc_proxy")
        .spawn()
        .context("spawn claude CLI — is `claude` on PATH?")?;

    let output = timeout(CLAUDE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("claude CLI timed out after {}s", CLAUDE_TIMEOUT.as_secs()))?
        .context("wait for claude CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("claude CLI exited with {}: stderr={} stdout={}", output.status, stderr, stdout);
    }

    let text = String::from_utf8(output.stdout)
        .context("claude CLI output was not valid UTF-8")?
        .trim()
        .to_string();
    Ok(text)
}
