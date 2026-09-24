//! LLM rerank stage.
//!
//! Sends the top-N hybrid candidates to a Claude model (Haiku or Sonnet) in a
//! single prompt and asks for a comma-separated ranking. The response is
//! parsed tolerantly (numbered lists, prose prefixes, garbage all degrade
//! gracefully) and used to reorder the candidates. If fewer than 5 valid
//! indices come back, we fall through to the input order (RRF passthrough).
//!
//! Why this design: failure analysis on the eval P2 smoke (LoCoMo 65%)
//! showed that recall is fine — correct facts are present in the top-30 —
//! but the top-10 selection is wrong. An LLM rerank salvages those buried
//! correct facts at small cost (~$0.05 per smoke run, ~$2 per full LoCoMo).
//!
//! Spec links (retrieval-promotion): SC-5.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;

use statefulmemory_core::config::ModelKind;
use statefulmemory_extract::claude_cli::{ClaudeClient, HAIKU_MODEL};

/// Default reranker model id for the legacy `rerank()` entry point. Newer
/// callers (the daemon's `service::rerank`) pass a [`ModelKind`] explicitly
/// via [`ClaudeReranker::new`].
const DEFAULT_RERANK_MODEL: &str = HAIKU_MODEL;

/// Below this many parsed indices we treat the call as garbage and pass
/// the original RRF order through unchanged. Five is a heuristic — most
/// successful Haiku responses return 10 indices. Three or fewer almost
/// always indicates the model ignored the format ask.
const MIN_VALID_INDICES: usize = 5;

/// Reranker abstraction. The eval harness uses [`rerank`] (free function)
/// for backwards compat; the daemon uses [`ClaudeReranker`] which accepts a
/// [`ModelKind`] selectable at runtime.
pub const DEFAULT_RERANK_BUDGET: Duration = Duration::from_secs(5);
pub const DEFAULT_RERANK_CANDIDATE_LIMIT: usize = 32;

#[derive(Debug, Clone)]
pub struct BoundedRerankResult {
    pub hits: Vec<String>,
    pub duration: Duration,
    pub timed_out: bool,
}

#[async_trait::async_trait]
pub trait Reranker: Send + Sync {
    /// Rerank `candidates` against `question`, returning the top `top_k` in
    /// the new order. Implementations must degrade to RRF passthrough on
    /// transport / parse failure.
    async fn rerank(
        &self,
        candidates: &[String],
        question: &str,
        top_k: usize,
    ) -> Result<(Vec<String>, Duration)>;
}

/// Claude shell-out reranker selectable between Haiku and Sonnet.
pub struct ClaudeReranker {
    claude: Arc<dyn ClaudeClient>,
    model_id: String,
}

impl ClaudeReranker {
    /// Build a reranker for the given model role (`fast`/`capable` inherit
    /// the agent current model unless `STATEFULMEMORY_LLM_MODEL` is set).
    pub fn new(claude: Arc<dyn ClaudeClient>, kind: ModelKind) -> Self {
        Self {
            claude,
            model_id: kind.cli_model_id().to_string(),
        }
    }

    /// Build a reranker with an explicit model id string.
    pub fn with_model_id(claude: Arc<dyn ClaudeClient>, model_id: impl Into<String>) -> Self {
        Self {
            claude,
            model_id: model_id.into(),
        }
    }
}

#[async_trait::async_trait]
impl Reranker for ClaudeReranker {
    async fn rerank(
        &self,
        candidates: &[String],
        question: &str,
        top_k: usize,
    ) -> Result<(Vec<String>, Duration)> {
        rerank_with_model(
            self.claude.clone(),
            candidates,
            question,
            top_k,
            &self.model_id,
        )
        .await
    }
}

/// Free-function rerank kept for `statefulmemory-eval` callers that pre-date the
/// [`Reranker`] trait. Routes through the legacy default model.
pub async fn rerank(
    claude: Arc<dyn ClaudeClient>,
    candidates: &[String],
    question: &str,
    top_k: usize,
) -> Result<(Vec<String>, Duration)> {
    rerank_with_model(claude, candidates, question, top_k, DEFAULT_RERANK_MODEL).await
}

pub async fn rerank_with_budget(
    claude: Arc<dyn ClaudeClient>,
    candidates: &[String],
    question: &str,
    top_k: usize,
    budget: Duration,
    candidate_limit: usize,
) -> BoundedRerankResult {
    rerank_with_budget_model(
        claude,
        candidates,
        question,
        top_k,
        DEFAULT_RERANK_MODEL,
        budget,
        candidate_limit,
    )
    .await
}

pub async fn rerank_with_budget_model(
    claude: Arc<dyn ClaudeClient>,
    candidates: &[String],
    question: &str,
    top_k: usize,
    model_id: &str,
    budget: Duration,
    candidate_limit: usize,
) -> BoundedRerankResult {
    let limit = candidate_limit.max(top_k).min(candidates.len());
    let input = &candidates[..limit];
    let fallback = candidates.iter().take(top_k).cloned().collect::<Vec<_>>();
    let started = Instant::now();
    let future = rerank_with_model(claude, input, question, top_k, model_id);
    match tokio::time::timeout(budget, future).await {
        Ok(Ok((hits, _))) => BoundedRerankResult {
            hits,
            duration: started.elapsed(),
            timed_out: false,
        },
        Ok(Err(error)) => {
            tracing::warn!(%error, "bounded rerank failed; using candidate order");
            BoundedRerankResult {
                hits: fallback,
                duration: started.elapsed(),
                timed_out: false,
            }
        }
        Err(_) => {
            tracing::warn!(
                budget_ms = budget.as_millis(),
                "bounded rerank timed out; using candidate order"
            );
            BoundedRerankResult {
                hits: fallback,
                duration: started.elapsed(),
                timed_out: true,
            }
        }
    }
}

