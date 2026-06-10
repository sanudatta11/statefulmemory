// Generated with AI Coding Rules Hub
//! Benchmark runner: ingest → retrieve → answer → judge → report.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::datasets::{EvalMemory, EvalQuery};
use crate::judge::JudgeClient;
use crate::prompt::{build_answer_prompt, build_judge_prompt};
use crate::retrieve::retrieve;

/// Which benchmark to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BenchmarkKind {
    Locomo,
    Longmemeval,
    Beam1m,
    Beam10m,
}

impl BenchmarkKind {
    pub fn dataset_project_prefix(self) -> &'static str {
        match self {
            Self::Locomo      => "locomo-",
            Self::Longmemeval => "lme-",
            Self::Beam1m      => "beam-1m",
            Self::Beam10m     => "beam-10m",
        }
    }
}

/// Parameters for a single benchmark run.
pub struct RunConfig {
    pub benchmark: BenchmarkKind,
    pub data_dir: PathBuf,
    pub k: i32,
    /// If `Some(n)`, stop after n queries (smoke-test mode).
    pub limit: Option<usize>,
    /// Number of concurrent LLM calls.
    pub concurrency: usize,
    pub output_path: PathBuf,
    /// Whether to skip ingestion (data already prepared).
    pub skip_ingest: bool,
}

/// Per-query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub id: String,
    pub question: String,
    pub gold_answer: String,
    pub model_answer: String,
    pub correct: bool,
    pub retrieval_us: u64,
    pub end_to_end_us: u64,
    pub prompt_tokens: usize,
}

/// Aggregated benchmark report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub benchmark: String,
    pub total_queries: usize,
    pub correct: usize,
    pub accuracy_pct: f64,
    pub mean_prompt_tokens: f64,
    pub retrieval_p50_ms: f64,
    pub retrieval_p95_ms: f64,
    pub end_to_end_p50_ms: f64,
    pub end_to_end_p95_ms: f64,
    pub query_results: Vec<QueryResult>,
}

impl RunReport {
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(&format!("# memlayer Benchmark: {}\n\n", self.benchmark));
        md.push_str("## Summary\n\n");
        md.push_str("| Metric | Value |\n|---|---|\n");
        md.push_str(&format!("| Accuracy | {:.1}% ({}/{}) |\n",
            self.accuracy_pct, self.correct, self.total_queries));
        md.push_str(&format!("| Mean prompt tokens | {:.0} |\n", self.mean_prompt_tokens));
        md.push_str(&format!("| Retrieval p50 | {:.2}ms |\n", self.retrieval_p50_ms));
        md.push_str(&format!("| Retrieval p95 | {:.2}ms |\n", self.retrieval_p95_ms));
        md.push_str(&format!("| End-to-end p50 | {:.2}ms |\n", self.end_to_end_p50_ms));
        md.push_str(&format!("| End-to-end p95 | {:.2}ms |\n\n", self.end_to_end_p95_ms));
        md.push_str("## Per-query results\n\n");
        md.push_str("| id | correct | ret_ms | e2e_ms | tokens |\n|---|---|---|---|---|\n");
        for q in &self.query_results {
            md.push_str(&format!(
                "| {} | {} | {:.1} | {:.1} | {} |\n",
                q.id,
                if q.correct { "✓" } else { "✗" },
                q.retrieval_us as f64 / 1000.0,
                q.end_to_end_us as f64 / 1000.0,
                q.prompt_tokens,
            ));
        }
        md
    }
}

