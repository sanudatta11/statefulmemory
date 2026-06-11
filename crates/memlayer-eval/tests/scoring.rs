// Generated with AI Coding Rules Hub
//! TS-22: additive hybrid scoring math.
//!
//! Replaces RRF fusion with the additive formula Mem0's audit (spec §3)
//! showed correlates with their reported accuracy gains:
//!
//!     combined = sem + bm25 + entity_boost   (each term in [0, 1])
//!
//! These tests pin the math down independently of the SQLite plumbing in
//! `retrieve_facts.rs` so a regression in the scorer is caught instantly.
//! Spec: retrieval-upgrade-v1 §6, TS-22, SC-13. Plan: P2 §4.5.

use std::collections::HashMap;

use memlayer_eval::scoring::{build_score_map, min_max_normalize, ScoreComponents};

#[test]
fn additive_formula_sums_three_signals() {
    let s = ScoreComponents {
        sem: 0.3,
        bm25: 0.4,
        entity_boost: 0.5,
    };
    let combined = s.combined();
    assert!(
        (combined - 1.2).abs() < 1e-6,
        "combined() must equal sem + bm25 + entity_boost (got {combined})",
    );
}

#[test]
fn min_max_normalize_handles_constant_input() {
    // All-equal input must return zeros — never NaN from a divide-by-zero.
    // This is the guard that keeps a single-hit retriever (one ANN result,
    // one BM25 result) from poisoning downstream fusion.
    let out = min_max_normalize(&[0.5, 0.5, 0.5]);
    assert_eq!(out, vec![0.0, 0.0, 0.0]);
}

#[test]
fn min_max_normalize_maps_to_zero_one() {
    // Standard min-max: min -> 0, max -> 1, midpoint -> 0.5.
    let out = min_max_normalize(&[1.0, 3.0, 2.0]);
    assert_eq!(out.len(), 3);
    assert!((out[0] - 0.0).abs() < 1e-6, "min mapped to 0 (got {})", out[0]);
    assert!((out[1] - 1.0).abs() < 1e-6, "max mapped to 1 (got {})", out[1]);
    assert!((out[2] - 0.5).abs() < 1e-6, "mid mapped to 0.5 (got {})", out[2]);
}

#[test]
fn min_max_normalize_empty_input() {
    // Empty input -> empty output, no panic.
    let out = min_max_normalize(&[]);
    assert!(out.is_empty());
}

#[test]
fn build_score_map_combines_three_sources() {
    // Fact 1: BM25 only.        Fact 2: dense only.
    // Fact 3: BM25 + dense + entity boost.   Fact 4: entity_boost only.
    //
    // Fact 3 should win because all three signals fire. This is the key
    // behavioural contract: facts with multi-signal evidence outrank
    // single-signal hits.
    let bm25_hits = vec![
        (1, -2.0_f64), // good BM25 (more negative is better in FTS5)
        (3, -3.0_f64), // best BM25
        (5, -1.0_f64), // weakest BM25 (anchor for normalization spread)
    ];
    let dense_hits = vec![
        (2, 0.4_f64),  // mediocre distance
        (3, 0.1_f64),  // best (closest)
        (6, 0.7_f64),  // worst (anchor)
    ];
    let mut entity_boost = HashMap::new();
    entity_boost.insert(3_i64, 0.5_f32);
    entity_boost.insert(4_i64, 0.5_f32);

    let map = build_score_map(&bm25_hits, &dense_hits, &entity_boost);

    // All facts should appear in the map.
    assert_eq!(map.len(), 6, "every fact mentioned in any input must surface");
    assert!(map.contains_key(&1));
    assert!(map.contains_key(&2));
    assert!(map.contains_key(&3));
    assert!(map.contains_key(&4));
    assert!(map.contains_key(&5));
    assert!(map.contains_key(&6));

    // Fact 3 must rank highest.
    let mut ranked: Vec<(i64, f32)> = map
        .iter()
        .map(|(id, sc)| (*id, sc.combined()))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    assert_eq!(ranked[0].0, 3, "fact 3 (3-signal) must rank first; got ranking {ranked:?}");

    // Fact 1 (BM25 only) and 2 (dense only) should have non-zero combined,
    // both lower than fact 3.
    let s1 = map.get(&1).unwrap();
    let s2 = map.get(&2).unwrap();
    let s3 = map.get(&3).unwrap();
    let s4 = map.get(&4).unwrap();
    assert!(s1.bm25 > 0.0, "fact 1 must have nonzero bm25");
    assert_eq!(s1.sem, 0.0, "fact 1 must have zero sem (not in dense hits)");
    assert_eq!(s1.entity_boost, 0.0);
    assert!(s2.sem > 0.0, "fact 2 must have nonzero sem");
    assert_eq!(s2.bm25, 0.0);
    assert!(s3.combined() > s1.combined());
    assert!(s3.combined() > s2.combined());
    // Fact 4: entity-only.
    assert_eq!(s4.bm25, 0.0);
    assert_eq!(s4.sem, 0.0);
    assert!((s4.entity_boost - 0.5).abs() < 1e-6);
}

#[test]
fn build_score_map_empty_inputs() {
    let map = build_score_map(&[], &[], &HashMap::new());
    assert!(map.is_empty());
}

#[test]
fn build_score_map_single_hit_each() {
    // Single hit per retriever — must not blow up, and the lone hit should
    // get full credit (1.0) since it's the only competitor in its list.
    let bm25_hits = vec![(10, -1.5_f64)];
    let dense_hits = vec![(20, 0.2_f64)];
    let mut eb = HashMap::new();
    eb.insert(30_i64, 0.5_f32);
    let map = build_score_map(&bm25_hits, &dense_hits, &eb);
    assert!(map.contains_key(&10));
    assert!(map.contains_key(&20));
    assert!(map.contains_key(&30));
    // Lone retriever hits get max signal in their lane.
    assert!((map.get(&10).unwrap().bm25 - 1.0).abs() < 1e-6);
    assert!((map.get(&20).unwrap().sem - 1.0).abs() < 1e-6);
    assert!((map.get(&30).unwrap().entity_boost - 0.5).abs() < 1e-6);
}

#[test]
fn build_score_map_clamps_entity_boost() {
    // Defensive: caller might pass boost > 1.0 if the cap fails. We must
    // clamp so combined() can't exceed 3.0.
    let mut eb = HashMap::new();
    eb.insert(7_i64, 5.0_f32);
    let map = build_score_map(&[], &[], &eb);
    assert!((map.get(&7).unwrap().entity_boost - 1.0).abs() < 1e-6);
}
