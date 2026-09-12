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

/// In-place stderr progress for long eval runs.
///
/// On a TTY, `status` rewrites one sticky line with `\r` (no scroll spam).
/// Phase messages use `note`, which commits a newline. Non-TTY (CI / redirected
/// logs) falls back to one newline per update so capture still works.
struct CliProgress {
    tty: bool,
    /// Visible width of the last sticky status line (for padding clears).
    sticky_len: usize,
}

impl CliProgress {
    fn new() -> Self {
        use std::io::IsTerminal;
        Self {
            tty: std::io::stderr().is_terminal(),
            sticky_len: 0,
        }
    }

    fn status(&mut self, msg: impl std::fmt::Display) {
        use std::io::Write;
        let line = format!("[eval] {msg}");
        if self.tty {
            eprint!("\r{line}");
            if line.len() < self.sticky_len {
                let pad = self.sticky_len - line.len();
                eprint!("{:pad$}", "");
                eprint!("\r{line}");
            }
            self.sticky_len = line.len();
            let _ = std::io::stderr().flush();
        } else {
            eprintln!("{line}");
        }
    }

    fn note(&mut self, msg: impl std::fmt::Display) {
        use std::io::Write;
        if self.tty && self.sticky_len > 0 {
            // End the sticky line before a permanent note.
            eprintln!();
            self.sticky_len = 0;
        }
        eprintln!("[eval] {msg}");
        let _ = std::io::stderr().flush();
    }
}

fn pct(done: usize, total: usize) -> f64 {
    if total == 0 {
        100.0
    } else {
        (done as f64 / total as f64) * 100.0
    }
}

fn fmt_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Compact ASCII bar for percentage display, e.g. `[####------]`.
fn pct_bar(done: usize, total: usize, width: usize) -> String {
    let filled = if total == 0 {
        width
    } else {
        ((done * width) + (total / 2)) / total
    }
    .min(width);
    let mut s = String::with_capacity(width + 2);
    s.push('[');
    for i in 0..width {
        s.push(if i < filled { '#' } else { '-' });
    }
    s.push(']');
    s
}

/// LLM rerank fires only when the rank-1 vs rank-2 score delta is below
/// this threshold (P5 spec-task-31 cost gate). When the top fact is a
/// clear winner, a Haiku-shuffle costs $$ and adds judge noise without
/// materially improving accuracy.
const RERANK_AMBIGUITY_THRESHOLD: f32 = 0.15;

/// Abort after this many consecutive answer-LLM failures (bad auth / model).
const CONSECUTIVE_ANSWER_FAIL_ABORT: usize = 5;

/// Resolve eval query concurrency from env (default 4, clamp 1..=16).
pub fn eval_concurrency_from_env(cli_default: usize) -> usize {
    std::env::var("MEMLAYER_EVAL_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(cli_default)
        .clamp(1, 16)
}

/// Which benchmark to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BenchmarkKind {
    Locomo,
    Longmemeval,
    Beam1m,
    Beam10m,
    Staleness,
}

impl BenchmarkKind {
    pub fn dataset_project_prefix(self) -> &'static str {
        match self {
            Self::Locomo      => "locomo-",
            Self::Longmemeval => "lme-",
            Self::Beam1m      => "beam-1m",
            Self::Beam10m     => "beam-10m",
            Self::Staleness   => "staleness-",
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
    /// Number of facts.db shards (P4 spec-task-26). 1 = single DB (the
    /// LoCoMo / LongMemEval path). N>1 enables the BEAM sharded layout
    /// — extract routes by `ShardRouter::shard_for(obs_id)`, retrieval
    /// fans out per-shard then merges via `merge_shard_results`.
    pub shards: usize,
    /// When true, skip answer/judge LLM calls. `model_answer` is the
    /// concatenated retrieval hits; `correct` is whether `gold_answer`
    /// appears in those hits (case-insensitive). Used by `memlayer eval
    /// --smoke` so CI needs no Claude CLI.
    pub lexical_judge: bool,
    /// When true, ingest without supersession so both the stale and current
    /// statements remain retrievable. Baseline arm for the staleness
    /// benchmark; mirrors an add-only memory design.
    pub no_supersede: bool,
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
    /// Rerank stage latency (Haiku call + parse). None when rerank is
    /// disabled for the run; spec-task-23 / SC-7.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_us: Option<u64>,
    /// Benchmark-provided category label, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// 0-based rank used for recall@k / MRR: evidence-turn match when the
    /// query has evidence ids, else gold-substring match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_rank: Option<usize>,
    /// 0-based rank of the first hit containing the gold answer string
    /// (case-insensitive substring). Diagnostic only — often 0 on LoCoMo
    /// because golds are short/normalized while hits are speaker turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_substring_rank: Option<usize>,
    pub hits_count: usize,
    /// True when the superseded (anti) value was served without the gold.
    #[serde(default)]
    pub stale_served: bool,
    /// True when LLM rerank was configured but skipped (ambiguity gate).
    #[serde(default)]
    pub rerank_skipped: bool,
}

/// Per-category accuracy rollup.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CategoryStats {
    pub total: usize,
    pub correct: usize,
    pub accuracy_pct: f64,
}

