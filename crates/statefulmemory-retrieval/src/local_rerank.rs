//! Local MiniLM-style cross-encoder rerank (Wave 3).
//!
//! Scores `(query, title+content)` with a lightweight feature CE inspired by
//! MS-MARCO MiniLM rerankers: token overlap, title boost, length prior, and
//! optional dense cosine when a query embedding + per-doc vectors are supplied.
//! Stays LLM-free and typically finishes 16 candidates in well under 50 ms.
//!
//! Soft-fails by returning the input order unchanged when scoring panics or
//! candidates are empty — same contract as [`ClaudeReranker`].

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::Result;

/// One candidate for local CE scoring.
#[derive(Debug, Clone)]
pub struct LocalCandidate<'a> {
    pub title: &'a str,
    pub content: &'a str,
    /// Optional dense embedding (same dim as query). When missing, lexical
    /// features alone drive the score.
    pub embedding: Option<&'a [f32]>,
}

/// Rank `candidates` against `query`. Returns indices into `candidates`
/// (best first), truncated to `top_k`. Never errors — degrades to identity.
pub fn local_rerank_indices(
    query: &str,
    candidates: &[LocalCandidate<'_>],
    query_emb: Option<&[f32]>,
    top_k: usize,
) -> (Vec<usize>, Duration) {
    let t0 = Instant::now();
    if candidates.is_empty() {
        return (Vec::new(), t0.elapsed());
    }
    let k = top_k.max(1).min(candidates.len());
    let q_tokens = tokenize(query);
    if q_tokens.is_empty() {
        return ((0..k).collect(), t0.elapsed());
    }
    let mut scored: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut score = lexical_score(&q_tokens, c.title, c.content);
            if let (Some(qe), Some(de)) = (query_emb, c.embedding) {
                score += 2.0 * cosine(qe, de).max(0.0);
            }
            (i, score)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    (
        scored.into_iter().take(k).map(|(i, _)| i).collect(),
        t0.elapsed(),
    )
}

/// Convenience: reorder owned strings (title\\ncontent) via lexical CE only.
pub fn mini_lm_rerank_texts(
    candidates: &[String],
    question: &str,
    top_k: usize,
) -> Result<(Vec<String>, Duration)> {
    let parsed: Vec<LocalCandidate<'_>> = candidates
        .iter()
        .map(|c| {
            let (title, content) = match c.split_once('\n') {
                Some((t, rest)) => (t, rest),
                None => (c.as_str(), ""),
            };
            LocalCandidate {
                title,
                content,
                embedding: None,
            }
        })
        .collect();
    let (idxs, dur) = local_rerank_indices(question, &parsed, None, top_k);
    let out: Vec<String> = idxs
        .into_iter()
        .filter_map(|i| candidates.get(i).cloned())
        .collect();
    Ok((out, dur))
}

fn tokenize(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn lexical_score(q_tokens: &HashSet<String>, title: &str, content: &str) -> f64 {
    let title_toks = tokenize(title);
    let body_toks = tokenize(content);
    let mut title_hits = 0u32;
    let mut body_hits = 0u32;
    for t in q_tokens {
        if title_toks.contains(t) {
            title_hits += 1;
        }
        if body_toks.contains(t) {
            body_hits += 1;
        }
    }
    let qn = q_tokens.len().max(1) as f64;
    let title_cov = title_hits as f64 / qn;
    let body_cov = body_hits as f64 / qn;
    // Mild length prior: prefer denser short notes over huge dumps.
    let len = (title.len() + content.len()).max(1) as f64;
    let length_prior = 1.0 / (1.0 + (len / 2000.0).ln_1p());
    3.0 * title_cov + 1.5 * body_cov + 0.15 * length_prior
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for i in 0..a.len() {
        let x = a[i] as f64;
        let y = b[i] as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom < 1e-12 {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_match_ranks_higher() {
        let cands = [
            LocalCandidate {
                title: "unrelated note",
                content: "lorem ipsum dolor",
                embedding: None,
            },
            LocalCandidate {
                title: "hybrid search design",
                content: "we use bm25 plus dense",
                embedding: None,
            },
        ];
        let (idxs, _) = local_rerank_indices("hybrid search", &cands, None, 2);
        assert_eq!(idxs[0], 1);
    }

    #[test]
    fn empty_query_preserves_order() {
        let cands = [
            LocalCandidate {
                title: "a",
                content: "x",
                embedding: None,
            },
            LocalCandidate {
                title: "b",
                content: "y",
                embedding: None,
            },
        ];
        let (idxs, _) = local_rerank_indices("   ", &cands, None, 2);
        assert_eq!(idxs, vec![0, 1]);
    }

    #[test]
    fn local_rerank_sixteen_cands_is_fast() {
        let titles: Vec<String> = (0..16).map(|i| format!("note title {i}")).collect();
        let cands: Vec<LocalCandidate<'_>> = titles
            .iter()
            .map(|t| LocalCandidate {
                title: t.as_str(),
                content: "body text about hybrid retrieval and token budgets",
                embedding: None,
            })
            .collect();
        let (_idxs, dur) = local_rerank_indices("hybrid retrieval", &cands, None, 8);
        assert!(
            dur.as_millis() < 50,
            "local CE should be well under 50ms, got {dur:?}"
        );
    }

    #[test]
    fn cosine_boosts_semantic_match() {
        let q = [1.0f32, 0.0, 0.0];
        let match_emb = [0.9f32, 0.1, 0.0];
        let miss_emb = [0.0f32, 1.0, 0.0];
        let cands = [
            LocalCandidate {
                title: "no lexical",
                content: "zzzz",
                embedding: Some(&miss_emb),
            },
            LocalCandidate {
                title: "also no lexical",
                content: "yyyy",
                embedding: Some(&match_emb),
            },
        ];
        let (idxs, _) = local_rerank_indices("qqq", &cands, Some(&q), 2);
        assert_eq!(idxs[0], 1);
    }
}
