// Generated with AI Coding Rules Hub
//! Benchmark runner: ingest → retrieve → answer → judge → report.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::{RetrievalConfig, RetrievalMode};
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
    /// Retrieval strategy + tunables (mode, k, evidence_window, rerank).
    /// Currently `mode` and `evidence_window` are recorded for reporting only;
    /// future tasks (spec-task-12/19) branch on these fields.
    pub retrieval: RetrievalConfig,
    /// Optional path to write per-query JSONL trace (full prompts, hits,
    /// answers, judge verdicts). One JSON object per line; suitable for
    /// `jq`. None = no trace file.
    pub trace_path: Option<PathBuf>,
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

    // Lazily initialise the hybrid retrieval stack only when --mode demands it.
    // Loading BGE-small + opening the cache is expensive; bm25 mode skips it.
    let hybrid_stack: Option<(
        std::sync::Arc<dyn memlayer_embed::Embedder>,
        std::sync::Arc<memlayer_embed::cache::EmbeddingCache>,
    )> = match cfg.retrieval.mode {
        RetrievalMode::Bm25 => None,
        RetrievalMode::Hybrid | RetrievalMode::HybridRerank => {
            info!("loading BGE-small embedder for hybrid retrieval");
            let embedder = std::sync::Arc::new(
                memlayer_embed::BgeSmallEmbedder::try_new()
                    .context("load BGE-small embedder")?,
            ) as std::sync::Arc<dyn memlayer_embed::Embedder>;
            let cache = std::sync::Arc::new(
                memlayer_embed::cache::EmbeddingCache::open(&cfg.data_dir)
                    .context("open embedding cache")?,
            );
            Some((embedder, cache))
        }
    };

    let mut results: Vec<QueryResult> = Vec::with_capacity(queries_to_run.len());

    // Optional per-query trace writer. JSONL: one self-describing JSON object
    // per line containing question, project, hits, prompts, raw LLM outputs,
    // and judge verdict. Used for debugging accuracy regressions.
    let mut trace_writer: Option<std::io::BufWriter<std::fs::File>> = match &cfg.trace_path {
        Some(p) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let f = std::fs::File::create(p)
                .with_context(|| format!("create trace file {}", p.display()))?;
            info!(trace = %p.display(), "writing per-query trace JSONL");
            Some(std::io::BufWriter::new(f))
        }
        None => None,
    };

    // Run queries sequentially (add semaphore for concurrency if needed).
    for q in &queries_to_run {
        let t_start = Instant::now();

        // Determine project name for this query (inferred from id prefix).
        let project = infer_project(cfg.benchmark, &q.id);

        // Retrieve — branch on mode. HybridRerank uses Hybrid for now;
        // the rerank stage lands in spec-task-21 (P3).
        let (hits, retrieval_us) = match cfg.retrieval.mode {
            RetrievalMode::Bm25 => {
                let ret = retrieve(&cfg.data_dir, &project, &q.question, cfg.k)
                    .with_context(|| format!("retrieve for query '{}'", q.id))?;
                let us = ret.latency.as_micros() as u64;
                (ret.hits, us)
            }
            RetrievalMode::Hybrid | RetrievalMode::HybridRerank => {
                let (embedder, cache) = hybrid_stack.as_ref().expect(
                    "hybrid_stack initialised when mode is Hybrid or HybridRerank",
                );
                // Prefer the P2 facts path when facts.db exists at the
                // benchmark-conventional location (data_dir/<benchmark>/facts.db).
                // EH-5 inside retrieve_facts catches missing files with a clear
                // remediation message; if facts.db is missing entirely we fall
                // back to the P1 retrieve_hybrid (raw observations + RRF) so
                // a fresh checkout still runs without `eval extract`.
                //
                // Fallback path: if retrieve_facts returns 0 hits (the file
                // exists but the queried project has no facts — e.g. partial
                // extraction), we cascade to retrieve_hybrid against raw
                // observations. Without this fallback, a project with raw
                // obs but no facts scores 0% even though BM25+dense over
                // the obs would have answered correctly.
                let facts_db_path = facts_db_path_for(cfg.benchmark, &cfg.data_dir);
                if facts_db_path.exists() {
                    let ret = crate::retrieve_facts::retrieve_facts(
                        &cfg.data_dir,
                        &facts_db_path,
                        &project,
                        &q.question,
                        cfg.k,
                        cfg.retrieval.evidence_window,
                        embedder.clone(),
                        cache.clone(),
                    )
                    .await
                    .with_context(|| format!("facts retrieve for query '{}'", q.id))?;
                    if ret.hits.is_empty() {
                        info!(
                            query_id = %q.id,
                            project = %project,
                            "facts retrieve returned 0 hits — falling back to hybrid over raw observations"
                        );
                        let fallback = crate::retrieve_hybrid::retrieve_hybrid(
                            &cfg.data_dir,
                            &project,
                            &q.question,
                            cfg.k,
                            embedder.clone(),
                            cache.clone(),
                        )
                        .await
                        .with_context(|| format!("hybrid fallback for query '{}'", q.id))?;
                        let us = (ret.latency + fallback.latency).as_micros() as u64;
                        (fallback.hits, us)
                    } else {
                        let us = ret.latency.as_micros() as u64;
                        (ret.hits, us)
                    }
                } else {
                    let ret = crate::retrieve_hybrid::retrieve_hybrid(
                        &cfg.data_dir,
                        &project,
                        &q.question,
                        cfg.k,
                        embedder.clone(),
                        cache.clone(),
                    )
                    .await
                    .with_context(|| format!("hybrid retrieve for query '{}'", q.id))?;
                    let us = ret.latency.as_micros() as u64;
                    (ret.hits, us)
                }
            }
        };

        // Build answer prompt and count tokens.
        let (system, user_msg, prompt_tokens) = build_answer_prompt(&hits, &q.question);

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

        info!(
            id = %q.id,
            correct = correct,
            hits = hits.len(),
            retrieval_ms = (retrieval_us as f64) / 1000.0,
            e2e_ms = (end_to_end_us as f64) / 1000.0,
            "query complete"
        );

        if let Some(w) = trace_writer.as_mut() {
            use std::io::Write;
            let entry = serde_json::json!({
                "id": q.id,
                "project": project,
                "question": q.question,
                "gold_answer": q.gold_answer,
                "mode": match cfg.retrieval.mode {
                    RetrievalMode::Bm25 => "bm25",
                    RetrievalMode::Hybrid => "hybrid",
                    RetrievalMode::HybridRerank => "hybrid-rerank",
                },
                "k": cfg.k,
                "evidence_window": cfg.retrieval.evidence_window,
                "retrieval_us": retrieval_us,
                "hits_count": hits.len(),
                "hits": hits,
                "answer_prompt_system": system,
                "answer_prompt_user": user_msg,
                "answer_prompt_tokens": prompt_tokens,
                "model_answer": model_answer,
                "judge_prompt": judge_prompt,
                "correct": correct,
                "end_to_end_us": end_to_end_us,
            });
            if let Err(e) = writeln!(w, "{}", entry) {
                warn!(query_id = %q.id, error = %e, "failed to write trace entry");
            }
        }

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

    let mode_tag = match cfg.retrieval.mode {
        RetrievalMode::Bm25 => "bm25",
        RetrievalMode::Hybrid => "hybrid",
        RetrievalMode::HybridRerank => "hybrid-rerank",
    };
    let report = build_report(
        cfg.benchmark,
        results,
        mode_tag,
        cfg.retrieval.evidence_window,
    );

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
            // id is the question_id directly; project is lme-{question_id}
            format!("lme-{query_id}")
        }
        BenchmarkKind::Beam1m  => "beam-1m".to_string(),
        BenchmarkKind::Beam10m => "beam-10m".to_string(),
    }
}

/// Conventional path for the eval-side facts.db for a benchmark:
/// `<data_dir>/<benchmark_kind_lower>/facts.db`. Used by both `eval extract`
/// (writes here) and the runner's hybrid path (reads from here).
pub fn facts_db_path_for(kind: BenchmarkKind, data_dir: &std::path::Path) -> PathBuf {
    let name = match kind {
        BenchmarkKind::Locomo => "locomo",
        BenchmarkKind::Longmemeval => "longmemeval",
        BenchmarkKind::Beam1m => "beam-1m",
        BenchmarkKind::Beam10m => "beam-10m",
    };
    data_dir.join(name).join("facts.db")
}

fn build_report(
    kind: BenchmarkKind,
    results: Vec<QueryResult>,
    mode_tag: &str,
    evidence_window: u8,
) -> RunReport {
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
        benchmark: format!("{kind:?} ({mode_tag}, w={evidence_window})"),
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
