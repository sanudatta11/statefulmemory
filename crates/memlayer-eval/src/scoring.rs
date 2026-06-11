// Generated with AI Coding Rules Hub
//! Additive hybrid scoring (TS-22, SC-13).
//!
//! Replaces RRF fusion with the additive formula Mem0's audit (spec §3)
//! flagged as their largest accuracy lever (~12pp on LoCoMo). Each candidate
//! fact accumulates three [0, 1]-bounded signals, then is ranked by their
//! sum:
//!
//!     combined(f) = sem(f) + bm25(f) + entity_boost(f)
//!
//! Where:
//!   * `sem`  — dense ANN similarity. Computed as `1.0 - normalized(distance)`
//!     so closer vectors score higher. Facts not in the dense top-N: 0.0.
//!   * `bm25` — FTS5 BM25 score, sign-flipped (FTS5 returns negatives, lower
//!     is better) and min-max normalized to [0, 1]. Facts not in the BM25
//!     top-N: 0.0.
//!   * `entity_boost` — pre-computed by the caller from query-entity ↔
//!     fact-entity matches. Capped at 1.0.
//!
//! The retriever in `retrieve_facts.rs` builds the per-retriever id+score
//! lists, hands them to [`build_score_map`], then sorts the resulting facts
//! by `ScoreComponents::combined` to pick top-k.
//!
//! Spec: retrieval-upgrade-v1 §6, TS-22, SC-13. Plan: P2 §4.5.

use std::collections::HashMap;

/// Per-fact additive score components. All three signals are normalized to
/// [0, 1] **before** they reach this struct so callers can sum them safely.
#[derive(Debug, Clone, Default)]
pub struct ScoreComponents {
    /// Dense ANN similarity (1.0 - normalized L2 distance). 0.0 when the
    /// fact is not in the dense top-N.
    pub sem: f32,
    /// Field-weighted BM25 score, sign-flipped and min-max normalized.
    /// 0.0 when the fact is not in the BM25 top-N.
    pub bm25: f32,
    /// Entity-boost contribution: 0.5 per matched query entity, capped at
    /// 1.0. 0.0 when no query entity links to this fact.
    pub entity_boost: f32,
}

impl ScoreComponents {
    /// Final ranking score for this fact. Caller sorts descending to pick
    /// top-k; ties are caller's responsibility (we return f32 with no
    /// implied ordering for equal sums).
    pub fn combined(&self) -> f32 {
        self.sem + self.bm25 + self.entity_boost
    }
}

/// Min-max normalize a slice of scores to [0, 1].
///
/// Returns a vector of zeros when all inputs are equal — this prevents
/// divide-by-zero from a degenerate single-hit retriever. Empty input
/// returns empty output (no panic). NaN inputs are treated as if they
/// equal the running min/max, which keeps the function total even on a
/// caller-side bug.
pub fn min_max_normalize(scores: &[f32]) -> Vec<f32> {
    if scores.is_empty() {
        return Vec::new();
    }
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &s in scores {
        if s < min {
            min = s;
        }
        if s > max {
            max = s;
        }
    }
    let range = max - min;
    if range <= 0.0 || !range.is_finite() {
        // Constant input (or all-NaN) -> all zeros, never NaN.
        return vec![0.0; scores.len()];
    }
    scores.iter().map(|s| (s - min) / range).collect()
}

/// True iff every element equals the first. Empty -> true (vacuously).
fn all_equal(xs: &[f32]) -> bool {
    match xs.first() {
        None => true,
        Some(&first) => xs.iter().all(|x| *x == first),
    }
}

/// Build a per-fact score map by combining three retriever outputs.
///
/// * `bm25_hits` — `(fact_id, raw bm25)` from the FTS5 retriever. FTS5
///   returns negative scores where lower is better; this fn flips the
///   sign internally and min-max normalizes. Facts not in this list get
///   `bm25 = 0.0`.
/// * `dense_hits` — `(fact_id, L2 distance)` from the vec0 ANN retriever.
///   Lower distance is better; we min-max normalize and invert
///   (`sem = 1.0 - norm_distance`). Facts not in this list get `sem = 0.0`.
/// * `entity_boost_per_fact` — pre-aggregated boost values keyed by
///   `fact_id`. The caller (retrieve_facts) computes these from query
///   entities ↔ entity_links lookups (vec sim + name match).
///
/// The returned map is keyed by `fact_id` (i64) and includes every fact
/// that appears in any of the three sources. Caller sorts by
/// `ScoreComponents::combined()` to rank.
pub fn build_score_map(
    bm25_hits: &[(i64, f64)],
    dense_hits: &[(i64, f64)],
    entity_boost_per_fact: &HashMap<i64, f32>,
) -> HashMap<i64, ScoreComponents> {
    let mut out: HashMap<i64, ScoreComponents> = HashMap::new();

    // BM25: flip sign so larger == better, then normalize. A single hit
    // (or all-equal scores) gets full credit (1.0) — there's nothing to
    // rank it against, so penalising it to 0 would lose the only signal.
    if !bm25_hits.is_empty() {
        let flipped: Vec<f32> = bm25_hits.iter().map(|(_, s)| -(*s as f32)).collect();
        let normed = if bm25_hits.len() == 1 || all_equal(&flipped) {
            vec![1.0_f32; bm25_hits.len()]
        } else {
            min_max_normalize(&flipped)
        };
        for ((fact_id, _), n) in bm25_hits.iter().zip(normed.iter()) {
            out.entry(*fact_id).or_default().bm25 = *n;
        }
    }

    // Dense: normalize distances, then invert (closer == higher sem). Same
    // single-hit handling as above — the only ANN match should not be
    // zeroed out.
    if !dense_hits.is_empty() {
        let dists: Vec<f32> = dense_hits.iter().map(|(_, d)| *d as f32).collect();
        let sims: Vec<f32> = if dense_hits.len() == 1 || all_equal(&dists) {
            vec![1.0_f32; dense_hits.len()]
        } else {
            // Invert *after* normalizing so the closest distance maps to 1.0.
            min_max_normalize(&dists)
                .into_iter()
                .map(|n| 1.0 - n)
                .collect()
        };
        for ((fact_id, _), s) in dense_hits.iter().zip(sims.iter()) {
            out.entry(*fact_id).or_default().sem = *s;
        }
    }

    // Entity boost: lift in directly, clamped to [0, 1]. The caller is
    // responsible for the 0.5-per-match step + cap, but we re-clamp here
    // as a defensive guardrail.
    for (fact_id, boost) in entity_boost_per_fact {
        let clamped = boost.clamp(0.0, 1.0);
        out.entry(*fact_id).or_default().entity_boost = clamped;
    }

    out
}
