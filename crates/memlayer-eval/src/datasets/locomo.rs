// Generated with AI Coding Rules Hub
//! LoCoMo dataset adapter.
//!
//! LoCoMo (Long-term Conversational Memory) — Stanford SNAP.
//! File: data/locomo/locomo10.json
//! Format: JSON array of 10 conversation objects.
//! Each object has:
//!   - sample_id: "conv-N"
//!   - conversation: { speaker_a, speaker_b, session_1_date_time, session_1: [{speaker, dia_id, text}], ... }
//!   - qa: [{question, answer, evidence, category}]
//!
//! Download: https://github.com/snap-research/locomo

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

use super::{EvalMemory, EvalQuery};

#[derive(Debug, Deserialize)]
struct LoCoMoConversation {
    sample_id: Option<Value>,
    /// Flat dict: { speaker_a, speaker_b, session_1_date_time, session_1: [...], session_2_date_time, ... }
    conversation: Value,
    qa: Vec<LoCoMoQA>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoTurn {
    speaker: String,
    #[serde(rename = "dia_id")]
    dia_id: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoQA {
    question: Value,
    answer: Option<Value>,
    category: Option<Value>,
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Load the LoCoMo dataset from `data_dir/locomo/locomo10.json`.
pub fn load(data_dir: &Path) -> Result<(Vec<EvalMemory>, Vec<EvalQuery>)> {
    let path = data_dir.join("locomo").join("locomo10.json");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read LoCoMo from {}", path.display()))?;
    let dataset: Vec<LoCoMoConversation> = serde_json::from_str(&raw)
        .with_context(|| "parse LoCoMo JSON")?;

    let mut memories = Vec::new();
    let mut queries = Vec::new();

    for conv in &dataset {
        let conv_id = conv.sample_id.as_ref()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let project = format!("locomo-{conv_id}");
        let session_id = uuid::Uuid::new_v4().to_string();

        if let Some(conv_map) = conv.conversation.as_object() {
            // Iterate session_N keys (skip speaker_a/b and date_time keys).
            for (key, value) in conv_map {
                if !key.starts_with("session_") || key.ends_with("_date_time") {
                    continue;
                }
                // Each session_N is a list of turn objects.
                let date = conv_map.get(&format!("{key}_date_time"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if let Some(turns) = value.as_array() {
                    for turn_val in turns {
                        let turn: LoCoMoTurn = match serde_json::from_value(turn_val.clone()) {
                            Ok(t) => t,
                            Err(_) => continue,
                        };
                        let content = match &turn.text {
                            Some(t) => t.clone(),
                            None => continue,
                        };
                        let dia_label = turn.dia_id.as_deref().unwrap_or("?");
                        let date_prefix = if date.is_empty() {
                            String::new()
                        } else {
                            format!("[{date}] ")
                        };
                        memories.push(EvalMemory {
                            project: project.clone(),
                            session_id: session_id.clone(),
                            obs_type: "conversation".to_string(),
                            title: format!("{date_prefix}{} ({})", turn.speaker, dia_label),
                            content: format!("{}: {content}", turn.speaker),
                            topic_key: None,
                        });
                    }
                }
            }
        }

        for (i, qa) in conv.qa.iter().enumerate() {
            let Some(answer) = &qa.answer else { continue };
            queries.push(EvalQuery {
                id: format!("{conv_id}-q{i}"),
                question: value_to_string(&qa.question),
                gold_answer: value_to_string(answer),
                judge_context: qa.category.as_ref().map(|c| format!("category: {c}")),
            });
        }
    }

    Ok((memories, queries))
}