/// Aggregated benchmark report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub benchmark: String,
    pub total_queries: usize,
    pub correct: usize,
    pub accuracy_pct: f64,
    pub mean_prompt_tokens: f64,
    /// Sum of per-query prompt token estimates.
    pub total_prompt_tokens: usize,
    /// Fraction of queries whose gold answer appeared anywhere in the
    /// retrieved set. Retrieval metric, independent of the judge verdict.
    /// Prefer evidence-turn match when the dataset provides evidence ids;
    /// otherwise gold-substring match.
    pub recall_at_k: f64,
    /// Mean reciprocal rank of the primary retrieval hit (evidence-aware
    /// when available).
    pub mrr: f64,
    /// Fraction of queries whose gold answer string appeared in any hit
    /// (substring). Diagnostic; usually lower than evidence-based recall
    /// on LoCoMo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_substring_recall: Option<f64>,
    /// Fraction of queries where LLM rerank was configured but skipped by
    /// the ambiguity gate. None when rerank is disabled for the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_skipped_pct: Option<f64>,
    /// Per-category totals, sorted by category name.
    #[serde(default)]
    pub by_category: std::collections::BTreeMap<String, CategoryStats>,
    /// Fraction of queries where the superseded value was served. Staleness
    /// benchmark only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_served_pct: Option<f64>,
    pub retrieval_p50_ms: f64,
    pub retrieval_p95_ms: f64,
    pub end_to_end_p50_ms: f64,
    pub end_to_end_p95_ms: f64,
    /// Reported separately so SC-5 (retrieval p50 < 50ms) is not gated
    /// by the rerank LLM call. None when rerank is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_p95_ms: Option<f64>,
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
        md.push_str(&format!("| Recall@k | {:.4} |\n", self.recall_at_k));
        md.push_str(&format!("| MRR | {:.4} |\n", self.mrr));
        if let Some(gsr) = self.gold_substring_recall {
            md.push_str(&format!("| Gold-substring recall | {:.4} |\n", gsr));
        }
        if let Some(rsp) = self.rerank_skipped_pct {
            md.push_str(&format!("| Rerank skipped | {:.1}% |\n", rsp));
        }
        md.push_str(&format!("| Mean prompt tokens | {:.0} |\n", self.mean_prompt_tokens));
        md.push_str(&format!("| Total prompt tokens | {} |\n", self.total_prompt_tokens));
        md.push_str(&format!("| Retrieval p50 | {:.2}ms |\n", self.retrieval_p50_ms));
        md.push_str(&format!("| Retrieval p95 | {:.2}ms |\n", self.retrieval_p95_ms));
        md.push_str(&format!("| End-to-end p50 | {:.2}ms |\n", self.end_to_end_p50_ms));
        md.push_str(&format!("| End-to-end p95 | {:.2}ms |\n", self.end_to_end_p95_ms));
        if let Some(p50) = self.rerank_p50_ms {
            md.push_str(&format!("| Rerank p50 | {:.2}ms |\n", p50));
        }
        if let Some(p95) = self.rerank_p95_ms {
            md.push_str(&format!("| Rerank p95 | {:.2}ms |\n", p95));
        }
        if let Some(pct) = self.superseded_served_pct {
            md.push_str(&format!("| Superseded served | {:.1}% |\n", pct));
        }
        if !self.by_category.is_empty() {
            md.push('\n');
            md.push_str("## By category\n\n");
            md.push_str("| category | correct | total | accuracy |\n|---|---|---|---|\n");
            for (name, stats) in &self.by_category {
                md.push_str(&format!(
                    "| {} | {} | {} | {:.1}% |\n",
                    name, stats.correct, stats.total, stats.accuracy_pct
                ));
            }
        }
        md.push('\n');
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
    let mut ui = CliProgress::new();

    // --- Ingest ---
    if !cfg.skip_ingest {
        info!(count = memories.len(), "ingesting memories");
        let n = memories.len();
        ui.status(format!("ingest  {:>5.1}%  0/{n}  queuing writes …", 0.0));
        crate::ingest::ingest_memories(&cfg.data_dir, &memories, 500, cfg.no_supersede)
            .await
            .context("ingest")?;
        ui.note(format!("ingest complete  ({n} memories)"));
    }

    // --- Evaluate ---
    let judge = if cfg.lexical_judge {
        None
    } else {
        ui.status("detecting agent CLI for answer/judge …");
        let j = JudgeClient::new().context("create judge client")?;
        if let Some(p) = memlayer_extract::agent_cli::detect_provider() {
            ui.note(format!(
                "LLM provider={} bin={} (override with MEMLAYER_LLM_PROVIDER / MEMLAYER_LLM_BIN)",
                p.id, p.bin
            ));
        } else {
            ui.note("agent CLI ready for answer/judge");
        }
        Some(j)
    };

    // Reuse the same shell-out client used by extraction so rerank shares
    // the proxy-strip + timeout machinery. Built once and reused per query.
    let rerank_claude: Option<std::sync::Arc<dyn memlayer_extract::claude_cli::ClaudeClient>> =
        if cfg.retrieval.rerank && !cfg.lexical_judge {
            Some(std::sync::Arc::new(
                memlayer_extract::claude_cli::ClaudeCliClient::new(),
            ))
        } else {
            None
        };

    let queries_to_run: Vec<_> = match cfg.limit {
        Some(n) => queries.into_iter().take(n).collect(),
        None    => queries,
    };

    info!(count = queries_to_run.len(), k = cfg.k, "running queries");
    let mode_label = match cfg.retrieval.mode {
        RetrievalMode::Bm25 => "bm25",
        RetrievalMode::Hybrid => "hybrid",
        RetrievalMode::HybridRerank => "hybrid-rerank",
    };
    ui.note(format!(
        "running {} queries (mode={mode_label}, k={})",
        queries_to_run.len(),
        cfg.k
    ));

    if matches!(
        cfg.retrieval.mode,
        RetrievalMode::Hybrid | RetrievalMode::HybridRerank
    ) {
        let facts_path = facts_db_path_for(cfg.benchmark, &cfg.data_dir);
        if !facts_path.exists() {
            info!(
                path = %facts_path.display(),
                "facts.db missing; using observation-level hybrid (no extract)"
            );
        }
    }

    // Lazily initialise the hybrid retrieval stack only when --mode demands it.
    // Loading BGE-small + opening the cache is expensive; bm25 mode skips it.
    let hybrid_stack: Option<(
        std::sync::Arc<dyn memlayer_embed::Embedder>,
        std::sync::Arc<memlayer_embed::cache::EmbeddingCache>,
    )> = match cfg.retrieval.mode {
        RetrievalMode::Bm25 => None,
        RetrievalMode::Hybrid | RetrievalMode::HybridRerank => {
            info!("loading BGE-small embedder for hybrid retrieval");
            ui.status("loading BGE-small embedder …");
            let embedder = std::sync::Arc::new(
                memlayer_embed::BgeSmallEmbedder::try_new()
                    .context("load BGE-small embedder")?,
            ) as std::sync::Arc<dyn memlayer_embed::Embedder>;
            let cache = std::sync::Arc::new(
                memlayer_embed::cache::EmbeddingCache::open(&cfg.data_dir)
                    .context("open embedding cache")?,
            );
            ui.note("BGE-small ready");
            Some((embedder, cache))
        }
    };

    // Pre-build sidecar vec indexes once before concurrent queries. Avoids
    // racing wipe/rebuild + concurrent ProjectRegistry write opens on the
    // same locomo-conv-* DB (pragma journal_mode disk I/O errors).
    if let Some((embedder, cache)) = hybrid_stack.as_ref() {
        let mut projects: Vec<String> = queries_to_run
            .iter()
            .map(|q| infer_project(cfg.benchmark, &q.id))
            .collect();
        projects.sort();
        projects.dedup();
        if !projects.is_empty() {
            ui.status(format!(
                "prewarming vec indexes for {} project(s) …",
                projects.len()
            ));
            crate::retrieve_hybrid::prewarm_vec_indexes(
                &cfg.data_dir,
                &projects,
                embedder.clone(),
                cache.clone(),
            )
            .await
            .context("prewarm hybrid vec indexes")?;
            ui.note(format!("vec indexes ready ({})", projects.len()));
        }
    }

    let total_queries = queries_to_run.len();
    let concurrency = cfg.concurrency.max(1);
    ui.note(format!("query concurrency={concurrency} (MEMLAYER_EVAL_CONCURRENCY)"));

    // Optional per-query trace writer (Mutex — concurrent queries).
    let trace_writer: Option<std::sync::Arc<tokio::sync::Mutex<std::io::BufWriter<std::fs::File>>>> =
        match &cfg.trace_path {
            Some(p) => {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                let f = std::fs::File::create(p)
                    .with_context(|| format!("create trace file {}", p.display()))?;
                info!(trace = %p.display(), "writing per-query trace JSONL");
                Some(std::sync::Arc::new(tokio::sync::Mutex::new(
                    std::io::BufWriter::new(f),
                )))
            }
            None => None,
        };

    let judge = judge.map(std::sync::Arc::new);
    let data_dir = cfg.data_dir.clone();
    let benchmark = cfg.benchmark;
    let k = cfg.k;
    let retrieval = cfg.retrieval.clone();
    let lexical_judge = cfg.lexical_judge;
    let hybrid_stack = hybrid_stack.map(|(e, c)| (e, c));
    let rerank_claude = rerank_claude;

    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let run_t0 = Instant::now();
    let correct_so_far = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let answer_skipped = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let consecutive_fails = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let abort_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let last_answer_err: std::sync::Arc<tokio::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let progress_ui = std::sync::Arc::new(tokio::sync::Mutex::new(ui));

    let mut join_set = tokio::task::JoinSet::new();
    for q in queries_to_run {
        if abort_flag.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let permit = sem
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore not closed");
        if abort_flag.load(std::sync::atomic::Ordering::Relaxed) {
            drop(permit);
            break;
        }

        let judge = judge.clone();
        let hybrid_stack = hybrid_stack.clone();
        let rerank_claude = rerank_claude.clone();
        let data_dir = data_dir.clone();
        let retrieval = retrieval.clone();
        let trace_writer = trace_writer.clone();
        let correct_so_far = correct_so_far.clone();
        let completed = completed.clone();
        let answer_skipped = answer_skipped.clone();
        let consecutive_fails = consecutive_fails.clone();
        let abort_flag = abort_flag.clone();
        let last_answer_err = last_answer_err.clone();
        let progress_ui = progress_ui.clone();
        let run_t0 = run_t0;

        join_set.spawn(async move {
            let _permit = permit;
            if abort_flag.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok::<Option<QueryResult>, anyhow::Error>(None);
            }

            {
                let done = completed.load(std::sync::atomic::Ordering::Relaxed);
                let mut ui = progress_ui.lock().await;
                ui.status(format!(
                    "{} {:5.1}%  {}/{total_queries}  …  id={}",
                    pct_bar(done, total_queries, 20),
                    pct(done, total_queries),
                    done + 1,
                    q.id,
                ));
            }

            let outcome = eval_one_query(
                &q,
                benchmark,
                &data_dir,
                k,
                &retrieval,
                lexical_judge,
                judge.as_ref(),
                hybrid_stack.as_ref(),
                rerank_claude.as_ref(),
                trace_writer.as_ref(),
            )
            .await;

            match outcome {
                Ok(qr) => {
                    consecutive_fails.store(0, std::sync::atomic::Ordering::Relaxed);
                    let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    let correct_n = if qr.correct {
                        correct_so_far.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
                    } else {
                        correct_so_far.load(std::sync::atomic::Ordering::Relaxed)
                    };
                    let acc_pct = (correct_n as f64 / done as f64) * 100.0;
                    let secs = run_t0.elapsed().as_secs_f64().max(0.001);
                    let remaining = total_queries.saturating_sub(done);
                    let eta_s = ((secs / done as f64) * remaining as f64) as u64;
                    let mark = if qr.correct { "ok" } else { "miss" };
                    info!(
                        progress = format!("{done}/{total_queries}"),
                        id = %qr.id,
                        correct = qr.correct,
                        acc_pct = format!("{acc_pct:.1}"),
                        hits = qr.hits_count,
                        retrieval_ms = (qr.retrieval_us as f64) / 1000.0,
                        e2e_ms = (qr.end_to_end_us as f64) / 1000.0,
                        eta_s = eta_s,
                        "query complete"
                    );
                    {
                        let mut ui = progress_ui.lock().await;
                        ui.status(format!(
                            "{} {:5.1}%  {done}/{total_queries}  {mark}  acc={acc_pct:.1}%  eta={}  e2e={:.1}s  id={}",
                            pct_bar(done, total_queries, 20),
                            pct(done, total_queries),
                            fmt_eta(eta_s),
                            (qr.end_to_end_us as f64) / 1_000_000.0,
                            qr.id,
                        ));
                    }
                    Ok(Some(qr))
                }
                Err(QueryEvalError::AnswerFailed(msg)) => {
                    answer_skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    {
                        let mut last = last_answer_err.lock().await;
                        *last = Some(msg.clone());
                    }
                    let streak =
                        consecutive_fails.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    {
                        let mut ui = progress_ui.lock().await;
                        ui.note(format!("answer LLM failed for {}: {msg}", q.id));
                    }
                    warn!(query_id = %q.id, error = %msg, streak, "answer LLM failed, skipping query");
                    if streak >= CONSECUTIVE_ANSWER_FAIL_ABORT {
                        abort_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        let mut ui = progress_ui.lock().await;
                        ui.note(format!(
                            "aborting after {streak} consecutive answer LLM failures"
                        ));
                    }
                    Ok(None)
                }
                Err(QueryEvalError::Other(e)) => Err(e),
            }
        });
    }

    let mut results: Vec<QueryResult> = Vec::with_capacity(total_queries);
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(Ok(Some(qr))) => results.push(qr),
            Ok(Ok(None)) => {}
            Ok(Err(e)) => {
                join_set.abort_all();
                return Err(e);
            }
            Err(e) => {
                join_set.abort_all();
                return Err(anyhow::anyhow!("query task join error: {e}"));
            }
        }
    }
    // Stable order by query id for reproducible scorecards.
    results.sort_by(|a, b| a.id.cmp(&b.id));

    let mut ui = progress_ui.lock().await;
    let answer_skipped_n = answer_skipped.load(std::sync::atomic::Ordering::Relaxed);
    if results.is_empty() && !cfg.lexical_judge && total_queries > 0 {
        let hint = last_answer_err
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "unknown LLM error".into());
        anyhow::bail!(
            "all {total_queries} queries skipped after answer LLM failures \
             ({answer_skipped_n} skips). Last error: {hint}. \
             Fix auth for the detected CLI, or pin a working one: \
             export MEMLAYER_LLM_PROVIDER=opencode  # or gemini / claude"
        );
    }

    let aborted = abort_flag.load(std::sync::atomic::Ordering::Relaxed);
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
        cfg.retrieval.rerank,
    );

    // Write JSON + Markdown.
    let json = serde_json::to_string_pretty(&report).context("serialize report")?;
    let json_path = cfg.output_path.with_extension("json");
    std::fs::write(&json_path, &json).context("write JSON report")?;
    std::fs::write(&cfg.output_path, report.to_markdown()).context("write Markdown report")?;
    info!(md = %cfg.output_path.display(), json = %json_path.display(), "report written");
    ui.note(format!(
        "done  accuracy={:.1}%  ({}/{})  recall={:.3}  report={}",
        report.accuracy_pct,
        report.correct,
        report.total_queries,
        report.recall_at_k,
        cfg.output_path.display()
    ));

    if aborted && !cfg.lexical_judge {
        let hint = last_answer_err
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "unknown LLM error".into());
        anyhow::bail!(
            "aborted after {CONSECUTIVE_ANSWER_FAIL_ABORT} consecutive answer LLM failures \
             ({answer_skipped_n} skips, {} completed). Partial report written to {}. \
             Last error: {hint}",
            report.total_queries,
            cfg.output_path.display()
        );
    }

    Ok(report)
}

