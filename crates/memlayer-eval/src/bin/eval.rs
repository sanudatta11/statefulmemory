// Generated with AI Coding Rules Hub
//! `eval` binary — CLI entry point for memlayer benchmark runs.
//!
//! Usage:
//!   eval run       --benchmark <B> [--k 10] [--limit N] [--out report.md]
//!   eval prepare   --benchmark <B>    # ingest-only (no LLM calls, for BEAM scale)
//!   eval summarize --reports <dir>    # merge multiple report JSONs into summary.md

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

use memlayer_eval::{
    config::{RetrievalConfig, RetrievalMode},
    datasets::{beam::BeamScale, locomo, longmemeval},
    runner::{BenchmarkKind, RunConfig},
};

#[derive(Parser)]
#[command(name = "eval", about = "memlayer benchmark runner")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a benchmark end-to-end: ingest → retrieve → answer → judge → report.
    Run {
        #[arg(long, value_enum)]
        benchmark: BenchmarkKind,

        /// Top-k memories to retrieve per query.
        #[arg(long, default_value_t = 10)]
        k: i32,

        /// Stop after N queries (smoke-test mode; cheap).
        #[arg(long)]
        limit: Option<usize>,

        /// Directory containing downloaded benchmark data.
        #[arg(long, default_value = "data")]
        data_dir: PathBuf,

        /// Path to write the Markdown report.
        #[arg(long, default_value = "reports/report.md")]
        out: PathBuf,

        /// Skip ingestion (data already prepared by `eval prepare`).
        #[arg(long, default_value_t = false)]
        skip_ingest: bool,

        /// Retrieval strategy.
        #[arg(long, value_enum, default_value_t = RetrievalMode::Bm25)]
        mode: RetrievalMode,

        /// Evidence window (±N raw observations around each hit).
        #[arg(long, default_value_t = 2)]
        evidence_window: u8,

        /// Number of facts.db shards. 1 = single DB (LoCoMo / LongMemEval
        /// default). N>1 routes facts to <data_dir>/<benchmark>-vec/shard-NN.db
        /// via FNV-based shard_for(obs_id) — used by BEAM-1M / 10M.
        #[arg(long, default_value_t = 1)]
        shards: usize,
    },

    /// Extract facts from a benchmark dataset (P2: not yet implemented).
    Extract {
        #[arg(long, value_enum)]
        benchmark: BenchmarkKind,

        #[arg(long, default_value = "data")]
        data_dir: PathBuf,

        /// Optional: limit to first N queries (smoke-test mode).
        #[arg(long)]
        limit: Option<usize>,

        /// Optional: stop after extracting N projects (cheap smoke-test).
        #[arg(long)]
        project_limit: Option<usize>,

        /// Optional: extract only this exact project (e.g.
        /// `--project locomo-conv-26`). Wins over `--project-limit`.
        /// Use when you need to target the conversations the smoke
        /// queries actually hit.
        #[arg(long)]
        project: Option<String>,

        /// Use Haiku for entity extraction on facts whose salience >=
        /// 0.85 (P5 spec-task-27c). Costs more LLM calls (~15% of
        /// facts) but captures richer entity sets on high-quality
        /// facts. Default off — smoke path stays cheap.
        #[arg(long, default_value_t = false)]
        haiku_entities: bool,

        /// Emit one session-summary fact per source_session via a
        /// single Haiku call (P5 spec-task-27d). Adds ~$0.50/LoCoMo
        /// run. Default off — turn on for full-benchmark runs.
        #[arg(long, default_value_t = false)]
        session_summaries: bool,
    },

    /// Ingest-only (no LLM calls). Use this to pre-populate BEAM at scale.
    Prepare {
        #[arg(long, value_enum)]
        benchmark: BenchmarkKind,

        #[arg(long, default_value = "data")]
        data_dir: PathBuf,

        /// Number of needle queries / memories to embed. Distractors fill the rest.
        #[arg(long, default_value_t = 1000)]
        num_queries: usize,
    },

    /// Merge multiple report JSON files into a single summary Markdown table.
    Summarize {
        #[arg(long, default_value = "reports")]
        reports: PathBuf,

        #[arg(long, default_value = "reports/summary.md")]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { benchmark, k, limit, data_dir, out, skip_ingest, mode, evidence_window, shards } => {
            std::fs::create_dir_all(out.parent().unwrap_or(std::path::Path::new("."))).ok();
            let (memories, queries) = load_dataset(benchmark, &data_dir, limit.unwrap_or(usize::MAX))?;

            let retrieval = RetrievalConfig {
                mode,
                k,
                evidence_window,
                rerank: matches!(mode, RetrievalMode::HybridRerank),
                decay_lambda: memlayer_eval::scoring::DEFAULT_DECAY_LAMBDA,
            };

            // Auto-derive trace file path from --out: foo.md -> foo.trace.jsonl.
            // Captures full per-query trace (hits, prompts, LLM I/O, verdicts).
            let trace_path = {
                let stem = out.file_stem().map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "report".to_string());
                let parent = out.parent().unwrap_or(std::path::Path::new("."));
                Some(parent.join(format!("{stem}.trace.jsonl")))
            };

            let cfg = RunConfig {
                benchmark,
                data_dir: data_dir.clone(),
                k,
                limit,
                concurrency: 4,
                output_path: out,
                skip_ingest,
                retrieval,
                trace_path,
                shards,
            };

            let report = memlayer_eval::runner::run(&cfg, memories, queries).await?;

            println!(
                "\n=== {} ===\nAccuracy: {:.1}% ({}/{})\nMean tokens: {:.0}\nRetrieval p50: {:.2}ms\nEnd-to-end p50: {:.2}ms\n",
                report.benchmark,
                report.accuracy_pct,
                report.correct,
                report.total_queries,
                report.mean_prompt_tokens,
                report.retrieval_p50_ms,
                report.end_to_end_p50_ms,
            );
        }

        Commands::Prepare { benchmark, data_dir, num_queries } => {
            match benchmark {
                BenchmarkKind::Beam1m | BenchmarkKind::Beam10m => {
                    let scale = if benchmark == BenchmarkKind::Beam1m {
                        BeamScale::M1
                    } else {
                        BeamScale::M10
                    };
                    println!(
                        "Preparing BEAM {:?}: {} total observations, {} needles",
                        scale, scale.total_observations(), num_queries
                    );
                    let iter = memlayer_eval::datasets::beam::memory_iter(scale, num_queries);
                    let total = memlayer_eval::ingest::ingest_stream(&data_dir, iter, 50_000).await?;
                    println!("Prepared {total} observations.");
                }
                _ => bail!("prepare is only needed for beam-1m and beam-10m"),
            }
        }

        Commands::Extract { benchmark, data_dir, limit, project_limit, project, haiku_entities, session_summaries } => {
            run_extract(benchmark, &data_dir, limit, project_limit, project, haiku_entities, session_summaries).await?;
        }

        Commands::Summarize { reports, out } => {
            summarize_reports(&reports, &out)?;
        }
    }

    Ok(())
}

