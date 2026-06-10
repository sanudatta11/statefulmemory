// Generated with AI Coding Rules Hub
//! LoCoMo dataset adapter.
//!
//! LoCoMo (Long-term Conversational Memory) — Stanford SNAP.
//! Dataset format: a JSON array of conversation objects, each with a
//! `conversation` list of `{speaker, text}` turns and a `qa` list of
//! `{question, answer, evidence}`.
//!
//! Download: https://github.com/snap-research/locomo
//! Expected file: data/locomo/locomo10_test.json

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

use super::{EvalMemory, EvalQuery};

#[derive(Debug, Deserialize)]
#[derive(Debug, Deserialize)]
struct LoCoMoConversation {
    #[serde(rename = "conv_id")]
    conv_id: Option<String>,
    conversation: Vec<LoCoMoTurn>,
    qa: Vec<LoCoMoQA>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoTurn {
    speaker: String,
    text: Option<String>,
    #[serde(rename = "blip2_caption")]
    caption: Option<String>,
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoQA {
    #[serde(rename = "qa_id")]
    qa_id: Option<String>,
    question: String,
    answer: String,
    #[serde(rename = "evidence_list")]
    evidence_list: Option<Vec<String>>,
}

/// Load the LoCoMo dataset from `data_dir/locomo10_test.json`.
/// Returns `(memories, queries)` ready for ingestion and eval.
pub fn load(data_dir: &Path) -> Result<(Vec<EvalMemory>, Vec<EvalQuery>)> {
    let path = data_dir.join("locomo").join("locomo10_test.json");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read LoCoMo from {}", path.display()))?;
    let dataset: Vec<LoCoMoConversation> = serde_json::from_str(&raw)
        .with_context(|| "parse LoCoMo JSON")?;

    let mut memories = Vec::new();
    let mut queries = Vec::new();

    for conv in &dataset {
        let conv_id = conv.conv_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let project = format!("locomo-{conv_id}");
        let session_id = uuid::Uuid::new_v4().to_string();

        // Each turn becomes an observation.
        for (i, turn) in conv.conversation.iter().enumerate() {
            let content = match (&turn.text, &turn.caption) {
                (Some(t), _) => t.clone(),
                (None, Some(c)) => format!("[image] {c}"),
                (None, None) => continue,
            };
            let date_prefix = turn.date.as_deref().map(|d| format!("[{d}] ")).unwrap_or_default();
            memories.push(EvalMemory {
                project: project.clone(),
                session_id: session_id.clone(),
                obs_type: "conversation".to_string(),
                title: format!("{}{}: turn {i}", date_prefix, turn.speaker),
                content: format!("{}: {content}", turn.speaker),
                topic_key: None,
            });
        }

        // Each QA pair becomes an eval query.
        for qa in &conv.qa {
            let qa_id = qa.qa_id.clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let evidence = qa.evidence_list.as_ref()
                .map(|ev| ev.join("\n"))
                .unwrap_or_default();
            queries.push(EvalQuery {
                id: format!("{conv_id}-{qa_id}"),
                question: qa.question.clone(),
                gold_answer: qa.answer.clone(),
                judge_context: if evidence.is_empty() { None } else { Some(evidence) },
            });
        }
    }

    Ok((memories, queries))
}
