//! Graph-based observation ranking.
//!
//! Pure scoring layer over graph traversal hits: each hit pairs an
//! observation with an entity it touches, at some hop distance from the
//! query entity, via an edge of a known [`statefulmemory_core::EdgeRelation`].
//!
//! Score per hit:
//!
//! ```text
//! contribution = edge_weight × HOP_DECAY_BASE^hop × kind_boost(relation)
//! ```
//!
//! Contributions aggregate per observation id, then scale by `boost`.
//! Output is sorted by descending score, ties broken by observation id
//! ascending (stable, deterministic), capped at [`GRAPH_RANK_MAX`] entries.

use std::collections::HashMap;

use statefulmemory_core::config::EdgeRelation;

/// Maximum entries returned by [`graph_score`].
pub const GRAPH_RANK_MAX: usize = 64;

/// Score multiplier base per hop: `HOP_DECAY_BASE.powi(hop)` —
/// hop 0 → 1.0, hop 1 → 0.5, hop 2 → 0.25.
pub const HOP_DECAY_BASE: f64 = 0.5;

/// One graph-traversal hit: observation `observation_id` touches entity
/// `entity_id` at `hop` edges from the query entity, via an edge of weight
/// `edge_weight` and relation `relation`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphHit {
    pub observation_id: u64,
    pub entity_id: i64,
    pub hop: u8,
    pub edge_weight: f64,
    pub relation: EdgeRelation,
}

/// Relation-kind boost applied to each hit's contribution.
///
/// 1.0 for `Mentions` / `About`, 1.5 for `Fixes`, 1.2 for `Contradicts`,
/// 0.5 for `CoOccurs`.
fn kind_boost(relation: EdgeRelation) -> f64 {
    match relation {
        EdgeRelation::Mentions | EdgeRelation::About => 1.0,
        EdgeRelation::Fixes => 1.5,
        EdgeRelation::Contradicts => 1.2,
        EdgeRelation::CoOccurs => 0.5,
    }
}

/// Aggregate graph hits into per-observation scores.
///
/// Returns `(observation_id, score)` pairs sorted by descending score with
/// ties broken by ascending observation id, capped at [`GRAPH_RANK_MAX`].
pub fn graph_score(hits: &[GraphHit], boost: f64) -> Vec<(u64, f64)> {
    if hits.is_empty() {
        return Vec::new();
    }

    let mut scores: HashMap<u64, f64> = HashMap::new();
    for hit in hits {
        let decay = HOP_DECAY_BASE.powi(hit.hop as i32);
        *scores.entry(hit.observation_id).or_insert(0.0) +=
            hit.edge_weight * decay * kind_boost(hit.relation);
    }

    let mut ranked: Vec<(u64, f64)> = scores.into_iter().map(|(id, s)| (id, s * boost)).collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    ranked.truncate(GRAPH_RANK_MAX);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(
        observation_id: u64,
        entity_id: i64,
        hop: u8,
        edge_weight: f64,
        relation: EdgeRelation,
    ) -> GraphHit {
        GraphHit {
            observation_id,
            entity_id,
            hop,
            edge_weight,
            relation,
        }
    }

    #[test]
    fn hop_decay_values() {
        assert_eq!(HOP_DECAY_BASE.powi(0), 1.0);
        assert_eq!(HOP_DECAY_BASE.powi(1), 0.5);
        assert_eq!(HOP_DECAY_BASE.powi(2), 0.25);
        assert_eq!(HOP_DECAY_BASE.powi(3), 0.125);
    }

    #[test]
    fn kind_boost_matches_spec() {
        assert_eq!(kind_boost(EdgeRelation::Mentions), 1.0);
        assert_eq!(kind_boost(EdgeRelation::About), 1.0);
        assert_eq!(kind_boost(EdgeRelation::Fixes), 1.5);
        assert_eq!(kind_boost(EdgeRelation::Contradicts), 1.2);
        assert_eq!(kind_boost(EdgeRelation::CoOccurs), 0.5);
    }

    #[test]
    fn two_entities_meet_in_one_observation() {
        // Entity 1 (hop 0, Fixes) and entity 2 (hop 1, Mentions) both anchor
        // observation 100: 2.0 × 1.0 × 1.5 + 1.0 × 0.5 × 1.0 = 3.5.
        let hits = vec![
            hit(100, 1, 0, 2.0, EdgeRelation::Fixes),
            hit(100, 2, 1, 1.0, EdgeRelation::Mentions),
        ];
        let scored = graph_score(&hits, 1.0);
        assert_eq!(scored, vec![(100, 3.5)]);
    }

    #[test]
    fn boost_multiplier_applies_to_sum() {
        let hits = vec![hit(1, 10, 0, 2.0, EdgeRelation::Contradicts)];
        // 2.0 × 1.0 × 1.2 × 3.0 = 7.2
        let scored = graph_score(&hits, 3.0);
        assert!((scored[0].1 - 7.2).abs() < 1e-9);
        assert_eq!(scored[0].0, 1);
    }

    #[test]
    fn cap_at_max_entries() {
        let hits: Vec<GraphHit> = (0..70u64)
            .map(|i| hit(i, (i as i64) + 1000, 0, 1.0, EdgeRelation::Mentions))
            .collect();
        let scored = graph_score(&hits, 1.0);
        assert_eq!(scored.len(), GRAPH_RANK_MAX);
    }

    #[test]
    fn deterministic_ordering() {
        let hits = vec![
            hit(7, 1, 1, 1.0, EdgeRelation::Mentions),
            hit(3, 2, 0, 1.0, EdgeRelation::Mentions),
            hit(3, 1, 0, 0.5, EdgeRelation::CoOccurs),
            hit(9, 3, 2, 4.0, EdgeRelation::About),
        ];
        let a = graph_score(&hits, 1.0);
        let b = graph_score(&hits, 1.0);
        assert_eq!(a, b, "graph_score must be deterministic");
        // obs 3: 1.0 + 0.25 = 1.25; obs 9: 4.0 × 0.25 = 1.0; obs 7: 0.5.
        assert_eq!(a, vec![(3, 1.25), (9, 1.0), (7, 0.5)]);
    }

    #[test]
    fn tie_breaks_by_observation_id_ascending() {
        let hits = vec![
            hit(42, 1, 0, 1.0, EdgeRelation::Mentions),
            hit(7, 1, 0, 1.0, EdgeRelation::Mentions),
            hit(20, 2, 0, 1.0, EdgeRelation::Mentions),
        ];
        let scored = graph_score(&hits, 1.0);
        let ids: Vec<u64> = scored.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![7, 20, 42]);
    }

    #[test]
    fn empty_slice_returns_empty_vec() {
        let hits: Vec<GraphHit> = Vec::new();
        assert_eq!(graph_score(&hits, 1.0), Vec::<(u64, f64)>::new());
    }
}
