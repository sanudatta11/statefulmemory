// Generated with AI Coding Rules Hub
//! Retrieval configuration types for the eval harness.
//!
//! This module defines [`RetrievalMode`], [`RetrievalConfig`], and the
//! [`default_profile`] benchmark-profile lookup. The lookup is currently a
//! stub returning [`RetrievalConfig::default`]; P3 (spec-task-22) will
//! replace it with per-benchmark tuned defaults.
//!
//! Spec links: SC-7, SC-9. Plan: P0 §2.2, §2.3.

use crate::runner::BenchmarkKind;

/// Retrieval strategy selector.
///
/// - `Bm25`: lexical-only (current default).
/// - `Hybrid`: BM25 + dense vector fusion.
/// - `HybridRerank`: hybrid candidates re-scored by a cross-encoder reranker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum RetrievalMode {
    Bm25,
    Hybrid,
    HybridRerank,
}

/// Tunable retrieval parameters used by the eval harness.
///
/// `evidence_window` controls how many neighboring chunks around each hit
/// are surfaced to the judge. `rerank` is reserved for downstream wiring;
/// it is also implied when `mode == HybridRerank`.
#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    pub mode: RetrievalMode,
    pub k: i32,
    pub evidence_window: u8,
    pub rerank: bool,
    /// Time-decay lambda applied during scoring (P5 spec-task-29).
    /// `combined *= exp(-decay_lambda * age_days)`. Default 0.005 gives
    /// a half-life of ~138 days. Set to 0.0 to disable decay entirely.
    pub decay_lambda: f64,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            mode: RetrievalMode::Bm25,
            k: 10,
            evidence_window: 0,
            rerank: false,
            decay_lambda: crate::scoring::DEFAULT_DECAY_LAMBDA,
        }
    }
}

/// Per-benchmark default retrieval profile (spec-task-22, P3).
///
/// LoCoMo + LongMemEval are conversational benchmarks where reranking
/// pays off: top-30 candidates get reordered by Haiku before the answer
/// LLM sees them. Evidence-window expansion adds surrounding turns so
/// the judge has conversational context.
///
/// BEAM (1M / 10M observations) is a recall benchmark where the LLM
/// rerank cost would dominate end-to-end latency. Stay on plain hybrid
/// (BM25 + dense, no rerank) and skip evidence expansion (BEAM facts
/// are atomic — no surrounding session to pull).
pub fn default_profile(b: BenchmarkKind) -> RetrievalConfig {
    match b {
        BenchmarkKind::Locomo => RetrievalConfig {
            mode: RetrievalMode::HybridRerank,
            k: 10,
            evidence_window: 2,
            rerank: true,
            decay_lambda: crate::scoring::DEFAULT_DECAY_LAMBDA,
        },
        BenchmarkKind::Longmemeval => RetrievalConfig {
            mode: RetrievalMode::HybridRerank,
            k: 10,
            evidence_window: 4,
            rerank: true,
            decay_lambda: crate::scoring::DEFAULT_DECAY_LAMBDA,
        },
        BenchmarkKind::Beam1m | BenchmarkKind::Beam10m => RetrievalConfig {
            mode: RetrievalMode::Hybrid,
            k: 10,
            evidence_window: 0,
            rerank: false,
            decay_lambda: 0.0,
        },
        BenchmarkKind::Staleness => RetrievalConfig {
            mode: RetrievalMode::Bm25,
            k: 10,
            evidence_window: 0,
            rerank: false,
            decay_lambda: 0.0,
        },
    }
}