fn load_dataset(
    kind: BenchmarkKind,
    data_dir: &PathBuf,
    limit: usize,
) -> Result<(Vec<memlayer_eval::datasets::EvalMemory>, Vec<memlayer_eval::datasets::EvalQuery>)> {
    match kind {
        BenchmarkKind::Locomo => {
            let (m, q) = locomo::load(data_dir)?;
            Ok((m, q.into_iter().take(limit).collect()))
        }
        BenchmarkKind::Longmemeval => {
            let (m, q) = longmemeval::load(data_dir)?;
            Ok((m, q.into_iter().take(limit).collect()))
        }
        BenchmarkKind::Beam1m => {
            let (m, q) = memlayer_eval::datasets::beam::generate_all(BeamScale::M1, limit.min(1000))?;
            Ok((m, q))
        }
        BenchmarkKind::Beam10m => {
            bail!("Use `eval prepare --benchmark beam-10m` first, then `eval run --skip-ingest`.")
        }
    }
}

/// Active `eval extract` subcommand (spec-task-20).
///
/// 1. Load the dataset (limit optional).
/// 2. Ensure the eval-side observations are ingested for each project so
///    extract_project has something to read.
/// 3. Open the facts.db at the conventional `<data_dir>/<benchmark>/facts.db`.
/// 4. Build an ExtractPipeline (real Claude CLI client + BGE embedder +
///    heuristic entity extractor by default).
/// 5. For each unique project name in the dataset, call
///    `extract_pipeline.extract_project(project, &facts_db_path)`.
/// 6. Print rolled-up stats.
async fn run_extract(
    benchmark: BenchmarkKind,
    data_dir: &std::path::Path,
    limit: Option<usize>,
    project_limit: Option<usize>,
    project_filter: Option<String>,
    haiku_entities: bool,
    session_summaries: bool,
) -> Result<()> {
    use std::sync::Arc;
    use memlayer_embed::BgeSmallEmbedder;
    use memlayer_eval::extract_pipeline::ExtractPipeline;
    use memlayer_extract::claude_cli::ClaudeCliClient;
    use memlayer_extract::entities::{HaikuEntityExtractor, HeuristicEntityExtractor};

    let lim = limit.unwrap_or(usize::MAX);
    let (memories, _queries) = load_dataset(benchmark, &data_dir.to_path_buf(), lim)?;

    if memories.is_empty() {
        println!("No memories loaded for {:?}; nothing to extract.", benchmark);
        return Ok(());
    }

    // Ensure observations exist for each project (idempotent — skips if
    // already ingested with same sync_ids).
    println!("Ingesting {} observations into storage projects...", memories.len());
    memlayer_eval::ingest::ingest_memories(data_dir, &memories, 500).await?;

    let facts_db_path = memlayer_eval::runner::facts_db_path_for(benchmark, data_dir);
    println!("Facts DB: {}", facts_db_path.display());

    let claude = Arc::new(ClaudeCliClient::new());
    let embedder = Arc::new(
        BgeSmallEmbedder::try_new()
            .map_err(|e| anyhow::anyhow!("load BGE-small embedder: {e}"))?,
    );
    let entity_extractor = Arc::new(HeuristicEntityExtractor::new());

    let mut pipeline = ExtractPipeline::new(data_dir.to_path_buf(), claude.clone(), embedder)
        .with_entity_extractor(entity_extractor)
        .with_session_summaries(session_summaries);
    if haiku_entities {
        let haiku_ext = Arc::new(HaikuEntityExtractor::new(claude));
        pipeline = pipeline.with_haiku_entity_extractor(haiku_ext);
    }

    // Distinct project names in input order, capped by --project-limit.
    // When project_limit is set, prefer the smallest projects (fewest memories →
    // fewest extraction windows → fastest smoke-test feedback).
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in &memories {
        *counts.entry(m.project.clone()).or_insert(0) += 1;
    }
    let mut seen = std::collections::HashSet::new();
    let all_projects: Vec<String> = memories
        .iter()
        .filter_map(|m| if seen.insert(m.project.clone()) { Some(m.project.clone()) } else { None })
        .collect();
    let projects: Vec<String> = match (project_filter.as_ref(), project_limit) {
        (Some(name), _) => {
            // Explicit --project wins. If the name isn't in the dataset,
            // bail with a clear error rather than silently extracting nothing.
            if !all_projects.iter().any(|p| p == name) {
                anyhow::bail!(
                    "--project '{name}' not found in {benchmark:?} dataset. Available projects: {}",
                    all_projects.join(", ")
                );
            }
            vec![name.clone()]
        }
        (None, Some(n)) => {
            let mut sorted = all_projects;
            sorted.sort_by_key(|p| counts.get(p).copied().unwrap_or(0));
            sorted.into_iter().take(n).collect()
        }
        (None, None) => all_projects,
    };

    let total_projects = projects.len();
    println!("Extracting facts for {total_projects} project(s)...");

    let mut total_facts = 0usize;
    let mut total_entities = 0usize;
    let mut total_failed = 0usize;
    let t0 = std::time::Instant::now();

    for (i, project) in projects.iter().enumerate() {
        match pipeline.extract_project(project, &facts_db_path).await {
            Ok(stats) => {
                println!(
                    "  [{:>3}/{}] {project}: facts={} entities={} cached={} failed={} ({:?})",
                    i + 1,
                    total_projects,
                    stats.facts_written,
                    stats.entities_written,
                    stats.windows_cached,
                    stats.windows_failed,
                    std::time::Duration::from_millis(stats.elapsed_ms as u64),
                );
                total_facts += stats.facts_written;
                total_entities += stats.entities_written;
                total_failed += stats.windows_failed;
            }
            Err(e) => {
                eprintln!("  [{:>3}/{}] {project}: ERROR {e:#}", i + 1, total_projects);
            }
        }
    }

    println!(
        "\nExtraction complete in {:?}. facts={} entities={} failed_windows={}",
        t0.elapsed(),
        total_facts,
        total_entities,
        total_failed,
    );
    let bench_cli = clap::ValueEnum::to_possible_value(&benchmark)
        .map(|v| v.get_name().to_string())
        .unwrap_or_else(|| format!("{benchmark:?}").to_lowercase());
    println!("Now run: cargo run --release --bin eval -- run --benchmark {bench_cli} --mode hybrid --out reports/p2-smoke.md");

    Ok(())
}

