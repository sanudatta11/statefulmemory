// Generated with AI Coding Rules Hub
//! 16-shard router for BEAM-scale workloads (P4 spec-task-25).
//!
//! At BEAM-10M scale a single facts.db saturates SQLite's page cache and
//! the vec0 ANN scan becomes the dominant latency. Sharding the corpus
//! across N independent SQLite files lets us fan retrieval out across
//! `tokio::spawn_blocking` workers and merge the per-shard top-K back
//! into a global top-K via a heap pass.
//!
//! This module is storage-agnostic — it owns the routing key + the
//! merge math. The actual per-shard SQL is wired in spec-task-26 for
//! the BEAM extract/retrieve path. LoCoMo / LongMemEval continue to
//! use a single facts.db (shards=1).
//!
//! Spec link: SC-NFR-LATENCY. Plan: P4 spec-task-25.

use std::path::{Path, PathBuf};

/// Default shard count for BEAM. Locked-grill choice — 16 keeps each
/// shard ~625k vectors at BEAM-10M, well within SQLite's comfort zone
/// while saturating typical 8-core dev boxes.
pub const DEFAULT_SHARD_COUNT: usize = 16;

/// Maps observation ids to a deterministic shard index in `0..shard_count`.
#[derive(Debug, Clone, Copy)]
pub struct ShardRouter {
    pub shard_count: usize,
}

impl ShardRouter {
    pub fn new(shard_count: usize) -> Self {
        Self {
            shard_count: shard_count.max(1),
        }
    }

    /// FNV-1a 64-bit hash mod shard_count. Deterministic across runs and
    /// platforms — important so re-extracts route the same obs to the
    /// same shard. Negative ids fold via twos-complement bytes; the
    /// distribution is uniform either way.
    pub fn shard_for(&self, obs_id: i64) -> usize {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in obs_id.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        (h % self.shard_count as u64) as usize
    }

    /// Conventional on-disk path: `<base_dir>/shard-NN.db` with `NN`
    /// zero-padded to two digits. Caller decides the parent directory
    /// (BEAM uses `<data_dir>/beam-vec/`).
    pub fn shard_path(&self, base_dir: &Path, shard_idx: usize) -> PathBuf {
        base_dir.join(format!("shard-{shard_idx:02}.db"))
    }

    /// Suggested per-shard k. Over-fetch 1.5× so the merge has enough
    /// candidates to cover the top-K when relevant items cluster in a
    /// single shard. `ceil((k * 1.5) / shard_count)`, never below 1.
    pub fn k_per_shard(&self, k: usize) -> usize {
        let raw = ((k as f32) * 1.5 / self.shard_count as f32).ceil() as usize;
        raw.max(1)
    }
}

/// Merge per-shard top-K results into a global top-K by ascending
/// distance (closest first). Stable secondary sort by id keeps output
/// deterministic on distance ties — important for TS-13's reference
/// equality check vs single-DB output.
pub fn merge_shard_results(
    shards: Vec<Vec<(i64, f32)>>,
    k: usize,
) -> Vec<(i64, f32)> {
    let mut all: Vec<(i64, f32)> = shards.into_iter().flatten().collect();
    all.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    all.truncate(k);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_for_is_deterministic_and_in_range() {
        let r = ShardRouter::new(16);
        for id in 0..1000 {
            let s1 = r.shard_for(id);
            let s2 = r.shard_for(id);
            assert_eq!(s1, s2, "deterministic per id");
            assert!(s1 < 16, "in range");
        }
    }

    #[test]
    fn shard_for_distributes_uniformly() {
        let r = ShardRouter::new(16);
        let mut counts = vec![0_usize; 16];
        for id in 0..16_000 {
            counts[r.shard_for(id)] += 1;
        }
        // Expected ~1000 per shard. Allow ±25% slack for hash variance.
        for c in counts {
            assert!(c > 750 && c < 1250, "uneven shard load: {c}");
        }
    }

    #[test]
    fn k_per_shard_over_fetches_1_5x() {
        let r = ShardRouter::new(16);
        // k=10 → ceil(15/16) = 1.
        assert_eq!(r.k_per_shard(10), 1);
        // k=100 → ceil(150/16) = 10.
        assert_eq!(r.k_per_shard(100), 10);
        // k=160 → ceil(240/16) = 15.
        assert_eq!(r.k_per_shard(160), 15);
    }

    #[test]
    fn k_per_shard_minimum_1() {
        let r = ShardRouter::new(100);
        assert_eq!(r.k_per_shard(1), 1);
    }

    #[test]
    fn merge_returns_global_topk_by_distance() {
        let shards = vec![
            vec![(1, 0.5), (2, 0.7)],
            vec![(3, 0.3), (4, 0.9)],
            vec![(5, 0.4)],
        ];
        let merged = merge_shard_results(shards, 3);
        assert_eq!(merged, vec![(3, 0.3), (5, 0.4), (1, 0.5)]);
    }

    #[test]
    fn merge_breaks_ties_by_id() {
        let shards = vec![
            vec![(7, 0.5)],
            vec![(2, 0.5)],
            vec![(9, 0.5)],
        ];
        let merged = merge_shard_results(shards, 3);
        assert_eq!(merged.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![2, 7, 9]);
    }

    #[test]
    fn merge_caps_at_k() {
        let shards = vec![vec![(1, 0.1), (2, 0.2), (3, 0.3), (4, 0.4)]];
        let merged = merge_shard_results(shards, 2);
        assert_eq!(merged, vec![(1, 0.1), (2, 0.2)]);
    }

    #[test]
    fn shard_path_zero_pads() {
        let r = ShardRouter::new(16);
        let base = Path::new("/tmp/beam-vec");
        assert_eq!(
            r.shard_path(base, 3),
            PathBuf::from("/tmp/beam-vec/shard-03.db")
        );
        assert_eq!(
            r.shard_path(base, 15),
            PathBuf::from("/tmp/beam-vec/shard-15.db")
        );
    }
}
