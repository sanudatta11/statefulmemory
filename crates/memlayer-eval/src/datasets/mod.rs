// Generated with AI Coding Rules Hub
//! Dataset adapters: one module per benchmark.

pub mod beam;
pub mod locomo;
pub mod longmemeval;
pub mod staleness;

use serde::{Deserialize, Serialize};

/// A single query from any benchmark dataset, normalised to a common shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalQuery {
    /// Unique identifier within the benchmark (e.g. LoCoMo QA id).
    pub id: String,
    /// Natural-language question to retrieve memories for.
    pub question: String,
    /// The gold / expected answer used by the LLM judge.
    pub gold_answer: String,
    /// Optional extra context passed verbatim to the judge (e.g. conversation
    /// excerpt from LoCoMo).
    pub judge_context: Option<String>,
    /// Benchmark-provided category label (e.g. LoCoMo single-hop / multi-hop /
    /// temporal / open-domain / adversarial). Used for per-category reporting.
    #[serde(default)]
    pub category: Option<String>,
    /// Value that must NOT be served (superseded). Staleness benchmark only.
    #[serde(default)]
    pub anti_answer: Option<String>,
}

/// A single memory item to be ingested before querying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalMemory {
    /// Project to ingest into (usually derived from benchmark + conversation id).
    pub project: String,
    /// Synthetic session id (UUIDv4 string).
    pub session_id: String,
    /// observation.type — e.g. "fact", "event", "preference".
    pub obs_type: String,
    /// Short title (FTS-indexed).
    pub title: String,
    /// Full content (FTS-indexed).
    pub content: String,
    /// Optional topic grouping key.
    pub topic_key: Option<String>,
}
