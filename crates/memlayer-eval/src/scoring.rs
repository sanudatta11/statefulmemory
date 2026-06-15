// Generated with AI Coding Rules Hub
//! Additive hybrid scoring with P5 quality modifiers.
//!
//! The core additive formula remains:
//!
//!     base(f) = sem(f) + bm25(f) + entity_boost(f)
//!
//! P5 layers four modifiers on top (locked grill 2026-06-12):
//!
//!     combined(f) = base(f)
//!                 * max(salience(f), 0.3)              // P5 spec-task-27
//!                 * exp(-0.005 * age_days(f))          // P5 spec-task-29
//!                 * contradiction_factor(f)            // P5 spec-task-30
//!                 + tiebreak_bonus(f)                  // P5 spec-task-30
//!
//! BM25 normalization itself is upgraded from min-max to an adaptive
//! sigmoid (P5 spec-task-28) tuned by query token count — Mem0's
//! published values that proved out across LoCoMo / LongMemEval.
//!
//! Spec: retrieval-upgrade-v1 §6, TS-15/16/17/18, SC-13. Plan: P5.

use std::collections::HashMap;

/// Default salience floor: even a 0.0-salience fact retains 30% of its
/// retrieval signal so we don't kill noisy-but-correct facts entirely.
pub const DEFAULT_SALIENCE_FLOOR: f32 = 0.3;

/// Default time-decay lambda. Half-life ≈ ln(2)/0.005 ≈ 138 days.
pub const DEFAULT_DECAY_LAMBDA: f64 = 0.005;

/// When two facts share (subject, predicate) but differ on object, both
/// score × this factor. Penalizes contradiction symmetrically.
pub const DEFAULT_CONTRADICTION_PENALTY: f32 = 0.5;

/// Additive bonus applied to the highest-salience fact within a
/// contradicted (subject, predicate) group. Breaks the symmetric penalty
/// in favor of the more confident extraction.
pub const DEFAULT_CONTRADICTION_TIEBREAK: f32 = 0.25;

/// Per-fact additive score components.
///
/// `sem`, `bm25`, `entity_boost` form the additive base. The remaining
/// fields are quality modifiers populated by [`apply_quality_modifiers`].
/// `combined()` rolls them all together.
#[derive(Debug, Clone)]
pub struct ScoreComponents {
    pub sem: f32,
    pub bm25: f32,
    pub entity_boost: f32,

    /// `max(raw_salience, salience_floor)`. Defaults to 1.0 so the
    /// pre-P5 additive sum is preserved when no quality data is fed in.
    pub salience_factor: f32,
    /// `exp(-lambda * age_days)`. Defaults to 1.0 (no decay) when
    /// temporal is unknown or modifiers haven't been applied.
    pub decay_factor: f32,
    /// 1.0 normally; [`DEFAULT_CONTRADICTION_PENALTY`] when this fact
    /// is in a (subject, predicate) cluster with conflicting objects.
    pub contradiction_factor: f32,
    /// Additive bonus for the highest-salience fact in a contradicted
    /// cluster. 0.0 otherwise.
    pub tiebreak_bonus: f32,
    /// True iff this fact is part of a contradicted (subject, predicate)
    /// group. Surfaced for diagnostics; does not affect scoring on its
    /// own (the factor + bonus do that).
    pub conflict: bool,
}

impl Default for ScoreComponents {
    fn default() -> Self {
        Self {
            sem: 0.0,
            bm25: 0.0,
            entity_boost: 0.0,
            salience_factor: 1.0,
            decay_factor: 1.0,
            contradiction_factor: 1.0,
            tiebreak_bonus: 0.0,
            conflict: false,
        }
    }
}

impl ScoreComponents {
    /// Final ranking score for this fact. Caller sorts descending.
    pub fn combined(&self) -> f32 {
        let base = self.sem + self.bm25 + self.entity_boost;
        base * self.salience_factor * self.decay_factor * self.contradiction_factor
            + self.tiebreak_bonus
    }
}

/// Min-max normalize a slice of scores to [0, 1].
pub fn min_max_normalize(scores: &[f32]) -> Vec<f32> {
    if scores.is_empty() {
        return Vec::new();
    }
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &s in scores {
        if s < min {
            min = s;
        }
        if s > max {
            max = s;
        }
    }
    let range = max - min;
    if range <= 0.0 || !range.is_finite() {
        return vec![0.0; scores.len()];
    }
    scores.iter().map(|s| (s - min) / range).collect()
}

