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
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            mode: RetrievalMode::Bm25,
            k: 10,
            evidence_window: 0,
            rerank: false,
        }
    }
}

/// Stub: filled out in P3 (spec-task-22). For now returns defaults
/// so callers compile.
pub fn default_profile(_b: BenchmarkKind) -> RetrievalConfig {
    RetrievalConfig::default()
}