async fn rerank_with_model(
    claude: Arc<dyn ClaudeClient>,
    candidates: &[String],
    question: &str,
    top_k: usize,
    model_id: &str,
) -> Result<(Vec<String>, Duration)> {
    if candidates.len() <= top_k {
        return Ok((candidates.to_vec(), Duration::ZERO));
    }

    let t0 = Instant::now();
    let prompt = build_rerank_prompt(candidates, question, top_k);

    let raw = match claude.ask(&prompt, model_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, model = model_id, "rerank LLM call failed; using RRF passthrough");
            return Ok((
                candidates.iter().take(top_k).cloned().collect(),
                t0.elapsed(),
            ));
        }
    };

    let indices = parse_rerank_indices(&raw, candidates.len());

    let result: Vec<String> = if indices.len() < MIN_VALID_INDICES {
        tracing::warn!(
            parsed = indices.len(),
            min = MIN_VALID_INDICES,
            response_preview = &raw[..raw.len().min(120)],
            "rerank parse below threshold; using RRF passthrough"
        );
        candidates.iter().take(top_k).cloned().collect()
    } else {
        indices
            .into_iter()
            .take(top_k)
            .filter_map(|i| candidates.get(i).cloned())
            .collect()
    };

    Ok((result, t0.elapsed()))
}

/// Build the rerank prompt. We truncate each candidate to its first three
/// lines so the prompt stays under ~6KB even with 30 candidates — Haiku
/// with full evidence-window expansion would otherwise blow past the
/// context budget.
pub fn build_rerank_prompt(candidates: &[String], question: &str, top_k: usize) -> String {
    let memories = candidates
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let preview: String = m.lines().take(3).collect::<Vec<_>>().join(" ");
            let preview: String = preview.chars().take(250).collect();
            format!("[{}] {}", i + 1, preview)
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are ranking memories by relevance to a question.\n\
         \n\
         Question: {question}\n\
         \n\
         Candidate memories (1-indexed):\n\
         {memories}\n\
         \n\
         Output the indices of the top {top_k} most relevant memories, most\n\
         relevant first, as a single line of comma-separated integers.\n\
         Example: 3,1,7,2,5,9,4,6,8,10\n\
         \n\
         Respond with ONLY the comma-separated integers. No prose, no\n\
         explanation, no formatting."
    )
}

/// Tolerant parser for rerank output. Walks the response and extracts every
/// integer in `1..=max`, deduping while preserving order of first occurrence.
pub fn parse_rerank_indices(raw: &str, max: usize) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut current = String::new();

    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() {
            current.push(c as char);
        } else if !current.is_empty() {
            push_if_valid(&current, max, &mut seen, &mut out);
            current.clear();
        }
        i += 1;
    }
    if !current.is_empty() {
        push_if_valid(&current, max, &mut seen, &mut out);
    }
    out
}

fn push_if_valid(
    s: &str,
    max: usize,
    seen: &mut std::collections::HashSet<usize>,
    out: &mut Vec<usize>,
) {
    if let Ok(n) = s.parse::<usize>() {
        if (1..=max).contains(&n) {
            let idx = n - 1;
            if seen.insert(idx) {
                out.push(idx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct HangingClient;

    #[async_trait::async_trait]
    impl ClaudeClient for HangingClient {
        async fn ask(&self, _prompt: &str, _model: &str) -> Result<String> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    #[tokio::test]
    async fn bounded_rerank_times_out_to_input_order() {
        let candidates = (0..8)
            .map(|index| format!("candidate-{index}"))
            .collect::<Vec<_>>();
        let result = rerank_with_budget(
            Arc::new(HangingClient),
            &candidates,
            "question",
            3,
            Duration::from_millis(10),
            4,
        )
        .await;
        assert!(result.timed_out);
        assert_eq!(result.hits, candidates[..3].to_vec());
        assert!(result.duration < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn bounded_rerank_limits_candidate_input() {
        struct CannedClient;
        #[async_trait::async_trait]
        impl ClaudeClient for CannedClient {
            async fn ask(&self, _prompt: &str, _model: &str) -> Result<String> {
                Ok("1,2,3,4".into())
            }
        }
        let candidates = (0..8)
            .map(|index| format!("candidate-{index}"))
            .collect::<Vec<_>>();
        let result = rerank_with_budget(
            Arc::new(CannedClient),
            &candidates,
            "question",
            2,
            DEFAULT_RERANK_BUDGET,
            4,
        )
        .await;
        assert!(!result.timed_out);
        assert_eq!(result.hits, vec!["candidate-0", "candidate-1"]);
    }
}
