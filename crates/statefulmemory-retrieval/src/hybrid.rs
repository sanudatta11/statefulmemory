//! Shared types for hybrid retrieval (BM25 + dense ANN, RRF-fused).
//!
//! The actual hybrid orchestration lives at the call site (the daemon has a
//! per-project version, the eval harness uses sidecar vec DBs). This module
//! holds just the small types that both call sites use so signatures match.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

pub const DEFAULT_CANDIDATE_DEPTH: i32 = 30;
pub const DEFAULT_RRF_K: u32 = 60;

/// Retrieval mode requested by the caller. The CLI flag `--mode` and the
/// proto `mode` field deserialize to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HybridMode {
    /// Lexical BM25 only. The default in v1.x for back-compat.
    #[default]
    Bm25,
    /// BM25 top-30 + dense top-30, fused via RRF.
    Hybrid,
}

impl HybridMode {
    /// Parse the wire string. `None` and the empty string both yield
    /// [`HybridMode::Bm25`] (back-compat for clients that pre-date the field).
    pub fn parse_wire(s: Option<&str>) -> Self {
        match s.map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) if v.eq_ignore_ascii_case("hybrid") => HybridMode::Hybrid,
            _ => HybridMode::Bm25,
        }
    }

    /// Like [`parse_wire`], but empty/absent uses `default` (typically config `search.mode`).
    pub fn parse_wire_or_default(s: Option<&str>, default: &str) -> Self {
        match s.map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) => Self::parse_wire(Some(v)),
            None => Self::parse_wire(Some(default)),
        }
    }

    pub fn as_wire(&self) -> &'static str {
        match self {
            HybridMode::Bm25 => "bm25",
            HybridMode::Hybrid => "hybrid",
        }
    }
}

/// One hit from a hybrid retrieval round, used as a small interchange type
/// when the call sites need to share fused candidates before rendering. The
/// daemon and eval each have their own richer "Observation" type that they
/// hydrate from their own DBs; this is just the rank-list representation.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridCandidate {
    pub id: u64,
    pub bm25_rank: Option<usize>,
    pub dense_rank: Option<usize>,
    /// Fused RRF score (sum of 1/(k+rank) across lists where this id appeared).
    pub rrf_score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateTrace {
    pub candidates: Vec<HybridCandidate>,
    pub fact_parent_ids: Vec<u64>,
}

impl CandidateTrace {
    pub fn ids(&self) -> Vec<u64> {
        self.candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect()
    }
}

pub fn fuse_candidate_trace(
    bm25: &[u64],
    dense: &[u64],
    fact_parent_ids: &[u64],
    k_const: u32,
) -> CandidateTrace {
    let fact_parents = dedup_preserving_order(fact_parent_ids);
    let lists = vec![bm25.to_vec(), dense.to_vec(), fact_parents.clone()];
    let scored = crate::rrf::rrf_fuse_scored(&lists, k_const);
    let bm25_ranks = rank_map(bm25);
    let dense_ranks = rank_map(dense);

    CandidateTrace {
        candidates: scored
            .into_iter()
            .map(|(id, rrf_score)| HybridCandidate {
                id,
                bm25_rank: bm25_ranks.get(&id).copied(),
                dense_rank: dense_ranks.get(&id).copied(),
                rrf_score,
            })
            .collect(),
        fact_parent_ids: fact_parents,
    }
}

fn dedup_preserving_order(ids: &[u64]) -> Vec<u64> {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

fn rank_map(ids: &[u64]) -> HashMap<u64, usize> {
    let mut ranks = HashMap::with_capacity(ids.len());
    for (index, id) in ids.iter().copied().enumerate() {
        ranks.entry(id).or_insert(index + 1);
    }
    ranks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wire_handles_default_and_explicit() {
        assert_eq!(HybridMode::parse_wire(None), HybridMode::Bm25);
        assert_eq!(HybridMode::parse_wire(Some("")), HybridMode::Bm25);
        assert_eq!(HybridMode::parse_wire(Some("bm25")), HybridMode::Bm25);
        assert_eq!(HybridMode::parse_wire(Some("hybrid")), HybridMode::Hybrid);
        assert_eq!(HybridMode::parse_wire(Some("HYBRID")), HybridMode::Hybrid);
        assert_eq!(HybridMode::parse_wire(Some("nope")), HybridMode::Bm25);
    }

    #[test]
    fn parse_wire_or_default_uses_config_when_absent() {
        assert_eq!(
            HybridMode::parse_wire_or_default(None, "hybrid"),
            HybridMode::Hybrid
        );
        assert_eq!(
            HybridMode::parse_wire_or_default(Some(""), "hybrid"),
            HybridMode::Hybrid
        );
        assert_eq!(
            HybridMode::parse_wire_or_default(Some("bm25"), "hybrid"),
            HybridMode::Bm25
        );
    }

    #[test]
    fn as_wire_round_trips() {
        for mode in [HybridMode::Bm25, HybridMode::Hybrid] {
            assert_eq!(HybridMode::parse_wire(Some(mode.as_wire())), mode);
        }
    }

    #[test]
    fn candidate_trace_preserves_lane_ranks_and_fact_parents() {
        let trace = fuse_candidate_trace(&[101, 102, 103], &[103, 101, 104], &[105, 102, 102], 60);
        assert_eq!(trace.fact_parent_ids, vec![105, 102]);
        assert_eq!(trace.ids(), vec![101, 103, 102, 105, 104]);

        let first = &trace.candidates[0];
        assert_eq!(first.bm25_rank, Some(1));
        assert_eq!(first.dense_rank, Some(2));
        let fact_only = trace
            .candidates
            .iter()
            .find(|candidate| candidate.id == 105)
            .unwrap();
        assert_eq!(fact_only.bm25_rank, None);
        assert_eq!(fact_only.dense_rank, None);
    }

    #[test]
    fn candidate_trace_empty_dense_lane_preserves_bm25_order() {
        let trace = fuse_candidate_trace(&[8, 3, 1], &[], &[], DEFAULT_RRF_K);
        assert_eq!(trace.ids(), vec![8, 3, 1]);
        assert!(trace
            .candidates
            .iter()
            .all(|candidate| candidate.dense_rank.is_none()));
    }
}