enum QueryEvalError {
    AnswerFailed(String),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for QueryEvalError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
    }
}

#[allow(clippy::too_many_arguments)]
async fn eval_one_query(
    q: &EvalQuery,
    benchmark: BenchmarkKind,
    data_dir: &std::path::Path,
    k: i32,
    retrieval: &RetrievalConfig,
    lexical_judge: bool,
    judge: Option<&std::sync::Arc<JudgeClient>>,
    hybrid_stack: Option<&(
        std::sync::Arc<dyn memlayer_embed::Embedder>,
        std::sync::Arc<memlayer_embed::cache::EmbeddingCache>,
    )>,
    rerank_claude: Option<&std::sync::Arc<dyn memlayer_extract::claude_cli::ClaudeClient>>,
    trace_writer: Option<
        &std::sync::Arc<tokio::sync::Mutex<std::io::BufWriter<std::fs::File>>>,
    >,
) -> Result<QueryResult, QueryEvalError> {
    let t_start = Instant::now();
    let project = infer_project(benchmark, &q.id);
    let evidence_window = evidence_window_for_question(retrieval.evidence_window, &q.question);

    let mut rerank_skipped = false;
    let (hits, retrieval_us, rerank_us) = match retrieval.mode {
        RetrievalMode::Bm25 => {
            let ret = retrieve(data_dir, &project, &q.question, k)
                .with_context(|| format!("retrieve for query '{}'", q.id))?;
            (ret.hits, ret.latency.as_micros() as u64, None)
        }
        RetrievalMode::Hybrid | RetrievalMode::HybridRerank => {
            let (embedder, cache) = hybrid_stack.expect(
                "hybrid_stack initialised when mode is Hybrid or HybridRerank",
            );
            let retrieve_k = if retrieval.rerank { k * 3 } else { k };
            let facts_db_path = facts_db_path_for(benchmark, data_dir);
            let mut rerank_ambiguous = true;
            let (raw_hits, raw_retrieval_us) = if facts_db_path.exists() {
                let ret = crate::retrieve_facts::retrieve_facts(
                    data_dir,
                    &facts_db_path,
                    &project,
                    &q.question,
                    retrieve_k,
                    evidence_window,
                    embedder.clone(),
                    cache.clone(),
                    retrieval.decay_lambda,
                )
                .await
                .with_context(|| format!("facts retrieve for query '{}'", q.id))?;
                rerank_ambiguous = ret
                    .top2_delta
                    .map(|d| d < RERANK_AMBIGUITY_THRESHOLD)
                    .unwrap_or(true);
                if ret.hits.is_empty() {
                    info!(
                        query_id = %q.id,
                        project = %project,
                        "facts retrieve returned 0 hits — falling back to hybrid over raw observations"
                    );
                    let fallback = crate::retrieve_hybrid::retrieve_hybrid(
                        data_dir,
                        &project,
                        &q.question,
                        retrieve_k,
                        embedder.clone(),
                        cache.clone(),
                    )
                    .await
                    .with_context(|| format!("hybrid fallback for query '{}'", q.id))?;
                    rerank_ambiguous = true;
                    (
                        fallback.hits,
                        (ret.latency + fallback.latency).as_micros() as u64,
                    )
                } else {
                    (ret.hits, ret.latency.as_micros() as u64)
                }
            } else {
                let ret = crate::retrieve_hybrid::retrieve_hybrid(
                    data_dir,
                    &project,
                    &q.question,
                    retrieve_k,
                    embedder.clone(),
                    cache.clone(),
                )
                .await
                .with_context(|| format!("hybrid retrieve for query '{}'", q.id))?;
                (ret.hits, ret.latency.as_micros() as u64)
            };

            if let Some(claude) = rerank_claude {
                if rerank_ambiguous {
                    let (reranked, rerank_dur) = crate::rerank::rerank(
                        claude.clone(),
                        &raw_hits,
                        &q.question,
                        k as usize,
                    )
                    .await
                    .with_context(|| format!("rerank for query '{}'", q.id))?;
                    (reranked, raw_retrieval_us, Some(rerank_dur.as_micros() as u64))
                } else {
                    let trimmed: Vec<String> =
                        raw_hits.into_iter().take(k as usize).collect();
                    info!(
                        query_id = %q.id,
                        "skipping rerank (top-2 score delta >= {RERANK_AMBIGUITY_THRESHOLD})"
                    );
                    rerank_skipped = true;
                    (trimmed, raw_retrieval_us, None)
                }
            } else {
                let trimmed: Vec<String> =
                    raw_hits.into_iter().take(k as usize).collect();
                (trimmed, raw_retrieval_us, None)
            }
        }
    };

    let (system, user_msg, prompt_tokens) = build_answer_prompt(&hits, &q.question);
    let gold_sub_rank = gold_substring_rank(&q.gold_answer, &hits);
    let primary_rank = primary_retrieval_rank(q, &hits);

    let (model_answer, judge_prompt, correct) = if lexical_judge {
        let joined = hits.join("\n");
        let model_answer = if joined.is_empty() {
            String::new()
        } else {
            joined
        };
        // Lexical path: prefer evidence match when present, else gold substring.
        let correct = primary_rank.is_some();
        (model_answer, String::new(), correct)
    } else {
        let judge = judge.expect("JudgeClient when lexical_judge is false");
        let model_answer = match judge.answer(&system, &user_msg).await {
            Ok(a) => a,
            Err(e) => {
                return Err(QueryEvalError::AnswerFailed(format!("{e:#}")));
            }
        };
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
        (model_answer, judge_prompt, correct)
    };

    let stale_served = match &q.anti_answer {
        Some(anti) if !anti.trim().is_empty() => {
            let anti_hit_rank = gold_substring_rank(anti, &hits);
            let anti_l = anti.trim().to_ascii_lowercase();
            let gold_l = q.gold_answer.trim().to_ascii_lowercase();
            let anti_in_answer = model_answer.to_ascii_lowercase().contains(&anti_l);
            let gold_in_answer =
                !gold_l.is_empty() && model_answer.to_ascii_lowercase().contains(&gold_l);
            match (anti_hit_rank, gold_sub_rank) {
                (Some(_), None) => true,
                (Some(a), Some(g)) => a < g,
                (None, _) => anti_in_answer && !gold_in_answer,
            }
        }
        _ => false,
    };

    let end_to_end_us = t_start.elapsed().as_micros() as u64;

    if let Some(w) = trace_writer {
        use std::io::Write;
        let entry = serde_json::json!({
            "id": q.id,
            "project": project,
            "question": q.question,
            "gold_answer": q.gold_answer,
            "evidence": q.evidence,
            "mode": match retrieval.mode {
                RetrievalMode::Bm25 => "bm25",
                RetrievalMode::Hybrid => "hybrid",
                RetrievalMode::HybridRerank => "hybrid-rerank",
            },
            "k": k,
            "evidence_window": evidence_window,
            "rerank_enabled": retrieval.rerank,
            "rerank_skipped": rerank_skipped,
            "retrieval_us": retrieval_us,
            "rerank_us": rerank_us,
            "hits_count": hits.len(),
            "hits": hits,
            "primary_rank": primary_rank,
            "gold_substring_rank": gold_sub_rank,
            "answer_prompt_system": system,
            "answer_prompt_user": user_msg,
            "answer_prompt_tokens": prompt_tokens,
            "model_answer": model_answer,
            "judge_prompt": judge_prompt,
            "correct": correct,
            "end_to_end_us": end_to_end_us,
        });
        let mut guard = w.lock().await;
        if let Err(e) = writeln!(guard, "{}", entry) {
            warn!(query_id = %q.id, error = %e, "failed to write trace entry");
        }
    }

    Ok(QueryResult {
        id: q.id.clone(),
        question: q.question.clone(),
        gold_answer: q.gold_answer.clone(),
        model_answer,
        correct,
        retrieval_us,
        end_to_end_us,
        prompt_tokens,
        rerank_us,
        category: q.category.clone(),
        gold_rank: primary_rank,
        gold_substring_rank: gold_sub_rank,
        hits_count: hits.len(),
        stale_served,
        rerank_skipped,
    })
}

