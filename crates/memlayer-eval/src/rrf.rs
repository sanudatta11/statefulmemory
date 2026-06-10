// Generated with AI Coding Rules Hub
//! Reciprocal Rank Fusion (RRF).
//!
//! Pure function combining N independently-ranked lists of document ids into
//! one merged ranking. Used by `retrieve_hybrid` to fuse BM25 and dense ANN
//! candidates without retraining or learning-to-rank.
//!
//! Formula (Cormack, Clarke & Buettcher 2009):
//!
//!     score(d) = Σ over lists L of 1 / (k_const + rank_L(d))
//!
//! where `rank_L(d)` is the **1-indexed position** of `d` in list `L`. If `d`
//! does not appear in `L`, it contributes 0 to the sum from that list.
//!
//! `k_const = 60` is the industry default; smaller values amplify the
//! advantage of high-ranked items.
//!
//! Spec links: TS-4. Plan: P1 §3.5.

use std::collections::HashMap;

/// Fuse N ranked lists of `u64` document ids into one ranking.
///
/// * `lists` — each inner `Vec<u64>` is a ranking from one retriever, most
///   relevant first. Lists may have different lengths.
/// * `k_const` — the RRF smoothing constant; 60 is the canonical default.
///
/// Returns a `Vec<u64>` of distinct document ids sorted by descending RRF
/// score. Ties are broken by **first appearance order across the input
/// lists** (stable, deterministic).
pub fn rrf_fuse(lists: &[Vec<u64>], k_const: u32) -> Vec<u64> {
    if lists.is_empty() {
        return Vec::new();
    }
    let k = k_const as f64;

    // (sum of 1/(k+rank)) per id, plus first-seen tie-breaker.
    let mut scores: HashMap<u64, f64> = HashMap::new();
    let mut first_seen: HashMap<u64, usize> = HashMap::new();
    let mut seq: usize = 0;

    for list in lists {
        for (i, &id) in list.iter().enumerate() {
            let rank = (i + 1) as f64; // 1-indexed
            *scores.entry(id).or_insert(0.0) += 1.0 / (k + rank);
            first_seen.entry(id).or_insert_with(|| {
                let s = seq;
                seq += 1;
                s
            });
        }
    }

    let mut merged: Vec<(u64, f64, usize)> = scores
        .into_iter()
        .map(|(id, s)| (id, s, *first_seen.get(&id).unwrap()))
        .collect();

    // Sort: score DESC, first-seen ASC (stable tie-break).
    merged.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });

    merged.into_iter().map(|(id, _, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty() {
        let lists: Vec<Vec<u64>> = vec![];
        assert_eq!(rrf_fuse(&lists, 60), Vec::<u64>::new());
    }

    #[test]
    fn single_list_preserves_order() {
        let lists = vec![vec![10, 20, 30]];
        assert_eq!(rrf_fuse(&lists, 60), vec![10, 20, 30]);
    }

    #[test]
    fn id_in_both_lists_outranks_id_in_one() {
        // Doc 1 ranks #1 in list A and #1 in list B → score = 2/(60+1) = 0.0328
        // Doc 2 ranks #2 in list A only                → score = 1/(60+2) = 0.0161
        let lists = vec![vec![1, 2], vec![1]];
        let merged = rrf_fuse(&lists, 60);
        assert_eq!(merged[0], 1);
        assert_eq!(merged[1], 2);
    }

    #[test]
    fn k_const_smoothing_is_applied() {
        // With k=0, rank-1 → 1/1 = 1.0; rank-2 → 1/2 = 0.5; ratio 2x.
        // With k=60, rank-1 → 1/61 ≈ 0.0164; rank-2 → 1/62 ≈ 0.0161; ratio ≈ 1.02.
        let lists = vec![vec![1, 2]];
        let merged_low_k = rrf_fuse(&lists, 0);
        let merged_high_k = rrf_fuse(&lists, 60);
        assert_eq!(merged_low_k, vec![1, 2]);
        assert_eq!(merged_high_k, vec![1, 2]);
    }
}
