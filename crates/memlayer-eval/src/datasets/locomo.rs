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
//!
//! SNAP JSON `category` ids (not paper prose order):
//!   1 = multi_hop, 2 = temporal, 3 = open_domain, 4 = single_hop, 5 = adversarial

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
    /// Dialogue turn ids supporting the answer (e.g. `["D1:3", "D1:5"]`).
    #[serde(default)]
    evidence: Vec<Value>,
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Map SNAP LoCoMo JSON category ids to human-readable names used in scorecards
/// and Mem0/Engram-style tables.
pub fn category_name(raw: &str) -> String {
    match raw.trim() {
        "1" => "multi_hop".into(),
        "2" => "temporal".into(),
        "3" => "open_domain".into(),
        "4" => "single_hop".into(),
        "5" => "adversarial".into(),
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

        if let Some(conv_map) = conv.conversation.as_object() {
            // Iterate session_N keys (skip speaker_a/b and date_time keys).
            for (key, value) in conv_map {
                if !key.starts_with("session_") || key.ends_with("_date_time") {
                    continue;
                }
                // One session_id per LoCoMo session so evidence_window expands
                // real conversational neighbors (not the whole conversation).
                let session_id = format!("{conv_id}-{key}");
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
                        // Put dia_id in content (and title) so evidence-aware
                        // recall@k can match turn ids in retrieved hit text.
                        memories.push(EvalMemory {
                            project: project.clone(),
                            session_id: session_id.clone(),
                            obs_type: "conversation".to_string(),
                            title: format!("{date_prefix}{} ({})", turn.speaker, dia_label),
                            content: format!("[{dia_label}] {}: {content}", turn.speaker),
                            topic_key: None,
                        });
                    }
                }
            }
        }

        for (i, qa) in conv.qa.iter().enumerate() {
            let Some(answer) = &qa.answer else { continue };
            let evidence: Vec<String> = qa
                .evidence
                .iter()
                .map(value_to_string)
                .filter(|s| !s.trim().is_empty())
                .collect();
            let cat_raw = qa.category.as_ref().map(value_to_string);
            let category = cat_raw.as_deref().map(category_name);
            queries.push(EvalQuery {
                id: format!("{conv_id}-q{i}"),
                question: value_to_string(&qa.question),
                gold_answer: value_to_string(answer),
                judge_context: category.as_ref().map(|c| format!("category: {c}")),
                category,
                anti_answer: None,
                evidence,
            });
        }
    }

    Ok((memories, queries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_evidence_ids_and_session_per_session() {
        // Minimal LoCoMo-shaped JSON exercising evidence parsing.
        let dir = tempfile::tempdir().unwrap();
        let locomo_dir = dir.path().join("locomo");
        std::fs::create_dir_all(&locomo_dir).unwrap();
        let json = r#"[
          {
            "sample_id": "conv-test",
            "conversation": {
              "speaker_a": "A",
              "speaker_b": "B",
              "session_1_date_time": "1:00 pm on 1 January, 2023",
              "session_1": [
                {"speaker": "A", "dia_id": "D1:1", "text": "hi"},
                {"speaker": "B", "dia_id": "D1:3", "text": "went to the park"}
              ],
              "session_2_date_time": "2:00 pm on 2 January, 2023",
              "session_2": [
                {"speaker": "A", "dia_id": "D2:1", "text": "later"}
              ]
            },
            "qa": [
              {
                "question": "Where did B go?",
                "answer": "the park",
                "evidence": ["D1:3"],
                "category": 1
              }
            ]
          }
        ]"#;
        std::fs::write(locomo_dir.join("locomo10.json"), json).unwrap();
        let (mems, qs) = load(dir.path()).unwrap();
        assert_eq!(mems.len(), 3);
        let s1: Vec<_> = mems.iter().filter(|m| m.session_id == "conv-test-session_1").collect();
        let s2: Vec<_> = mems.iter().filter(|m| m.session_id == "conv-test-session_2").collect();
        assert_eq!(s1.len(), 2);
        assert_eq!(s2.len(), 1);
        assert!(mems.iter().any(|m| m.content.contains("[D1:3]")));
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].evidence, vec!["D1:3".to_string()]);
        assert_eq!(qs[0].category.as_deref(), Some("multi_hop"));
    }

    #[test]
    fn category_name_maps_snap_ids() {
        assert_eq!(category_name("1"), "multi_hop");
        assert_eq!(category_name("4"), "single_hop");
        assert_eq!(category_name("5"), "adversarial");
    }
}
