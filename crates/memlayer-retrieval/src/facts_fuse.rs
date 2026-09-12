//! Fuse BM25, dense, and fact-parent observation id lists via RRF.
//!
//! Daemon hybrid search ranks observations. Atomic facts point at a parent
//! `obs_id`; those parent ids join the fusion as a third ranked list so a
//! fact hit can surface its source observation even when BM25/dense missed it.

use crate::rrf::rrf_fuse_scored;

/// Reciprocal-rank fusion over observation-id lists. Empty lists are skipped
/// so a project with no facts (or no dense hits yet) degrades cleanly.
pub fn fuse_observation_lists(lists: &[Vec<u64>], k_const: u32) -> Vec<u64> {
    fuse_observation_lists_scored(lists, k_const)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Scored variant of [`fuse_observation_lists`] for time-decay reweighting.
pub fn fuse_observation_lists_scored(lists: &[Vec<u64>], k_const: u32) -> Vec<(u64, f64)> {
    let nonempty: Vec<Vec<u64>> = lists
        .iter()
        .filter(|l| !l.is_empty())
        .cloned()
        .collect();
    rrf_fuse_scored(&nonempty, k_const)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lists_yield_empty() {
        assert!(fuse_observation_lists(&[], 60).is_empty());
        assert!(fuse_observation_lists(&[vec![], vec![]], 60).is_empty());
    }

    #[test]
    fn skips_empty_and_preserves_single_list() {
        assert_eq!(
            fuse_observation_lists(&[vec![], vec![10, 20], vec![]], 60),
            vec![10, 20]
        );
    }

    #[test]
    fn fact_parent_in_both_lists_ranks_first() {
        let bm25 = vec![1, 2, 3];
        let dense = vec![2, 1];
        let facts = vec![3, 4];
        let fused = fuse_observation_lists(&[bm25, dense, facts], 60);
        assert!(fused.contains(&3));
        assert!(fused.contains(&4), "fact-only parent still appears");
        assert_eq!(fused.len(), 4);
        // 3 is in BM25 and facts → outranks fact-only 4.
        let i3 = fused.iter().position(|id| *id == 3).unwrap();
        let i4 = fused.iter().position(|id| *id == 4).unwrap();
        assert!(i3 < i4);
    }

    #[test]
    fn known_rrf_order_two_lists() {
        let fused = fuse_observation_lists(&[vec![1, 2], vec![1]], 60);
        assert_eq!(fused[0], 1);
        assert_eq!(fused[1], 2);
    }
}
