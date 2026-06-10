// Generated with AI Coding Rules Hub
//! Skeleton TS-4: rrf_fuse over two ranked lists yields the expected order.

use memlayer_eval::rrf::rrf_fuse;

#[test]
fn ts4_two_lists_top_30_matches_manual_rrf_math() {
    // List A (BM25): doc 1 best, then 2, 3, 4, 5
    // List B (dense): doc 3 best, then 2, 1, 6, 7
    //
    // Manual RRF with k=60:
    //   doc 1: 1/(60+1) + 1/(60+3)  = 0.01639 + 0.01587 = 0.03226
    //   doc 2: 1/(60+2) + 1/(60+2)  = 0.01613 + 0.01613 = 0.03226   (tie with doc 1)
    //   doc 3: 1/(60+3) + 1/(60+1)  = 0.01587 + 0.01639 = 0.03226   (tie too)
    //   doc 4: 1/(60+4)              = 0.01563
    //   doc 5: 1/(60+5)              = 0.01538
    //   doc 6: 1/(60+4)              = 0.01563
    //   doc 7: 1/(60+5)              = 0.01538
    //
    // Top three are tied; tie-break is first-seen order: 1, 2, 3.
    // Then doc 4 (first-seen-4) > doc 6 (first-seen-6) at score 0.01563 tie.
    // Then doc 5 > doc 7 at score 0.01538 tie.
    let bm25  = vec![1u64, 2, 3, 4, 5];
    let dense = vec![3u64, 2, 1, 6, 7];

    let fused = rrf_fuse(&[bm25, dense], 60);

    assert_eq!(
        fused,
        vec![1u64, 2, 3, 4, 6, 5, 7],
        "RRF fusion must rank co-occurring docs above singletons, with first-seen tie-break"
    );
}

#[test]
fn ts4_disjoint_lists_concatenate_in_rank_order() {
    let a = vec![10u64, 20];
    let b = vec![30u64, 40];
    let fused = rrf_fuse(&[a, b], 60);
    // No overlap, so ranking is purely by rank: 10 (rank-1 in A), 30 (rank-1 in B),
    // 20 (rank-2 in A), 40 (rank-2 in B). Tie-break = first-seen.
    assert_eq!(fused, vec![10u64, 30, 20, 40]);
}

#[test]
fn ts4_handles_lists_of_different_lengths() {
    let a = vec![1u64, 2, 3, 4, 5, 6, 7, 8, 9, 10]; // BM25 top-10
    let b = vec![5u64, 1];                          // dense top-2
    let fused = rrf_fuse(&[a, b], 60);
    // Doc 1 appears in both: 1/61 + 1/62 ≈ 0.0325
    // Doc 5 appears in both: 1/65 + 1/61 ≈ 0.0318
    // Then docs 2,3,4,6,7,8,9,10 each appear once.
    assert_eq!(fused[0], 1u64);
    assert_eq!(fused[1], 5u64);
    assert!(fused.contains(&10u64));
    assert_eq!(fused.len(), 10);
}
