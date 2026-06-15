// Generated with AI Coding Rules Hub
//! TS-12: int8 quantization recall.
//!
//! 1000 random 384-d unit vectors, 50 sample queries. Top-10 Jaccard
//! between f32 and int8 round-trip retrieval must be ≥0.9. Confirms
//! quantization preserves nearest-neighbor structure tightly enough
//! for the BEAM workload (recall@10 expected ~95%+).
//!
//! Spec link: SC-NFR-MEM. Plan: P4 spec-task-24.

use memlayer_embed::quantize::{calibrate_scale, f32_to_int8, int8_to_f32};

/// Cosine similarity. Both vectors are assumed unit-normalized in the
/// f32 baseline; on the int8 round-trip they are not, so we
/// L2-normalize defensively.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    dot / (na * nb)
}

fn topk(scores: &[f32], k: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|a, b| scores[*b].partial_cmp(&scores[*a]).unwrap());
    idx.truncate(k);
    idx
}

fn jaccard(a: &[usize], b: &[usize]) -> f32 {
    let sa: std::collections::HashSet<usize> = a.iter().copied().collect();
    let sb: std::collections::HashSet<usize> = b.iter().copied().collect();
    let inter = sa.intersection(&sb).count() as f32;
    let union = sa.union(&sb).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Deterministic LCG so the test is reproducible across runs / platforms.
fn lcg(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let bits = ((*state >> 33) as u32) as f32 / (u32::MAX as f32);
    bits * 2.0 - 1.0
}

fn random_unit_vec(dim: usize, state: &mut u64) -> Vec<f32> {
    let raw: Vec<f32> = (0..dim).map(|_| lcg(state)).collect();
    let n = raw.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    raw.into_iter().map(|x| x / n).collect()
}

#[test]
fn ts12_int8_topk_jaccard_at_least_0_9() {
    const N: usize = 1000;
    const D: usize = 384;
    const Q: usize = 50;
    const K: usize = 10;

    let mut state: u64 = 0xC0FFEE;
    let corpus: Vec<Vec<f32>> = (0..N).map(|_| random_unit_vec(D, &mut state)).collect();
    let queries: Vec<Vec<f32>> = (0..Q).map(|_| random_unit_vec(D, &mut state)).collect();

    // Calibrate from corpus + queries (they all live in the same scale).
    let mut all_refs: Vec<&[f32]> = corpus.iter().map(|v| v.as_slice()).collect();
    all_refs.extend(queries.iter().map(|v| v.as_slice()));
    let scale = calibrate_scale(&all_refs);

    let corpus_int8: Vec<Vec<i8>> =
        corpus.iter().map(|v| f32_to_int8(v, scale)).collect();
    let corpus_recovered: Vec<Vec<f32>> = corpus_int8
        .iter()
        .map(|v| int8_to_f32(v, scale))
        .collect();

    let mut total_j = 0.0_f32;
    for q in &queries {
        let scores_f32: Vec<f32> = corpus.iter().map(|c| cosine(q, c)).collect();
        let scores_int8: Vec<f32> =
            corpus_recovered.iter().map(|c| cosine(q, c)).collect();
        let t1 = topk(&scores_f32, K);
        let t2 = topk(&scores_int8, K);
        total_j += jaccard(&t1, &t2);
    }
    let mean_j = total_j / Q as f32;
    assert!(
        mean_j >= 0.9,
        "TS-12 mean Jaccard@{K} too low: {mean_j} (need >= 0.9)"
    );
}