fn summarize_reports(reports_dir: &PathBuf, out: &PathBuf) -> Result<()> {
    use memlayer_eval::runner::RunReport;

    let mut rows: Vec<RunReport> = Vec::new();
    for entry in std::fs::read_dir(reports_dir)? {
        let entry = entry?;
        if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
            let raw = std::fs::read_to_string(entry.path())?;
            let r: RunReport = serde_json::from_str(&raw)?;
            rows.push(r);
        }
    }

    rows.sort_by(|a, b| a.benchmark.cmp(&b.benchmark));

    let mut md = String::from(
        "# memlayer Benchmark Summary\n\n\
         | Benchmark | Accuracy | Tokens | Ret p50 | E2E p50 | Mem0 Old | Mem0 New |\n\
         |---|---|---|---|---|---|---|\n"
    );
    let mem0 = [
        ("locomo",      "71.4%", "91.6%"),
        ("longmemeval", "67.8%", "94.8%"),
        ("beam-1m",     "—",     "64.1%"),
        ("beam-10m",    "—",     "48.6%"),
    ];

    for r in &rows {
        let key = r.benchmark.to_lowercase().replace("benchmarkkind::", "");
        let (old, new) = mem0.iter()
            .find(|(k, _, _)| key.contains(k))
            .map(|(_, o, n)| (*o, *n))
            .unwrap_or(("—", "—"));
        md.push_str(&format!(
            "| {} | {:.1}% ({}/{}) | {:.0} | {:.2}ms | {:.2}ms | {} | {} |\n",
            r.benchmark,
            r.accuracy_pct, r.correct, r.total_queries,
            r.mean_prompt_tokens,
            r.retrieval_p50_ms,
            r.end_to_end_p50_ms,
            old, new,
        ));
    }

    std::fs::create_dir_all(out.parent().unwrap_or(std::path::Path::new("."))).ok();
    std::fs::write(out, &md)?;
    println!("Summary written to {}", out.display());
    Ok(())
}
