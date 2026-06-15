// Generated with AI Coding Rules Hub
//! TS-13: 16-shard merged top-10 == single-DB top-10 with rowid tiebreak.
//!
//! 100k synthetic vectors, distributed across 16 shards via the
//! deterministic FNV router. The cross-shard merge of per-shard top-K
//! must equal the global top-K computed in a single pass over all
//! 100k. This is the correctness property that lets BEAM-1M / 10M
//! shard without changing retrieval semantics.
//!
//! Spec link: SC-NFR-LATENCY. Plan: P4 spec-task-25.

use memlayer_eval::sharding::{merge_shard_results, ShardRouter};

const N: usize = 100_000;
const K: usize = 10;
const SHARDS: usize = 16;

/// Deterministic LCG so the test reproduces.
fn lcg(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let bits = ((*state >> 33) as u32) as f32 / (u32::MAX as f32);
    bits * 2.0 - 1.0
}

#[test]
fn ts13_sharded_topk_equals_single_db_topk() {
    let mut state: u64 = 0x5EED_BEEF;

    // Generate 100k (id, distance) pairs.
    let all: Vec<(i64, f32)> = (0..N)
        .map(|i| (i as i64, lcg(&mut state).abs())) // distance >= 0
        .collect();

    // Single-DB top-K: sort all and take top-K (smallest distance wins).
    let mut single = all.clone();
    single.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    single.truncate(K);

    // Sharded: distribute by router, take per-shard top-(k_per_shard) with
    // 1.5× over-fetch, then merge.
    let router = ShardRouter::new(SHARDS);
    let k_per = router.k_per_shard(K);
    let mut shards: Vec<Vec<(i64, f32)>> = vec![Vec::new(); SHARDS];
    for (id, dist) in &all {
        let s = router.shard_for(*id);
        shards[s].push((*id, *dist));
    }
    // Per-shard top k_per by distance.
    let shard_topk: Vec<Vec<(i64, f32)>> = shards
        .into_iter()
        .map(|mut s| {
            s.sort_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(&b.0))
            });
            s.truncate(k_per);
            s
        })
        .collect();

    let merged = merge_shard_results(shard_topk, K);

    assert_eq!(
        merged.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        single.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        "merged top-{K} ids must equal single-DB top-{K}"
    );
}
