//! Standardized evaluation scorecard generator and exporter.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::runner::{CategoryStats, RunReport};

/// Standardized benchmark scorecard for statefulmemory retrieval quality gating (SC-Scorecard).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scorecard {
    pub scorecard_version: String,
    pub timestamp: String,
    pub commit_hash: String,
    pub benchmark: String,
    pub total_queries: usize,
    pub correct: usize,
    pub accuracy_pct: f64,
    /// Retrieval recall@k: evidence turn present (when provided) or gold
    /// substring present in the retrieved set.
    pub recall_at_k: f64,
    pub mrr: f64,
    /// Gold-answer substring recall (diagnostic; often lower than evidence R@k).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_substring_recall: Option<f64>,
    /// Fraction of queries where LLM rerank was configured but skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_skipped_pct: Option<f64>,
    /// Judge/lexical pass rate per category.
    #[serde(default)]
    pub by_category: BTreeMap<String, CategoryStats>,
    pub total_prompt_tokens: usize,
    pub mean_prompt_tokens: f64,
    /// Token-overlap F1 is not implemented; kept as null so tooling does not
    /// confuse accuracy with published paper F1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_f1: Option<f64>,
    /// Staleness benchmark only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_served_pct: Option<f64>,
    /// Answer LLM pin when disclosed via env (`STATEFULMEMORY_LLM_MODEL` / provider).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_model: Option<String>,
    /// Judge LLM pin when disclosed (same env family; roles may differ).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
    pub retrieval_p50_ms: f64,
    pub retrieval_p95_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_p95_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_p95_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_p95_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_timeout_pct: Option<f64>,
    pub end_to_end_p50_ms: f64,
    pub end_to_end_p95_ms: f64,
    /// Queries the run was asked to evaluate (after limit / stratified sample).
    #[serde(default)]
    pub queries_requested: usize,
    /// Queries dropped because answer/judge LLM failed after retries.
    /// Non-zero is disclosed (and publishable only while the runner's
    /// ≤1% tolerance holds — field exists so JSON stays self-describing).
    #[serde(default)]
    pub queries_skipped: usize,
}

impl Scorecard {
    pub fn from_report(report: &RunReport, commit_hash: impl Into<String>) -> Self {
        let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let (answer_model, judge_model) = llm_model_disclosure();

        Self {
            scorecard_version: "2.0".to_string(),
            timestamp: ts,
            commit_hash: commit_hash.into(),
            benchmark: report.benchmark.clone(),
            total_queries: report.total_queries,
            correct: report.correct,
            accuracy_pct: report.accuracy_pct,
            recall_at_k: report.recall_at_k,
            mrr: report.mrr,
            gold_substring_recall: report.gold_substring_recall,
            rerank_skipped_pct: report.rerank_skipped_pct,
            by_category: report.by_category.clone(),
            total_prompt_tokens: report.total_prompt_tokens,
            mean_prompt_tokens: report.mean_prompt_tokens,
            token_f1: None,
            superseded_served_pct: report.superseded_served_pct,
            answer_model,
            judge_model,
            retrieval_p50_ms: report.retrieval_p50_ms,
            retrieval_p95_ms: report.retrieval_p95_ms,
            rerank_p50_ms: report.rerank_p50_ms,
            rerank_p95_ms: report.rerank_p95_ms,
            answer_p50_ms: report.answer_p50_ms,
            answer_p95_ms: report.answer_p95_ms,
            judge_p50_ms: report.judge_p50_ms,
            judge_p95_ms: report.judge_p95_ms,
            rerank_timeout_pct: report.rerank_timeout_pct,
            end_to_end_p50_ms: report.end_to_end_p50_ms,
            end_to_end_p95_ms: report.end_to_end_p95_ms,
            queries_requested: report.queries_requested,
            queries_skipped: report.queries_skipped,
        }
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create parent dir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("serialize scorecard to JSON")?;
        fs::write(path, json).with_context(|| format!("write scorecard to {}", path.display()))?;
        Ok(())
    }

