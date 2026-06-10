// Generated with AI Coding Rules Hub
//! LongMemEval dataset adapter.
//!
//! LongMemEval — HuggingFace: xiaowu0162/longmemeval-cleaned
//! Files: longmemeval_oracle.json, longmemeval_s_cleaned.json, longmemeval_m_cleaned.json
//!
//! Each file is a JSON array. Each element:
//!   { "question_id", "question_type", "question", "answer", "question_date",
//!     "haystack_sessions": [ [{role, content, has_answer?}] ] }
//!
//! We use longmemeval_s_cleaned.json (single-session) for the default run.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

use super::{EvalMemory, EvalQuery};

#[derive(Debug, Deserialize)]
struct LmeRecord {
    question_id: Option<String>,
    question_type: Option<String>,
    question: String,
    answer: String,
    /// List of sessions; each session is a list of turns.
    haystack_sessions: Vec<Vec<LmeTurn>>,
}

#[derive(Debug, Deserialize)]
struct LmeTurn {
    role: String,
    content: String,
}

/// Load LongMemEval from `data_dir/longmemeval/longmemeval_s_cleaned.json`.
pub fn load(data_dir: &Path) -> Result<(Vec<EvalMemory>, Vec<EvalQuery>)> {
    let path = data_dir.join("longmemeval").join("longmemeval_s_cleaned.json");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read LongMemEval from {}", path.display()))?;
    let records: Vec<LmeRecord> = serde_json::from_str(&raw)
        .with_context(|| "parse LongMemEval JSON")?;

    let mut memories: Vec<EvalMemory> = Vec::new();
    let mut queries: Vec<EvalQuery> = Vec::new();

    for record in &records {
        let q_id = record.question_id.clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let project = format!("lme-{q_id}");

        for (sess_idx, session) in record.haystack_sessions.iter().enumerate() {
            let session_id = uuid::Uuid::new_v4().to_string();
            for (turn_idx, turn) in session.iter().enumerate() {
                memories.push(EvalMemory {
                    project: project.clone(),
                    session_id: session_id.clone(),
                    obs_type: "conversation".to_string(),
                    title: format!("{} (s{}-t{})", turn.role, sess_idx, turn_idx),
                    content: format!("{}: {}", turn.role, turn.content),
                    topic_key: None,
                });
            }
        }

        queries.push(EvalQuery {
            id: q_id.clone(),
            question: record.question.clone(),
            gold_answer: record.answer.clone(),
            judge_context: record.question_type.as_ref().map(|t| format!("type: {t}")),
        });
    }

    Ok((memories, queries))
}
