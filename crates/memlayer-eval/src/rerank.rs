// Generated with AI Coding Rules Hub
//! LLM rerank stage (spec-task-21, P3).
//!
//! Sends the top-N hybrid candidates to claude-4.5-haiku in a single prompt
//! and asks for a comma-separated ranking. The Haiku output is parsed
//! tolerantly (numbered lists, prose prefixes, garbage all degrade
//! gracefully) and used to reorder the candidates. If fewer than 5 valid
//! indices come back, we fall through to the input order (RRF passthrough).
//!
//! Why this design: the failure analysis on the P2 smoke (LoCoMo 65%)
//! showed that recall is fine — correct facts are present in the top-30 —
//! but the top-10 selection is wrong. An LLM rerank salvages those buried
//! correct facts at small cost (~$0.05 per smoke run, ~$2 per full LoCoMo).
//!
//! Cost gate (locked in grill session 2026-06-12): rerank should fire
//! every query for now; once entity-walk (P5 spec-task-31) lands it will
//! gate firing on top-2 score delta < 0.05. That cuts cost ~70%.
//!
//! Spec link: TS-10. Plan: P3 §5.2, §5.5.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;

use memlayer_extract::claude_cli::ClaudeClient;

/// Reranker model. Haiku is fast and cheap; Sonnet is overkill for picking
/// 10 from 30 candidates.
const RERANK_MODEL: &str = "claude-4.5-haiku";

/// Below this many parsed indices we treat the call as garbage and pass
/// the original RRF order through unchanged. Five is a heuristic — most
/// successful Haiku responses return 10 indices. Three or fewer almost
/// always indicates Haiku ignored the format ask.
const MIN_VALID_INDICES: usize = 5;

/// LLM rerank: takes top-N candidate memories (in RRF order) and returns
/// the top-K reordered by Haiku's relevance judgment. On parse failure or
/// transport error, returns the first K candidates (RRF passthrough).
///
/// The returned `Duration` measures only the rerank stage (Haiku call +
/// parse), so the runner can log it separately from BM25/dense retrieval
/// latency.
pub async fn rerank(
    claude: Arc<dyn ClaudeClient>,
    candidates: &[String],
    question: &str,
    top_k: usize,
) -> Result<(Vec<String>, Duration)> {
    if candidates.len() <= top_k {
        // Nothing to rerank — already at-or-below the target size.
        return Ok((candidates.to_vec(), Duration::ZERO));
    }

    let t0 = Instant::now();
    let prompt = build_rerank_prompt(candidates, question, top_k);

    let raw = match claude.ask(&prompt, RERANK_MODEL).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "rerank LLM call failed; using RRF passthrough");
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
pub fn build_rerank_prompt(
    candidates: &[String],
    question: &str,
    top_k: usize,
) -> String {
    let memories = candidates
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let preview: String = m.lines().take(3).collect::<Vec<_>>().join(" ");
            // Cap each line at 250 chars so a runaway memory doesn't dominate.
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

/// Tolerant parser for Haiku rerank output. Walks the response and
/// extracts every integer in `1..=max`, deduping while preserving order
/// of first occurrence.
///
/// Handles all four TS-10 fixture variants:
///   - clean: `"1,2,3,4,5,6,7,8,9,10"`
///   - numbered list: `"1.\n2.\n3.\n..."` (extracts the leading numbers)
///   - prose prefix: `"top: 1,2,3 then ..."`
///   - garbage: `"I don't know"` → empty Vec
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
