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
        Commands::Run { benchmark, k, limit, data_dir, out, skip_ingest } => {
            std::fs::create_dir_all(out.parent().unwrap_or(std::path::Path::new("."))).ok();
            let (memories, queries) = load_dataset(benchmark, &data_dir, limit.unwrap_or(usize::MAX))?;

            let cfg = RunConfig {
                benchmark,
                data_dir: data_dir.clone(),
                k,
                limit,
                concurrency: 4,
                output_path: out,
                skip_ingest,
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