    pub fn render_text(&self) -> String {
        let mut out = format!(
            "=== StatefulMemory Benchmark Scorecard ===\n\
             Benchmark:        {}\n\
             Commit:           {}\n\
             Timestamp:        {}\n\
             Queries:          {}\n\
             Correct:          {}\n\
             Accuracy:         {:.2}%\n\
             Recall@k:         {:.4}\n\
             MRR:              {:.4}\n\
             Prompt tokens:    {} (mean {:.0})\n\
             Retrieval p50:    {:.2} ms\n\
             Retrieval p95:    {:.2} ms\n\
             End-to-End p50:   {:.2} ms\n",
            self.benchmark,
            self.commit_hash,
            self.timestamp,
            self.total_queries,
            self.correct,
            self.accuracy_pct,
            self.recall_at_k,
            self.mrr,
            self.total_prompt_tokens,
            self.mean_prompt_tokens,
            self.retrieval_p50_ms,
            self.retrieval_p95_ms,
            self.end_to_end_p50_ms,
        );
        if let Some(gsr) = self.gold_substring_recall {
            out.push_str(&format!("Gold-substring R:  {:.4}\n", gsr));
        }
        if let Some(p50) = self.rerank_p50_ms {
            out.push_str(&format!("Rerank p50:        {p50:.2} ms\n"));
        }
        if let Some(p95) = self.rerank_p95_ms {
            out.push_str(&format!("Rerank p95:        {p95:.2} ms\n"));
        }
        if let Some(p50) = self.answer_p50_ms {
            out.push_str(&format!("Answer p50:        {p50:.2} ms\n"));
        }
        if let Some(p95) = self.answer_p95_ms {
            out.push_str(&format!("Answer p95:        {p95:.2} ms\n"));
        }
        if let Some(p50) = self.judge_p50_ms {
            out.push_str(&format!("Judge p50:         {p50:.2} ms\n"));
        }
        if let Some(p95) = self.judge_p95_ms {
            out.push_str(&format!("Judge p95:         {p95:.2} ms\n"));
        }
        if let Some(pct) = self.rerank_timeout_pct {
            out.push_str(&format!("Rerank timeouts:   {pct:.1}%\n"));
        }
        if let Some(rsp) = self.rerank_skipped_pct {
            out.push_str(&format!("Rerank skipped:    {:.1}%\n", rsp));
        }
        if let Some(m) = &self.answer_model {
            out.push_str(&format!("Answer model:      {m}\n"));
        }
        if let Some(m) = &self.judge_model {
            out.push_str(&format!("Judge model:       {m}\n"));
        }
        if let Some(pct) = self.superseded_served_pct {
            out.push_str(&format!("Superseded served: {:.2}%\n", pct));
        }
        if self.queries_requested > 0 {
            out.push_str(&format!("Queries requested:  {}\n", self.queries_requested));
        }
        if self.queries_skipped > 0 {
            out.push_str(&format!(
                "Queries SKIPPED:    {} (≤1% tolerance — disclosed, not hidden)\n",
                self.queries_skipped
            ));
        }
        if !self.by_category.is_empty() {
            out.push_str("By category:\n");
            for (name, stats) in &self.by_category {
                out.push_str(&format!(
                    "  {name}: {:.1}% ({}/{})\n",
                    stats.accuracy_pct, stats.correct, stats.total
                ));
            }
        }
        out.push_str("=====================================");
        out
    }
}

/// Disclose answer/judge model pins from env when present (measure recipe).
fn llm_model_disclosure() -> (Option<String>, Option<String>) {
    let model = std::env::var("STATEFULMEMORY_LLM_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let provider = std::env::var("STATEFULMEMORY_LLM_PROVIDER")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let pin = match (provider, model) {
        (Some(p), Some(m)) => Some(format!("{p}/{m}")),
        (None, Some(m)) => Some(m),
        (Some(p), None) => Some(p),
        (None, None) => None,
    };
    // Same pin for both roles unless split env is added later; disclose once each.
    (pin.clone(), pin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn sample_report() -> RunReport {
        let mut by_category = BTreeMap::new();
        by_category.insert(
            "single_hop".into(),
            CategoryStats {
                total: 5,
                correct: 4,
                accuracy_pct: 80.0,
            },
        );
        RunReport {
            benchmark: "locomo".into(),
            total_queries: 10,
            correct: 8,
            accuracy_pct: 80.0,
            mean_prompt_tokens: 150.0,
            total_prompt_tokens: 1500,
            recall_at_k: 0.9,
            mrr: 0.75,
            gold_substring_recall: Some(0.4),
            rerank_skipped_pct: Some(25.0),
            by_category,
            superseded_served_pct: None,
            retrieval_p50_ms: 12.5,
            retrieval_p95_ms: 25.0,
            end_to_end_p50_ms: 450.0,
            end_to_end_p95_ms: 800.0,
            rerank_p50_ms: Some(11.0),
            rerank_p95_ms: Some(22.0),
            answer_p50_ms: Some(30.0),
            answer_p95_ms: Some(40.0),
            judge_p50_ms: Some(50.0),
            judge_p95_ms: Some(60.0),
            rerank_timeout_pct: Some(0.0),
            queries_requested: 10,
            queries_skipped: 0,
            query_results: Vec::new(),
        }
    }

    #[test]
    fn scorecard_from_report_and_save_round_trip() {
        let report = sample_report();
        let card = Scorecard::from_report(&report, "abc1234");
        assert_eq!(card.benchmark, "locomo");
        assert_eq!(card.correct, 8);
        assert_eq!(card.accuracy_pct, 80.0);
        assert_eq!(card.commit_hash, "abc1234");
        assert_eq!(card.scorecard_version, "2.0");
        assert_eq!(card.recall_at_k, 0.9);
        assert_eq!(card.mrr, 0.75);
        assert_eq!(card.rerank_p50_ms, Some(11.0));
        assert_eq!(card.rerank_p95_ms, Some(22.0));
        assert_eq!(card.answer_p50_ms, Some(30.0));
        assert_eq!(card.judge_p95_ms, Some(60.0));
        assert!(card.token_f1.is_none());
        assert!(!card.render_text().contains("F1 Score"));

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eval").join("scorecard.json");
        card.save_to_file(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let loaded: Scorecard = serde_json::from_str(&text).unwrap();
        assert_eq!(card, loaded);
        assert!(card.render_text().contains("locomo"));
    }

    #[test]
    fn by_category_survives_serialization() {
        let card = Scorecard::from_report(&sample_report(), "deadbeef");
        let json = serde_json::to_string(&card).unwrap();
        let loaded: Scorecard = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.by_category.len(), 1);
        let stats = loaded.by_category.get("single_hop").unwrap();
        assert_eq!(stats.correct, 4);
        assert_eq!(stats.total, 5);
        assert_eq!(loaded.queries_requested, 10);
        assert_eq!(loaded.queries_skipped, 0);
    }

    #[test]
    fn skipped_queries_render_warning() {
        let mut report = sample_report();
        report.queries_skipped = 3;
        let card = Scorecard::from_report(&report, "deadbeef");
        assert_eq!(card.queries_skipped, 3);
        assert!(card.render_text().contains("SKIPPED"));
        assert!(card.render_text().contains("tolerance"));
    }
}
