//! Adaptive query complexity router (Wave 3 / Adaptive-RAG-inspired).
//!
//! Heuristic baseline + optional Laya System-1 choice (Wave 4). When Laya is
//! disabled, unreachable, times out, or low-confidence → heuristic only.
//! Never calls an agent CLI for tier selection.

use std::sync::Arc;

use crate::laya::LayaClient;
use crate::laya_schemas;
use crate::query_expand;
use statefulmemory_core::config::LayaConfig;

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

    pub fn from_str_label(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "easy" => Some(Self::Easy),
            "normal" => Some(Self::Normal),
            "hard" => Some(Self::Hard),
            _ => None,
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

/// Prefer Laya when enabled; on any failure use [`classify`] (never Claude).
pub async fn classify_maybe_laya(
    query: &str,
    router_enabled: bool,
    laya: Option<&Arc<LayaClient>>,
    laya_cfg: &LayaConfig,
) -> QueryTier {
    let heuristic = classify(query, router_enabled);
    if !router_enabled {
        return heuristic;
    }
    let Some(client) = laya else {
        return heuristic;
    };
    if !laya_cfg.enabled || !laya_cfg.router {
        return heuristic;
    }

    if let Some(cached) = client.cached_tier(query) {
        if let Some(t) = QueryTier::from_str_label(&cached) {
            return t;
        }
    }

    let client = Arc::clone(client);
    let q = query.to_string();
    let model = laya_cfg.model_router.clone();
    let state = laya_schemas::router_state(&q);
    let questions = laya_schemas::router_questions();
    let state_c = state.clone();
    let questions_c = questions.clone();

    let result = tokio::task::spawn_blocking(move || {
        client
            .predict(&state_c, &questions_c, &model)
            .map(|r| (client, r))
    })
    .await;

    match result {
        Ok(Ok((client, pred))) => {
            if let Some(label) = client.choice_value(&pred, "tier") {
                if let Some(tier) = QueryTier::from_str_label(label) {
                    let conf = pred
                        .answers
                        .get("tier")
                        .and_then(|a| a.confidence)
                        .unwrap_or(1.0);
                    client.put_tier_cache(&q, tier.as_str(), conf);
                    crate::laya::log_teacher(
                        "router",
                        &state,
                        &questions,
                        &serde_json::json!({ "tier": label, "confidence": conf, "source": "laya" }),
                    );
                    return tier;
                }
            }
            heuristic
        }
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "laya router predict failed — heuristic");
            heuristic
        }
        Err(e) => {
            tracing::debug!(error = %e, "laya router join failed — heuristic");
            heuristic
        }
    }
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

    #[test]
    fn from_str_label_roundtrip() {
        assert_eq!(QueryTier::from_str_label("EASY"), Some(QueryTier::Easy));
        assert_eq!(QueryTier::from_str_label("nope"), None);
    }
}