/// Widen the evidence window for temporal questions (eval-only).
pub fn evidence_window_for_question(base: u8, question: &str) -> u8 {
    let q = question.to_ascii_lowercase();
    if q.contains("when") || q.contains("before") || q.contains("after") || q.contains("date") {
        base.max(4)
    } else {
        base
    }
}

/// 0-based rank of the first hit containing any evidence turn id.
pub fn evidence_rank(evidence: &[String], hits: &[String]) -> Option<usize> {
    if evidence.is_empty() {
        return None;
    }
    let needles: Vec<String> = evidence
        .iter()
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    if needles.is_empty() {
        return None;
    }
    hits.iter().position(|h| {
        let hl = h.to_ascii_lowercase();
        needles.iter().any(|n| hl.contains(n))
    })
}

/// 0-based rank of the first hit containing the gold answer substring.
fn gold_substring_rank(gold: &str, hits: &[String]) -> Option<usize> {
    let g = gold.trim().to_ascii_lowercase();
    if g.is_empty() {
        return None;
    }
    hits.iter().position(|h| h.to_ascii_lowercase().contains(&g))
}

/// Primary retrieval rank: evidence ids when present, else gold substring.
pub fn primary_retrieval_rank(q: &EvalQuery, hits: &[String]) -> Option<usize> {
    if !q.evidence.is_empty() {
        evidence_rank(&q.evidence, hits)
    } else {
        gold_substring_rank(&q.gold_answer, hits)
    }
}