fn all_equal(xs: &[f32]) -> bool {
    match xs.first() {
        None => true,
        Some(&first) => xs.iter().all(|x| *x == first),
    }
}

/// BM25 normalization strategy (spec-task-28).
///
/// Min-max keeps the original P2 behavior. AdaptiveSigmoid uses query
/// token count to pick (midpoint, steepness) per Mem0's tuned table.
/// Sigmoid is bounded [0, 1] and gracefully maps the wide raw BM25 range
/// without depending on the largest hit in the batch.
#[derive(Debug, Clone, Copy)]
pub enum Bm25Norm {
    MinMax,
    AdaptiveSigmoid { query_token_count: usize },
}

impl Default for Bm25Norm {
    fn default() -> Self {
        Bm25Norm::MinMax
    }
}

/// Mem0's empirically tuned (midpoint, steepness) per query length.
/// Short queries hit fewer terms so the BM25 dynamic range is
/// compressed; tighter steepness keeps the sigmoid responsive.
fn sigmoid_params(query_token_count: usize) -> (f32, f32) {
    match query_token_count {
        0..=3 => (5.0, 0.7),
        4..=7 => (8.0, 0.6),
        8..=15 => (10.0, 0.55),
        _ => (12.0, 0.5),
    }
}

fn sigmoid(x: f32, midpoint: f32, steepness: f32) -> f32 {
    1.0 / (1.0 + (-steepness * (x - midpoint)).exp())
}

/// Build a per-fact score map combining BM25 + dense + entity boost.
///
/// The new `bm25_norm` parameter controls BM25 normalization (MinMax for
/// backward compat / tests, AdaptiveSigmoid for production). All quality
/// modifiers (salience, decay, contradiction) are layered on by the
/// separate [`apply_quality_modifiers`] call.
pub fn build_score_map(
    bm25_hits: &[(i64, f64)],
    dense_hits: &[(i64, f64)],
    entity_boost_per_fact: &HashMap<i64, f32>,
    bm25_norm: Bm25Norm,
) -> HashMap<i64, ScoreComponents> {
    let mut out: HashMap<i64, ScoreComponents> = HashMap::new();

    // BM25: flip sign so larger == better, then normalize.
    if !bm25_hits.is_empty() {
        let flipped: Vec<f32> = bm25_hits.iter().map(|(_, s)| -(*s as f32)).collect();
        let normed = match bm25_norm {
            Bm25Norm::MinMax => {
                if bm25_hits.len() == 1 || all_equal(&flipped) {
                    vec![1.0_f32; bm25_hits.len()]
                } else {
                    min_max_normalize(&flipped)
                }
            }
            Bm25Norm::AdaptiveSigmoid { query_token_count } => {
                let (midpoint, steepness) = sigmoid_params(query_token_count);
                flipped
                    .iter()
                    .map(|&x| sigmoid(x, midpoint, steepness))
                    .collect()
            }
        };
        for ((fact_id, _), n) in bm25_hits.iter().zip(normed.iter()) {
            out.entry(*fact_id).or_default().bm25 = *n;
        }
    }

    if !dense_hits.is_empty() {
        let dists: Vec<f32> = dense_hits.iter().map(|(_, d)| *d as f32).collect();
        let sims: Vec<f32> = if dense_hits.len() == 1 || all_equal(&dists) {
            vec![1.0_f32; dense_hits.len()]
        } else {
            min_max_normalize(&dists)
                .into_iter()
                .map(|n| 1.0 - n)
                .collect()
        };
        for ((fact_id, _), s) in dense_hits.iter().zip(sims.iter()) {
            out.entry(*fact_id).or_default().sem = *s;
        }
    }

    for (fact_id, boost) in entity_boost_per_fact {
        let clamped = boost.clamp(0.0, 1.0);
        out.entry(*fact_id).or_default().entity_boost = clamped;
    }

    out
}

/// Per-fact metadata used by [`apply_quality_modifiers`].
#[derive(Debug, Clone)]
pub struct FactMeta {
    pub salience: f32,
    /// Unix timestamp seconds of the fact's event date. None when the
    /// extractor didn't capture a parseable temporal field; decay is
    /// then disabled (factor=1.0).
    pub temporal_unix: Option<i64>,
    pub subject: String,
    pub predicate: String,
    pub object: String,
}

