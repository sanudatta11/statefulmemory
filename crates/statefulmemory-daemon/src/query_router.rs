//! Adaptive query complexity router (Wave 3 / Adaptive-RAG-inspired).
//!
//! Heuristic only — no trained classifier. Routes Easy queries off the dense
//! + CE path to keep p99 under 300 ms.

use crate::query_expand;

/// Retrieval effort tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryTier {
    /// ≤3 tokens, no temporal / multi-entity / multi-hop cues → BM25 only.
    Easy,
    /// Default hybrid + local CE.
    Normal,
    /// Temporal, multi-entity, or multi-hop phrasing → hybrid + expand + PPR.
    Hard,
}

impl QueryTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Hard => "hard",
        }
    }

    pub fn use_dense(self) -> bool {
        !matches!(self, Self::Easy)
    }

    pub fn use_local_rerank(self) -> bool {
        !matches!(self, Self::Easy)
    }

    pub fn use_graph(self) -> bool {
        matches!(self, Self::Hard)
    }

    pub fn use_fact_expand(self) -> bool {
        !matches!(self, Self::Easy)
    }
}

/// Classify `query`. When `router_enabled` is false, always returns [`Normal`].
pub fn classify(query: &str, router_enabled: bool) -> QueryTier {
    if !router_enabled {
        return QueryTier::Normal;
    }
    let q = query.trim();
    if q.is_empty() {
        return QueryTier::Easy;
    }
    let lower = q.to_ascii_lowercase();
    let tokens: Vec<&str> = q.split_whitespace().filter(|t| !t.is_empty()).collect();

    let temporal = query_expand::infer_time_range(q, chrono::Utc::now()).is_some();
    let multi_hop = [
        "how does",
        "how do",
        "relate",
        "related to",
        "which ",
        " after ",
        " before ",
        "between ",
        "multi",
        " vs ",
        "versus",
    ]
    .iter()
    .any(|k| lower.contains(k));

    let entity_cues = statefulmemory_extract::entity_resolve::query_entities(q);
    let multi_entity = entity_cues.len() >= 2;
    let backtick_paths = q.matches('`').count() >= 2 || lower.contains("::");

    if temporal || multi_hop || multi_entity || backtick_paths {
        return QueryTier::Hard;
    }
    if tokens.len() <= 3 && entity_cues.len() <= 1 {
        return QueryTier::Easy;
    }
    QueryTier::Normal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_query_is_easy() {
        assert_eq!(classify("pgx auth", true), QueryTier::Easy);
    }

    #[test]
    fn temporal_is_hard() {
        assert_eq!(
            classify("what did we decide yesterday?", true),
            QueryTier::Hard
        );
    }

    #[test]
    fn router_off_is_normal() {
        assert_eq!(classify("pgx", false), QueryTier::Normal);
    }

    #[test]
    fn relate_phrase_is_hard() {
        assert_eq!(
            classify("how does validate relate to refresh", true),
            QueryTier::Hard
        );
    }
}