#[cfg(test)]
fn gold_in_hits(gold: &str, hits: &[String]) -> bool {
    gold_substring_rank(gold, hits).is_some()
}

fn infer_project(kind: BenchmarkKind, query_id: &str) -> String {
    match kind {
        BenchmarkKind::Locomo => {
            // id format: "{conv_id}-{qa_id}"
            let conv_id = query_id.rsplit_once('-').map(|x| x.0).unwrap_or(query_id);
            format!("locomo-{conv_id}")
        }
        BenchmarkKind::Longmemeval => {
            // id is the question_id directly; project is lme-{question_id}
            format!("lme-{query_id}")
        }
        BenchmarkKind::Beam1m  => "beam-1m".to_string(),
        BenchmarkKind::Beam10m => "beam-10m".to_string(),
        BenchmarkKind::Staleness => {
            let tid = query_id.strip_suffix("-q").unwrap_or(query_id);
            format!("staleness-{tid}")
        }
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
        BenchmarkKind::Staleness => "staleness",
    };
    data_dir.join(name).join("facts.db")
}

fn build_report(
    kind: BenchmarkKind,
    results: Vec<QueryResult>,
    mode_tag: &str,
    evidence_window: u8,
    rerank_enabled: bool,
) -> RunReport {
    let total = results.len();
    let correct = results.iter().filter(|r| r.correct).count();
    let accuracy_pct = if total == 0 { 0.0 } else { correct as f64 / total as f64 * 100.0 };

    let total_prompt_tokens: usize = results.iter().map(|r| r.prompt_tokens).sum();
    let mean_tokens = if total == 0 {
        0.0
    } else {
        total_prompt_tokens as f64 / total as f64
    };

    let recall_hits = results.iter().filter(|r| r.gold_rank.is_some()).count();
    let recall_at_k = if total == 0 {
        0.0
    } else {
        recall_hits as f64 / total as f64
    };
    let mrr = if total == 0 {
        0.0
    } else {
        results
            .iter()
            .map(|r| r.gold_rank.map(|rank| 1.0 / (rank as f64 + 1.0)).unwrap_or(0.0))
            .sum::<f64>()
            / total as f64
    };

    let gold_sub_hits = results
        .iter()
        .filter(|r| r.gold_substring_rank.is_some())
        .count();
    let gold_substring_recall = if total == 0 {
        None
    } else {
        Some(gold_sub_hits as f64 / total as f64)
    };

    let rerank_skipped_pct = if rerank_enabled && total > 0 {
        let skipped = results.iter().filter(|r| r.rerank_skipped).count();
        Some(skipped as f64 / total as f64 * 100.0)
    } else {
        None
    };

    let mut by_category: std::collections::BTreeMap<String, CategoryStats> =
        std::collections::BTreeMap::new();
    for r in &results {
        let Some(cat) = r.category.as_ref() else { continue };
        let entry = by_category.entry(cat.clone()).or_default();
        entry.total += 1;
        if r.correct {
            entry.correct += 1;
        }
    }
    for stats in by_category.values_mut() {
        stats.accuracy_pct = if stats.total == 0 {
            0.0
        } else {
            stats.correct as f64 / stats.total as f64 * 100.0
        };
    }

    let stale_count = results.iter().filter(|r| r.stale_served).count();
    let has_anti = results.iter().any(|r| {
        r.category.as_deref() == Some("staleness")
    });
    let superseded_served_pct = if has_anti {
        Some(if total == 0 {
            0.0
        } else {
            stale_count as f64 / total as f64 * 100.0
        })
    } else {
        None
    };

    let retrieval_p50 = percentile_ms(&mut results.iter().map(|r| r.retrieval_us).collect::<Vec<_>>(), 50);
    let retrieval_p95 = percentile_ms(&mut results.iter().map(|r| r.retrieval_us).collect::<Vec<_>>(), 95);
    let e2e_p50 = percentile_ms(&mut results.iter().map(|r| r.end_to_end_us).collect::<Vec<_>>(), 50);
    let e2e_p95 = percentile_ms(&mut results.iter().map(|r| r.end_to_end_us).collect::<Vec<_>>(), 95);

    let rerank_samples: Vec<u64> = results.iter().filter_map(|r| r.rerank_us).collect();
    let (rerank_p50_ms, rerank_p95_ms) = if rerank_samples.is_empty() {
        (None, None)
    } else {
        (
            Some(percentile_ms(&mut rerank_samples.clone(), 50)),
            Some(percentile_ms(&mut rerank_samples.clone(), 95)),
        )
    };

    RunReport {
        benchmark: format!("{kind:?} ({mode_tag}, w={evidence_window})"),
        total_queries: total,
        correct,
        accuracy_pct,
        mean_prompt_tokens: mean_tokens,
        total_prompt_tokens,
        recall_at_k,
        mrr,
        gold_substring_recall,
        rerank_skipped_pct,
        by_category,
        superseded_served_pct,
        retrieval_p50_ms: retrieval_p50,
        retrieval_p95_ms: retrieval_p95,
        end_to_end_p50_ms: e2e_p50,
        end_to_end_p95_ms: e2e_p95,
        rerank_p50_ms,
        rerank_p95_ms,
        query_results: results,
    }
}