/// Knobs for the four P5 scoring modifiers.
#[derive(Debug, Clone)]
pub struct QualityConfig {
    pub salience_floor: f32,
    pub decay_lambda: f64,
    pub contradiction_penalty: f32,
    pub contradiction_tiebreak: f32,
    /// Reference time in unix seconds for age-days computation. The
    /// runner sets this to the latest temporal in the dataset (so a
    /// 2023-conversation benchmark anchors decay at 2023-12-31 rather
    /// than today's calendar date).
    pub now_unix: i64,
}

impl QualityConfig {
    pub fn with_now(now_unix: i64) -> Self {
        Self {
            salience_floor: DEFAULT_SALIENCE_FLOOR,
            decay_lambda: DEFAULT_DECAY_LAMBDA,
            contradiction_penalty: DEFAULT_CONTRADICTION_PENALTY,
            contradiction_tiebreak: DEFAULT_CONTRADICTION_TIEBREAK,
            now_unix,
        }
    }
}

/// Layer salience + time-decay + contradiction on top of the additive
/// base scores. Modifies `score_map` in place. Facts in `score_map` that
/// have no entry in `meta_by_id` keep their default modifiers (1.0 / 1.0
/// / 1.0 / 0.0 / false) — the additive base survives.
pub fn apply_quality_modifiers(
    score_map: &mut HashMap<i64, ScoreComponents>,
    meta_by_id: &HashMap<i64, FactMeta>,
    cfg: &QualityConfig,
) {
    // 1. Salience + decay per-fact.
    for (id, sc) in score_map.iter_mut() {
        if let Some(meta) = meta_by_id.get(id) {
            sc.salience_factor = meta.salience.max(cfg.salience_floor);
            sc.decay_factor = match meta.temporal_unix {
                Some(t) => {
                    let age_days = ((cfg.now_unix - t).max(0) as f64) / 86400.0;
                    (-cfg.decay_lambda * age_days).exp() as f32
                }
                None => 1.0,
            };
        }
    }

    // 2. Contradiction grouping. Cluster fact ids by (subject, predicate),
    //    case-insensitive after trim. Any cluster with >1 distinct object
    //    is a contradiction.
    let mut clusters: HashMap<(String, String), Vec<i64>> = HashMap::new();
    for (&id, _) in score_map.iter() {
        if let Some(meta) = meta_by_id.get(&id) {
            let key = (
                meta.subject.trim().to_lowercase(),
                meta.predicate.trim().to_lowercase(),
            );
            clusters.entry(key).or_default().push(id);
        }
    }
    for (_key, ids) in clusters.iter() {
        if ids.len() < 2 {
            continue;
        }
        // Distinct-object check (case-insensitive trim).
        let mut objects = std::collections::HashSet::new();
        for id in ids {
            if let Some(m) = meta_by_id.get(id) {
                objects.insert(m.object.trim().to_lowercase());
            }
        }
        if objects.len() < 2 {
            // Same fact extracted twice (canonical-key dedup is task-27b);
            // not a real contradiction.
            continue;
        }
        // Apply symmetric penalty + tiebreak bonus to the highest-salience
        // member.
        let mut top_id: Option<i64> = None;
        let mut top_sal: f32 = -1.0;
        for id in ids {
            if let (Some(sc), Some(meta)) = (score_map.get_mut(id), meta_by_id.get(id)) {
                sc.contradiction_factor = cfg.contradiction_penalty;
                sc.conflict = true;
                if meta.salience > top_sal {
                    top_sal = meta.salience;
                    top_id = Some(*id);
                }
            }
        }
        if let Some(id) = top_id {
            if let Some(sc) = score_map.get_mut(&id) {
                sc.tiebreak_bonus = cfg.contradiction_tiebreak;
            }
        }
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(salience: f32, subject: &str, predicate: &str, object: &str) -> FactMeta {
        FactMeta {
            salience,
            temporal_unix: None,
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
        }
    }

    #[test]
    fn ts15_high_salience_outranks_equal_low_salience() {
        let mut map: HashMap<i64, ScoreComponents> = HashMap::new();
        // Two facts with identical sem+bm25+entity (base = 1.0).
        for id in [1_i64, 2_i64] {
            map.insert(
                id,
                ScoreComponents {
                    bm25: 0.5,
                    sem: 0.5,
                    ..Default::default()
                },
            );
        }
        let mut metas = HashMap::new();
        metas.insert(1, meta(0.9, "x", "p", "a"));
        metas.insert(2, meta(0.1, "x", "q", "b"));
        let cfg = QualityConfig::with_now(0);
        apply_quality_modifiers(&mut map, &metas, &cfg);
        let s1 = map[&1].combined();
        let s2 = map[&2].combined();
        assert!(s1 > s2, "high-sal {s1} should beat low-sal {s2}");
    }

    #[test]
    fn ts15_floor_prevents_zero_suppression() {
        let mut map = HashMap::new();
        map.insert(
            1_i64,
            ScoreComponents {
                bm25: 1.0,
                ..Default::default()
            },
        );
        let mut metas = HashMap::new();
        metas.insert(1_i64, meta(0.0, "x", "p", "a"));
        let cfg = QualityConfig::with_now(0);
        apply_quality_modifiers(&mut map, &metas, &cfg);
        // 0.0 salience clamped to floor 0.3 → combined = 1.0 * 0.3 = 0.3.
        assert!((map[&1].combined() - 0.3).abs() < 1e-5);
    }

    #[test]
    fn ts16_adaptive_sigmoid_short_query() {
        // Query of 2 tokens → midpoint 5.0, steepness 0.7.
        // Single hit with raw flipped score 5.0 → sigmoid(0)=0.5.
        let bm25 = vec![(1_i64, -5.0_f64)];
        let dense: Vec<(i64, f64)> = vec![];
        let boost = HashMap::new();
        let map = build_score_map(
            &bm25,
            &dense,
            &boost,
            Bm25Norm::AdaptiveSigmoid { query_token_count: 2 },
        );
        let bm25_norm = map[&1].bm25;
        assert!((bm25_norm - 0.5).abs() < 1e-3, "got {bm25_norm}");
    }

    #[test]
    fn ts17_time_decay_older_fact_lower() {
        let mut map = HashMap::new();
        for id in [1_i64, 2_i64] {
            map.insert(
                id,
                ScoreComponents {
                    bm25: 1.0,
                    ..Default::default()
                },
            );
        }
        let mut metas = HashMap::new();
        // 1 = today (now_unix=10000). 2 = 200 days ago.
        let now = 200 * 86400_i64;
        metas.insert(
            1_i64,
            FactMeta {
                temporal_unix: Some(now),
                ..meta(1.0, "a", "p", "x")
            },
        );
        metas.insert(
            2_i64,
            FactMeta {
                temporal_unix: Some(0),
                ..meta(1.0, "b", "q", "y")
            },
        );
        let cfg = QualityConfig::with_now(now);
        apply_quality_modifiers(&mut map, &metas, &cfg);
        assert!(map[&1].decay_factor > map[&2].decay_factor);
        assert!((map[&1].decay_factor - 1.0).abs() < 1e-5);
        // 200 days * 0.005 = 1.0 -> exp(-1) ≈ 0.367.
        assert!((map[&2].decay_factor - (-1.0_f64).exp() as f32).abs() < 1e-3);
    }

    #[test]
    fn ts18_contradiction_penalizes_both_sides() {
        let mut map = HashMap::new();
        for id in [1_i64, 2_i64] {
            map.insert(
                id,
                ScoreComponents {
                    bm25: 1.0,
                    ..Default::default()
                },
            );
        }
        let mut metas = HashMap::new();
        metas.insert(1_i64, meta(0.9, "Caroline", "lives_in", "Toronto"));
        metas.insert(2_i64, meta(0.4, "Caroline", "lives_in", "Vancouver"));
        let cfg = QualityConfig::with_now(0);
        apply_quality_modifiers(&mut map, &metas, &cfg);
        assert!(map[&1].conflict);
        assert!(map[&2].conflict);
        assert!((map[&1].contradiction_factor - 0.5).abs() < 1e-5);
        assert!((map[&2].contradiction_factor - 0.5).abs() < 1e-5);
        // Higher-salience gets the tiebreak bonus.
        assert!((map[&1].tiebreak_bonus - 0.25).abs() < 1e-5);
        assert_eq!(map[&2].tiebreak_bonus, 0.0);
    }

    #[test]
    fn ts18_same_object_is_not_contradiction() {
        let mut map = HashMap::new();
        for id in [1_i64, 2_i64] {
            map.insert(
                id,
                ScoreComponents {
                    bm25: 1.0,
                    ..Default::default()
                },
            );
        }
        let mut metas = HashMap::new();
        metas.insert(1_i64, meta(0.9, "Mel", "likes", "running"));
        metas.insert(2_i64, meta(0.4, "Mel", "likes", "Running"));
        let cfg = QualityConfig::with_now(0);
        apply_quality_modifiers(&mut map, &metas, &cfg);
        assert!(!map[&1].conflict);
        assert!(!map[&2].conflict);
    }
}
