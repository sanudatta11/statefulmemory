// Generated with AI Coding Rules Hub
//! Shell-out client for the local `claude` CLI.
//!
//! Mirrors the proxy-strip + model selection pattern proven in
//! `memlayer-eval/src/judge.rs:40-65`. Capital One's enterprise environment
//! sets `ALL_PROXY=socks5h://...` for awsproxy; the Bedrock SDK behind
//! `claude` rejects socks5h URIs, so we drop those vars before spawning.
//!
//! For tests, `MockClaudeClient` lets us script responses without shelling
//! out (TS-6 / TS-7 in P2 use this).

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

/// Timeout for a single Claude CLI call (Bedrock proxy can be slow).
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(120);

pub const HAIKU_MODEL: &str = "claude-4.5-haiku";

/// Abstract Claude client. Real impl shells out; mock returns canned output.
#[async_trait]
pub trait ClaudeClient: Send + Sync {
    /// Send a single prompt to the named model, return the raw text response
    /// (trimmed of trailing whitespace). Errors are wrapped with anyhow context.
    async fn ask(&self, prompt: &str, model: &str) -> Result<String>;
}

/// Production client: shells out to the `claude` CLI on PATH.
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
        let child = Command::new("claude")
            .args(["-p", prompt, "--model", model, "--output-format", "text"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Same proxy strip as memlayer-eval/src/judge.rs:46-51 — Bedrock SDK
            // doesn't speak socks5h, which Capital One's awsproxy sets.
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
            bail!(
                "claude CLI exited with {}: stderr={} stdout={}",
                output.status,
                stderr,
                stdout
            );
        }

        let text = String::from_utf8(output.stdout)
            .context("claude CLI output was not valid UTF-8")?
            .trim()
            .to_string();
        if text.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "claude CLI returned empty response for model '{}' (bad model ID?): stderr={}",
                model,
                stderr
            );
        }
        debug!(model, response_len = text.len(), "claude CLI ok");
        Ok(text)
    }
}

/// Test double: deque of canned responses, popped in FIFO order. Construct
/// with [`MockClaudeClient::with_responses`]; tests can also count calls via
/// the `Arc<Mutex<...>>` interior. Signal exhaustion by popping a `None`,
/// at which point `ask` returns an error rather than panicking.
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