fn percentile_ms(values: &mut [u64], p: usize) -> f64 {
    if values.is_empty() { return 0.0; }
    values.sort_unstable();
    let idx = ((p as f64 / 100.0) * (values.len() - 1) as f64).round() as usize;
    values[idx.min(values.len() - 1)] as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporal_questions_widen_evidence_window() {
        assert_eq!(evidence_window_for_question(2, "When did Caroline go?"), 4);
        assert_eq!(evidence_window_for_question(2, "What is her job?"), 2);
        assert_eq!(evidence_window_for_question(5, "the date of the trip"), 5);
    }

    #[test]
    fn gold_match_is_case_insensitive_substring() {
        let hits = vec!["Caroline went to the Park on Saturday.".into()];
        assert_eq!(gold_substring_rank("the park", &hits), Some(0));
        assert!(gold_in_hits("the park", &hits));
        assert!(!gold_in_hits("the zoo", &hits));
        assert_eq!(gold_substring_rank("the zoo", &hits), None);
    }

    #[test]
    fn evidence_rank_matches_dia_id_in_hit_title() {
        let hits = vec![
            "[9:55 am] Melanie (D1:1)\nMelanie: hello".into(),
            "[10:00 am] Caroline (D1:3)\nCaroline: I went to the LGBTQ support group".into(),
            "[10:05 am] Melanie (D1:4)\nMelanie: cool".into(),
        ];
        assert_eq!(evidence_rank(&["D1:3".into()], &hits), Some(1));
        assert_eq!(evidence_rank(&["D1:9".into()], &hits), None);
        // Gold substring may miss while evidence hits.
        assert_eq!(gold_substring_rank("LGBTQ support group", &hits), Some(1));
        assert_eq!(gold_substring_rank("the park", &hits), None);

        let q = EvalQuery {
            id: "c-q0".into(),
            question: "Where did Caroline go?".into(),
            gold_answer: "LGBTQ support group".into(),
            judge_context: None,
            category: Some("1".into()),
            anti_answer: None,
            evidence: vec!["D1:3".into()],
        };
        assert_eq!(primary_retrieval_rank(&q, &hits), Some(1));
        // Primary prefers evidence even when gold also matches a different hit.
        let hits2 = vec![
            "noise mentioning LGBTQ support group without dia_id".into(),
            "[10:00 am] Caroline (D1:3)\nCaroline: I went to the LGBTQ support group".into(),
        ];
        assert_eq!(primary_retrieval_rank(&q, &hits2), Some(1));
    }

    #[test]
    fn primary_rank_falls_back_to_gold_without_evidence() {
        let hits = vec!["Caroline went to the park.".into()];
        let q = EvalQuery {
            id: "c-q0".into(),
            question: "Where?".into(),
            gold_answer: "the park".into(),
            judge_context: None,
            category: None,
            anti_answer: None,
            evidence: Vec::new(),
        };
        assert_eq!(primary_retrieval_rank(&q, &hits), Some(0));
    }
}

