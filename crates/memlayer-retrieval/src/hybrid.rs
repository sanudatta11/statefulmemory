//! Shared types for hybrid retrieval (BM25 + dense ANN, RRF-fused).
//!
//! The actual hybrid orchestration lives at the call site (the daemon has a
//! per-project version, the eval harness uses sidecar vec DBs). This module
//! holds just the small types that both call sites use so signatures match.

use serde::{Deserialize, Serialize};

/// Retrieval mode requested by the caller. The CLI flag `--mode` and the
/// proto `mode` field deserialize to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HybridMode {
    /// Lexical BM25 only. The default in v1.x for back-compat.
    Bm25,
    /// BM25 top-30 + dense top-30, fused via RRF.
    Hybrid,
}

impl Default for HybridMode {
    fn default() -> Self {
        HybridMode::Bm25
    }
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
    fn as_wire_round_trips() {
        for mode in [HybridMode::Bm25, HybridMode::Hybrid] {
            assert_eq!(HybridMode::parse_wire(Some(mode.as_wire())), mode);
        }
    }
}
