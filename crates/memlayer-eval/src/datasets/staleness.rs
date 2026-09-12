//! Staleness benchmark: measure how often a superseded value is still served.
//!
//! Each JSONL line is one atomic transition (stale → current). The loader emits
//! two memories (stale first, then current, shared `topic_key`) and one query.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

use super::{EvalMemory, EvalQuery};

#[derive(Debug, Deserialize)]
struct Transition {
    id: String,
    stale_statement: String,
    current_statement: String,
    question: String,
    gold_answer: String,
    #[serde(default)]
    topic_key: Option<String>,
    #[serde(default)]
    anti_answer: Option<String>,
    #[serde(default)]
    stale_value: Option<String>,
}

/// Load transitions from `path` (JSONL). Prefer the committed fixture under
/// `crates/memlayer-eval/fixtures/staleness-sample.jsonl` when `data/` is empty.
pub fn load(data_dir: &Path) -> Result<(Vec<EvalMemory>, Vec<EvalQuery>)> {
    let primary = data_dir.join("staleness").join("transitions.jsonl");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("staleness-sample.jsonl");
    let path = if primary.exists() {
        primary
    } else if fixture.exists() {
        fixture
    } else {
        anyhow::bail!(
            "staleness dataset missing: tried {} and {}",
            primary.display(),
            fixture.display()
        );
    };
    load_jsonl(&path)
}

fn load_jsonl(path: &Path) -> Result<(Vec<EvalMemory>, Vec<EvalQuery>)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read staleness from {}", path.display()))?;
    let mut memories = Vec::new();
    let mut queries = Vec::new();

    for (line_no, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let t: Transition = serde_json::from_str(line)
            .with_context(|| format!("parse staleness line {}", line_no + 1))?;
        let project = format!("staleness-{}", t.id);
        let session_id = format!("staleness-sess-{}", t.id);
        let topic = t
            .topic_key
            .clone()
            .unwrap_or_else(|| format!("staleness/{}", t.id));
        let anti = t
            .anti_answer
            .or(t.stale_value)
            .unwrap_or_else(|| {
                // Fallback: anything that differs between statements is not ideal;
                // callers should set anti_answer / stale_value explicitly.
                String::new()
            });

        memories.push(EvalMemory {
            project: project.clone(),
            session_id: session_id.clone(),
            obs_type: "decision".into(),
            title: format!("{} (stale)", t.id),
            content: t.stale_statement.clone(),
            topic_key: Some(topic.clone()),
        });
        memories.push(EvalMemory {
            project: project.clone(),
            session_id: session_id.clone(),
            obs_type: "decision".into(),
            title: format!("{} (current)", t.id),
            content: t.current_statement.clone(),
            topic_key: Some(topic),
        });
        queries.push(EvalQuery {
            id: format!("{}-q", t.id),
            question: t.question,
            gold_answer: t.gold_answer,
            judge_context: None,
            category: Some("staleness".into()),
            anti_answer: if anti.is_empty() { None } else { Some(anti) },
            evidence: Vec::new(),
        });
    }

    Ok((memories, queries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_loads_pairs() {
        let (mems, qs) = load(Path::new("/nonexistent")).unwrap();
        assert!(!qs.is_empty());
        assert_eq!(mems.len(), qs.len() * 2);
        assert!(qs.iter().all(|q| q.anti_answer.is_some()));
    }
}
