// Generated with AI Coding Rules Hub
//! Claude API client for answer generation and LLM-as-judge scoring.
//!
//! Uses the Anthropic Messages API directly via reqwest. Reads
//! `ANTHROPIC_API_KEY` from the environment. Includes exponential-backoff
//! retry for 429 / 529 (overload) responses.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANSWER_MODEL: &str = "claude-sonnet-4-6";
const JUDGE_MODEL: &str = "claude-haiku-4-5-20251001";
const MAX_RETRIES: u32 = 4;

#[derive(Debug, Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<Message<'a>>,
}

#[derive(Debug, Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

pub struct JudgeClient {
    client: reqwest::Client,
    api_key: String,
}

impl JudgeClient {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY must be set")?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("build reqwest client")?;
        Ok(Self { client, api_key })
    }

    /// Call the answer model: returns the model's answer string.
    pub async fn answer(&self, system: &str, question: &str) -> Result<String> {
        let req = MessagesRequest {
            model: ANSWER_MODEL,
            max_tokens: 256,
            system,
            messages: vec![Message { role: "user", content: question }],
        };
        self.call_with_retry(&req).await
    }

    /// Call the judge model: returns `true` if the answer is correct (YES),
    /// `false` if incorrect (NO).
    pub async fn judge(&self, judge_prompt: &str) -> Result<bool> {
        let req = MessagesRequest {
            model: JUDGE_MODEL,
            max_tokens: 8,
            system: "You are a strict judge. Reply only YES or NO.",
            messages: vec![Message { role: "user", content: judge_prompt }],
        };
        let verdict = self.call_with_retry(&req).await?;
        let verdict = verdict.trim().to_uppercase();
        debug!(verdict = %verdict, "judge verdict");
        Ok(verdict.starts_with("YES"))
    }

    async fn call_with_retry(&self, req: &MessagesRequest<'_>) -> Result<String> {
        let mut delay = Duration::from_millis(500);
        for attempt in 0..=MAX_RETRIES {
            let resp = self.client
                .post(ANTHROPIC_API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(req)
                .send()
                .await
                .context("send Claude API request")?;

            let status = resp.status();
            if status == 429 || status.as_u16() == 529 {
                if attempt == MAX_RETRIES {
                    bail!("Claude API rate-limited after {MAX_RETRIES} retries (status {status})");
                }
                warn!(attempt, status = %status, delay_ms = delay.as_millis(), "rate limited — retrying");
                sleep(delay).await;
                delay *= 2;
                continue;
            }

            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                bail!("Claude API error {status}: {body}");
            }

            let body: MessagesResponse = resp.json().await.context("parse Claude API response")?;
            let text = body.content.into_iter()
                .find(|b| b.kind == "text")
                .and_then(|b| b.text)
                .ok_or_else(|| anyhow!("no text block in Claude API response"))?;
            return Ok(text);
        }
        unreachable!()
    }
}
