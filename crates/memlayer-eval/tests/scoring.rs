// Generated with AI Coding Rules Hub
//! TS-22: additive hybrid scoring math.
//!
//! Pre-P5 the combined score was just `sem + bm25 + entity_boost`. After
//! P5 (spec-task-27/28/29/30) it picks up multiplicative quality factors;
//! these tests stay focused on the BM25 + dense + entity boost math by
//! leaving the quality modifiers at their default (1.0) values, which
//! makes `combined()` reduce back to `sem + bm25 + entity_boost`.
//!
//! Spec: retrieval-upgrade-v1 §6, TS-22, SC-13.

use std::collections::HashMap;

use memlayer_eval::scoring::{build_score_map, min_max_normalize, Bm25Norm, ScoreComponents};

#[test]
fn additive_formula_sums_three_signals() {
    let s = ScoreComponents {
        sem: 0.3,
        bm25: 0.4,
        entity_boost: 0.5,
        ..Default::default()
    };
    let combined = s.combined();
    assert!(
        (combined - 1.2).abs() < 1e-6,
        "combined() must equal sem + bm25 + entity_boost (got {combined})",
    );
}

#[test]
fn min_max_normalize_handles_constant_input() {
    let out = min_max_normalize(&[0.5, 0.5, 0.5]);
    assert_eq!(out, vec![0.0, 0.0, 0.0]);
}

#[test]
fn min_max_normalize_maps_to_zero_one() {
    let out = min_max_normalize(&[1.0, 3.0, 2.0]);
    assert_eq!(out.len(), 3);
    assert!((out[0] - 0.0).abs() < 1e-6);
    assert!((out[1] - 1.0).abs() < 1e-6);
    assert!((out[2] - 0.5).abs() < 1e-6);
}

#[test]
fn min_max_normalize_empty_input() {
    let out = min_max_normalize(&[]);
    assert!(out.is_empty());
}

#[test]
fn build_score_map_combines_three_sources() {
    let bm25_hits = vec![
        (1, -2.0_f64),
        (3, -3.0_f64),
        (5, -1.0_f64),
    ];
    let dense_hits = vec![
        (2, 0.4_f64),
        (3, 0.1_f64),
        (6, 0.7_f64),
    ];
    let mut entity_boost = HashMap::new();
    entity_boost.insert(3_i64, 0.5_f32);
    entity_boost.insert(4_i64, 0.5_f32);

    let map = build_score_map(&bm25_hits, &dense_hits, &entity_boost, Bm25Norm::MinMax);

    assert_eq!(map.len(), 6);
    assert!(map.contains_key(&1));
    assert!(map.contains_key(&3));

    let mut ranked: Vec<(i64, f32)> = map
        .iter()
        .map(|(id, sc)| (*id, sc.combined()))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    assert_eq!(ranked[0].0, 3, "3-signal fact must rank first; got {ranked:?}");

    let s1 = map.get(&1).unwrap();
    let s2 = map.get(&2).unwrap();
    let s3 = map.get(&3).unwrap();
    let s4 = map.get(&4).unwrap();
    assert!(s1.bm25 > 0.0);
    assert_eq!(s1.sem, 0.0);
    assert_eq!(s1.entity_boost, 0.0);
    assert!(s2.sem > 0.0);
    assert_eq!(s2.bm25, 0.0);
    assert!(s3.combined() > s1.combined());
    assert!(s3.combined() > s2.combined());
    assert_eq!(s4.bm25, 0.0);
    assert_eq!(s4.sem, 0.0);
    assert!((s4.entity_boost - 0.5).abs() < 1e-6);
}

#[test]
fn build_score_map_empty_inputs() {
    let map = build_score_map(&[], &[], &HashMap::new(), Bm25Norm::MinMax);
    assert!(map.is_empty());
}

#[test]
fn build_score_map_single_hit_each() {
    let bm25_hits = vec![(10, -1.5_f64)];
    let dense_hits = vec![(20, 0.2_f64)];
    let mut eb = HashMap::new();
    eb.insert(30_i64, 0.5_f32);
    let map = build_score_map(&bm25_hits, &dense_hits, &eb, Bm25Norm::MinMax);
    assert!((map.get(&10).unwrap().bm25 - 1.0).abs() < 1e-6);
    assert!((map.get(&20).unwrap().sem - 1.0).abs() < 1e-6);
    assert!((map.get(&30).unwrap().entity_boost - 0.5).abs() < 1e-6);
}

#[test]
fn build_score_map_clamps_entity_boost() {
    let mut eb = HashMap::new();
    eb.insert(7_i64, 5.0_f32);
    let map = build_score_map(&[], &[], &eb, Bm25Norm::MinMax);
    assert!((map.get(&7).unwrap().entity_boost - 1.0).abs() < 1e-6);
}
