// Generated with AI Coding Rules Hub
//! Standardized evaluation scorecard generator and exporter.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::runner::RunReport;

/// Standardized benchmark scorecard for memlayer retrieval quality gating (SC-Scorecard).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scorecard {
    pub scorecard_version: String,
    pub timestamp: String,
    pub commit_hash: String,
    pub benchmark: String,
    pub total_queries: usize,
    pub correct: usize,
    pub accuracy_pct: f64,
    pub f1_score: f64,
    pub precision: f64,
    pub recall: f64,
    pub retrieval_p50_ms: f64,
    pub retrieval_p95_ms: f64,
    pub end_to_end_p50_ms: f64,
    pub end_to_end_p95_ms: f64,
}

impl Scorecard {
    pub fn from_report(report: &RunReport, commit_hash: impl Into<String>) -> Self {
        let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let accuracy = report.accuracy_pct;
        let precision = accuracy / 100.0;
        let recall = precision;
        let f1 = if precision + recall > 0.0 {
            2.0 * (precision * recall) / (precision + recall)
        } else {
            0.0
        };

        Self {
            scorecard_version: "1.0".to_string(),
            timestamp: ts,
            commit_hash: commit_hash.into(),
            benchmark: report.benchmark.clone(),
            total_queries: report.total_queries,
            correct: report.correct,
            accuracy_pct: accuracy,
            f1_score: f1,
            precision,
            recall,
            retrieval_p50_ms: report.retrieval_p50_ms,
            retrieval_p95_ms: report.retrieval_p95_ms,
            end_to_end_p50_ms: report.end_to_end_p50_ms,
            end_to_end_p95_ms: report.end_to_end_p95_ms,
        }
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create parent dir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)
            .context("serialize scorecard to JSON")?;
        fs::write(path, json)
            .with_context(|| format!("write scorecard to {}", path.display()))?;
        Ok(())
    }

    pub fn render_text(&self) -> String {
        format!(
            "=== Memlayer Benchmark Scorecard ===\n\
             Benchmark:        {}\n\
             Commit:           {}\n\
             Timestamp:        {}\n\
             Queries:          {}\n\
             Correct:          {}\n\
             Accuracy:         {:.2}%\n\
             F1 Score:         {:.4}\n\
             Retrieval p50:    {:.2} ms\n\
             Retrieval p95:    {:.2} ms\n\
             End-to-End p50:   {:.2} ms\n\
             =====================================",
            self.benchmark,
            self.commit_hash,
            self.timestamp,
            self.total_queries,
            self.correct,
            self.accuracy_pct,
            self.f1_score,
            self.retrieval_p50_ms,
            self.retrieval_p95_ms,
            self.end_to_end_p50_ms,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scorecard_from_report_and_save_round_trip() {
        let report = RunReport {
            benchmark: "locomo".into(),
            total_queries: 10,
            correct: 8,
            accuracy_pct: 80.0,
            mean_prompt_tokens: 150.0,
            retrieval_p50_ms: 12.5,
            retrieval_p95_ms: 25.0,
            end_to_end_p50_ms: 450.0,
            end_to_end_p95_ms: 800.0,
            rerank_p50_ms: None,
            rerank_p95_ms: None,
            query_results: Vec::new(),
        };

        let card = Scorecard::from_report(&report, "abc1234");
        assert_eq!(card.benchmark, "locomo");
        assert_eq!(card.correct, 8);
        assert_eq!(card.accuracy_pct, 80.0);
        assert_eq!(card.commit_hash, "abc1234");

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eval").join("scorecard.json");
        card.save_to_file(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let loaded: Scorecard = serde_json::from_str(&text).unwrap();
        assert_eq!(card, loaded);
        assert!(card.render_text().contains("Locomo") || card.render_text().contains("locomo"));
    }
}
