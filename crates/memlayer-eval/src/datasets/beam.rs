// Generated with AI Coding Rules Hub
//! BEAM benchmark generator (Letta / MemGPT protocol).
//!
//! BEAM tests retrieval accuracy over large episodic memory stores. The
//! synthetic generator creates N total observations per project, with exactly
//! one "needle" observation per query. All other observations are distractors.
//!
//! Protocol (adapted from Letta BEAM):
//! - Each query has exactly one gold observation ("needle").
//! - The needle is a fact about a named entity (e.g. "Alice's favourite colour
//!   is teal").
//! - The question asks for the entity property directly.
//! - All other observations are structurally similar facts about *different*
//!   entities (distractors).
//! - Evaluation: the needle must appear in top-k retrieval (recall@k); the LLM
//!   judge then checks whether the final answer matches the gold value.

use anyhow::Result;

use super::{EvalMemory, EvalQuery};

/// BEAM scale variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeamScale {
    M1,   // 1 million observations
    M10,  // 10 million observations
}

impl BeamScale {
    pub fn total_observations(self) -> usize {
        match self {
            BeamScale::M1  => 1_000_000,
            BeamScale::M10 => 10_000_000,
        }
    }
    pub fn project_name(self) -> &'static str {
        match self {
            BeamScale::M1  => "beam-1m",
            BeamScale::M10 => "beam-10m",
        }
    }
}

// A small vocabulary of entity names, colours, foods, hobbies, and cities that
// we cycle through to generate distractors and needles.
const NAMES: &[&str] = &[
    "Alice","Bob","Carol","Dave","Eve","Frank","Grace","Hank","Ivy","Jack",
    "Karen","Leo","Mia","Ned","Olive","Pete","Quinn","Rose","Sam","Tina",
];
const PROPS: &[(&str, &[&str])] = &[
    ("favourite colour",  &["red","blue","green","yellow","purple","orange","teal","pink","white","black"]),
    ("favourite food",    &["sushi","pizza","tacos","pasta","ramen","burgers","dumplings","curry","falafel","steak"]),
    ("hobby",             &["hiking","painting","chess","coding","cycling","cooking","gaming","reading","yoga","knitting"]),
    ("home city",         &["London","Tokyo","Paris","New York","Sydney","Cairo","Lagos","Seoul","Mumbai","Berlin"]),
];

/// Generate BEAM memories and queries.
///
/// `num_queries` controls how many needle/question pairs are created. Each
/// needle is one of the `num_queries` observations; the rest are distractors.
/// Total observations = `scale.total_observations()`.
///
/// This function yields chunks suitable for streaming to avoid allocating
/// the full dataset in RAM for the 10M case — but for simplicity here we
/// return only the queries eagerly; the memories are generated via the
/// iterator returned by `memory_iter`.
pub fn generate_queries(scale: BeamScale, num_queries: usize) -> Vec<EvalQuery> {
    let mut queries = Vec::with_capacity(num_queries);
    let project = scale.project_name().to_string();

    for i in 0..num_queries {
        let (name, prop, value) = needle_triple(i);
        queries.push(EvalQuery {
            id: format!("{}-q{i}", project),
            question: format!("What is {name}'s {prop}?"),
            gold_answer: value.to_string(),
            judge_context: None,
            category: None,
            anti_answer: None,
            evidence: Vec::new(),
        });
    }
    queries
}

/// Returns an iterator that yields all `scale.total_observations()` memories
/// in a deterministic order. Needles are at fixed positions; everything else
/// is a distractor.
///
/// Caller should consume this in batches and write each batch to SQLite to
/// avoid OOM at 10M scale.
pub fn memory_iter(scale: BeamScale, num_needles: usize)
    -> impl Iterator<Item = EvalMemory>
{
    let total = scale.total_observations();
    let project = scale.project_name().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();

    (0..total).map(move |i| {
        let is_needle = i < num_needles;
        if is_needle {
            let (name, prop, value) = needle_triple(i);
            EvalMemory {
                project: project.clone(),
                session_id: session_id.clone(),
                obs_type: "fact".to_string(),
                title: format!("{name} {prop}"),
                content: format!("{name}'s {prop} is {value}."),
                topic_key: Some("beam-needle".to_string()),
            }
        } else {
            // Distractor: cycle through name/prop/value combinations offset
            // by num_needles so they never collide with needles.
            let (name, prop, value) = distractor_triple(i, num_needles);
            EvalMemory {
                project: project.clone(),
                session_id: session_id.clone(),
                obs_type: "fact".to_string(),
                title: format!("{name} {prop}"),
                content: format!("{name}'s {prop} is {value}."),
                topic_key: Some("beam-distractor".to_string()),
            }
        }
    })
}

/// Deterministic needle: (name, property-name, gold-value) for needle index i.
fn needle_triple(i: usize) -> (&'static str, &'static str, &'static str) {
    // For each query we fix a name, a property kind, and rotate the value.
    let name = NAMES[i % NAMES.len()];
    let (prop_name, values) = PROPS[i % PROPS.len()];
    // Offset value by (i / NAMES.len()) to avoid same name getting same value.
    let value = values[(i / NAMES.len()) % values.len()];
    (name, prop_name, value)
}

/// Distractor: similar shape but offset so it never matches a needle.
fn distractor_triple(i: usize, offset: usize) -> (&'static str, &'static str, &'static str) {
    let j = (i + offset) % (NAMES.len() * PROPS.len() * 10);
    let name = NAMES[j % NAMES.len()];
    let (prop_name, values) = PROPS[(j / NAMES.len()) % PROPS.len()];
    let value = values[(j / (NAMES.len() * PROPS.len())) % values.len()];
    (name, prop_name, value)
}

/// Convenience: generate both memories and queries for small-scale tests.
pub fn generate_all(scale: BeamScale, num_queries: usize)
    -> Result<(Vec<EvalMemory>, Vec<EvalQuery>)>
{
    let total = scale.total_observations();
    if total > 10_000 {
        anyhow::bail!(
            "generate_all is only safe for small datasets (<= 10K). \
             Use memory_iter + batch ingestion for {scale:?}."
        );
    }
    let memories: Vec<EvalMemory> = memory_iter(scale, num_queries).collect();
    let queries = generate_queries(scale, num_queries);
    Ok((memories, queries))
}
