// Generated with AI Coding Rules Hub
//! LongMemEval dataset adapter.
//!
//! LongMemEval — 5 task categories over long conversation histories.
//! Dataset format: JSONL, one record per question.
//!   { "session_id", "question", "answer", "category", "evidence_turns": [...] }
//!   Companion file: sessions.json  { session_id → [ {speaker, text, turn_id} ] }
//!
//! Download: https://github.com/xiaowu0162/LongMemEval
//! Expected files:
//!   data/longmemeval/questions.jsonl
//!   data/longmemeval/sessions.json

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

use super::{EvalMemory, EvalQuery};

#[derive(Debug, Deserialize)]
struct LmeQuestion {
    #[serde(rename = "q_id")]
    q_id: Option<String>,
    session_id: String,
    question: String,
    answer: String,
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LmeTurn {
    turn_id: Option<u32>,
    speaker: String,
    text: Option<String>,
}

/// Load LongMemEval from `data_dir/longmemeval/`.
/// Returns `(memories, queries)`.
pub fn load(data_dir: &Path) -> Result<(Vec<EvalMemory>, Vec<EvalQuery>)> {
    let q_path = data_dir.join("longmemeval").join("questions.jsonl");
    let s_path = data_dir.join("longmemeval").join("sessions.json");

    // Parse sessions map first.
    let sessions_raw = std::fs::read_to_string(&s_path)
        .with_context(|| format!("read LongMemEval sessions from {}", s_path.display()))?;
    let sessions_map: HashMap<String, Vec<LmeTurn>> = serde_json::from_str(&sessions_raw)
        .with_context(|| "parse LongMemEval sessions.json")?;

    // Parse questions.
    let q_raw = std::fs::read_to_string(&q_path)
        .with_context(|| format!("read LongMemEval questions from {}", q_path.display()))?;
    let questions: Vec<LmeQuestion> = q_raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<LmeQuestion>(l)
            .with_context(|| format!("parse LME question line: {l}")))
        .collect::<Result<_>>()?;

    let mut memories: Vec<EvalMemory> = Vec::new();
    let mut seen_sessions: std::collections::HashSet<String> = Default::default();
    let mut queries: Vec<EvalQuery> = Vec::new();

    for q in &questions {
        // Ingest session turns once per unique session_id.
        if seen_sessions.insert(q.session_id.clone()) {
            if let Some(turns) = sessions_map.get(&q.session_id) {
                let project = format!("lme-{}", q.session_id);
                let session_id = uuid::Uuid::new_v4().to_string();
                for turn in turns {
                    let content = match &turn.text {
                        Some(t) => t.clone(),
                        None => continue,
                    };
                    let turn_label = turn.turn_id.map(|i| format!("turn-{i}"))
                        .unwrap_or_else(|| "turn".to_string());
                    memories.push(EvalMemory {
                        project: project.clone(),
                        session_id: session_id.clone(),
                        obs_type: "conversation".to_string(),
                        title: format!("{}: {}", turn.speaker, turn_label),
                        content: format!("{}: {content}", turn.speaker),
                        topic_key: q.category.clone(),
                    });
                }
            }
        }

        let q_id = q.q_id.clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        queries.push(EvalQuery {
            id: format!("{}-{q_id}", q.session_id),
            question: q.question.clone(),
            gold_answer: q.answer.clone(),
            judge_context: q.category.as_ref().map(|c| format!("category: {c}")),
        });
    }

    Ok((memories, queries))
}