/// Run the full benchmark pipeline for pre-loaded memories and queries.
pub async fn run(
    cfg: &RunConfig,
    memories: Vec<EvalMemory>,
    queries: Vec<EvalQuery>,
) -> Result<RunReport> {
    // --- Ingest ---
    if !cfg.skip_ingest {
        info!(count = memories.len(), "ingesting memories");
        crate::ingest::ingest_memories(&cfg.data_dir, &memories, 500).await
            .context("ingest")?;
    }

    // --- Evaluate ---
    let judge = JudgeClient::new().context("create judge client")?;
    let queries_to_run: Vec<_> = match cfg.limit {
        Some(n) => queries.into_iter().take(n).collect(),
        None    => queries,
    };

    info!(count = queries_to_run.len(), k = cfg.k, "running queries");

    let mut results: Vec<QueryResult> = Vec::with_capacity(queries_to_run.len());

    // Run queries sequentially (add semaphore for concurrency if needed).
    for q in &queries_to_run {
        let t_start = Instant::now();

        // Determine project name for this query (inferred from id prefix).
        let project = infer_project(cfg.benchmark, &q.id);

        // Retrieve.
        let ret = retrieve(&cfg.data_dir, &project, &q.question, cfg.k)
            .with_context(|| format!("retrieve for query '{}'", q.id))?;
        let retrieval_us = ret.latency.as_micros() as u64;

        // Build answer prompt and count tokens.
        let (system, user_msg, prompt_tokens) = build_answer_prompt(&ret.hits, &q.question);

        // Answer LLM call.
        let model_answer = match judge.answer(&system, &user_msg).await {
            Ok(a) => a,
            Err(e) => {
                warn!(query_id = %q.id, error = %e, "answer LLM failed, skipping query");
                continue;
            }
        };

        // Judge call.
        let judge_prompt = build_judge_prompt(
            &q.question,
            &q.gold_answer,
            &model_answer,
            q.judge_context.as_deref(),
        );
        let correct = match judge.judge(&judge_prompt).await {
            Ok(c) => c,
            Err(e) => {
                warn!(query_id = %q.id, error = %e, "judge LLM failed, marking incorrect");
                false
            }
        };

        let end_to_end_us = t_start.elapsed().as_micros() as u64;

        results.push(QueryResult {
            id: q.id.clone(),
            question: q.question.clone(),
            gold_answer: q.gold_answer.clone(),
            model_answer,
            correct,
            retrieval_us,
            end_to_end_us,
            prompt_tokens,
        });
    }

    let report = build_report(cfg.benchmark, results);

    // Write JSON + Markdown.
    let json = serde_json::to_string_pretty(&report).context("serialize report")?;
    let json_path = cfg.output_path.with_extension("json");
    std::fs::write(&json_path, &json).context("write JSON report")?;
    std::fs::write(&cfg.output_path, report.to_markdown()).context("write Markdown report")?;
    info!(md = %cfg.output_path.display(), json = %json_path.display(), "report written");

    Ok(report)
}

fn infer_project(kind: BenchmarkKind, query_id: &str) -> String {
    match kind {
        BenchmarkKind::Locomo => {
            // id format: "{conv_id}-{qa_id}"
            let conv_id = query_id.rsplitn(2, '-').nth(1).unwrap_or(query_id);
            format!("locomo-{conv_id}")
        }
        BenchmarkKind::Longmemeval => {
            // id format: "{session_id}-{q_id}"
            let session_id = query_id.rsplitn(2, '-').nth(1).unwrap_or(query_id);
            format!("lme-{session_id}")
        }
        BenchmarkKind::Beam1m  => "beam-1m".to_string(),
        BenchmarkKind::Beam10m => "beam-10m".to_string(),
    }
}

fn build_report(kind: BenchmarkKind, results: Vec<QueryResult>) -> RunReport {
    let total = results.len();
    let correct = results.iter().filter(|r| r.correct).count();
    let accuracy_pct = if total == 0 { 0.0 } else { correct as f64 / total as f64 * 100.0 };

    let mean_tokens = if total == 0 {
        0.0
    } else {
        results.iter().map(|r| r.prompt_tokens as f64).sum::<f64>() / total as f64
    };

    let retrieval_p50 = percentile_ms(&mut results.iter().map(|r| r.retrieval_us).collect::<Vec<_>>(), 50);
    let retrieval_p95 = percentile_ms(&mut results.iter().map(|r| r.retrieval_us).collect::<Vec<_>>(), 95);
    let e2e_p50 = percentile_ms(&mut results.iter().map(|r| r.end_to_end_us).collect::<Vec<_>>(), 50);
    let e2e_p95 = percentile_ms(&mut results.iter().map(|r| r.end_to_end_us).collect::<Vec<_>>(), 95);

    RunReport {
        benchmark: format!("{kind:?}"),
        total_queries: total,
        correct,
        accuracy_pct,
        mean_prompt_tokens: mean_tokens,
        retrieval_p50_ms: retrieval_p50,
        retrieval_p95_ms: retrieval_p95,
        end_to_end_p50_ms: e2e_p50,
        end_to_end_p95_ms: e2e_p95,
        query_results: results,
    }
}

fn percentile_ms(values: &mut Vec<u64>, p: usize) -> f64 {
    if values.is_empty() { return 0.0; }
    values.sort_unstable();
    let idx = ((p as f64 / 100.0) * (values.len() - 1) as f64).round() as usize;
    values[idx.min(values.len() - 1)] as f64 / 1000.0
}
