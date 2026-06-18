// Generated with AI Coding Rules Hub
//! memlayer-extract — LLM-based fact extraction for the eval pipeline.
//!
//! This crate turns raw conversation turns into atomic, searchable facts via
//! a claude-haiku call (per spec retrieval-upgrade-v1 §3, §4). Output flows
//! through the eval-side facts.db (sqlite-vec virtual tables for semantic
//! retrieval; FTS5 for lexical) and feeds the hybrid retrieval pipeline.
//!
//! API surface:
//! * [`Fact`] — extracted (subject, predicate, object, ...) record.
//! * [`Extractor`] trait — abstract entry point.
//! * [`HaikuExtractor`] (spec-task-14) — claude-4.5-haiku impl.
//! * [`ClaudeClient`] trait + impls (this task).

pub mod cache;
pub mod claude_cli;
pub mod entities;
pub mod extractor;
pub mod prompt;

pub use cache::{CachedExtraction, ExtractionCache};
pub use entities::{EntityExtractor, HaikuEntityExtractor, HeuristicEntityExtractor};
pub use extractor::ClaudeCliExtractor;
pub use prompt::{build_extraction_prompt, parse_facts};

use serde::{Deserialize, Serialize};

/// One turn of dialogue fed to the extraction prompt. `obs_id` and
/// `session_id` come from the storage layer (`observations`/`sessions`)
/// and are stitched onto each [`Fact`] via the `evidence_turn_idx` the
/// model emits — that's how the retriever knows which raw row to expand
/// when surfacing a fact (§4.5, EH-3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub speaker: String,
    pub text: String,
    pub obs_id: i64,
    pub session_id: Option<String>,
}

/// One atomic fact extracted from a window of conversation turns.
///
/// `evidence_obs_id` points back at the raw observation row that fired this
/// fact; the retriever expands hits via the storage layer to give the judge
/// surrounding context (§4.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub temporal: Option<String>,
    /// 0.0..=1.0 (per the extraction prompt). Reserved for P3 reranker.
    pub salience: f32,
    /// `observations.id` for the turn that produced this fact.
    pub evidence_obs_id: i64,
    /// `sessions.id` the evidence turn belongs to. Used to bound `expand_evidence`.
    pub source_session: Option<String>,
}

/// Pluggable extractor. P2 ships a single Haiku-backed implementation
/// (spec-task-14); the trait keeps tests mockable and leaves room for a
/// stronger model swap later (e.g. claude-4.6-sonnet for stubborn windows).
#[async_trait::async_trait]
pub trait Extractor: Send + Sync {
    /// Extract facts from one window of consecutive turns. The window is the
    /// already-formatted prompt body (the orchestrator handles batching and
    /// caching).
    async fn extract_window(&self, prompt: &str) -> anyhow::Result<Vec<Fact>>;
}
