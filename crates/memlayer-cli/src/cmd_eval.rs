//! `memlayer eval` — benchmark evaluation and scorecard reporting.

use std::path::PathBuf;
use std::process::ExitCode;

use memlayer_eval::config::{RetrievalConfig, RetrievalMode, default_profile};
use memlayer_eval::datasets::{EvalMemory, EvalQuery};
use memlayer_eval::{BenchmarkKind, RunConfig, Scorecard};

use crate::cli::{EvalArgs, OutputFormat};
use crate::exit;

pub async fn dispatch(args: EvalArgs, output_fmt: Option<OutputFormat>) -> ExitCode {
    let result = run_eval(args, output_fmt).await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("memlayer: {e}");
            ExitCode::from(exit::USAGE)
        }
    }
}

async fn run_eval(args: EvalArgs, output_fmt: Option<OutputFormat>) -> Result<(), String> {
    let benchmark_kind = match args.benchmark.to_lowercase().as_str() {
        "locomo" => BenchmarkKind::Locomo,
        "longmemeval" | "lme" => BenchmarkKind::Longmemeval,
        "beam1m" | "beam-1m" => BenchmarkKind::Beam1m,
        "beam10m" | "beam-10m" => BenchmarkKind::Beam10m,
        "staleness" => BenchmarkKind::Staleness,
        other => {
            return Err(format!(
                "unknown benchmark '{other}'; valid: locomo, longmemeval, beam1m, beam10m, staleness"
            ))
        }
    };

    let data_dir = std::env::var("MEMLAYER_EVAL_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"));

    let (memories, queries, retrieval, eval_data_dir, lexical_judge, skip_ingest, shards) =
        if args.smoke {
            let tmp = tempfile::tempdir().map_err(|e| format!("create temp eval dir: {e}"))?;
            let eval_data_dir = tmp.path().to_path_buf();
            // Keep TempDir alive for the run by leaking it — the process is
            // short-lived; dropping would delete the DB under the runner.
            std::mem::forget(tmp);
            if benchmark_kind == BenchmarkKind::Staleness {
                let (memories, queries) = memlayer_eval::datasets::staleness::load(&eval_data_dir)
                    .map_err(|e| format!("load staleness fixture: {e:#}"))?;
                (
                    memories,
                    queries,
                    RetrievalConfig {
                        mode: RetrievalMode::Bm25,
                        k: 10,
                        evidence_window: 0,
                        rerank: false,
                        decay_lambda: 0.0,
                    },
                    eval_data_dir,
                    true,
                    false,
                    1usize,
                )
            } else {
            (
                smoke_memories(),
                smoke_queries(),
                RetrievalConfig {
                    mode: RetrievalMode::Bm25,
                    k: 10,
                    evidence_window: 0,
                    rerank: false,
                    decay_lambda: 0.0,
                },
                eval_data_dir,
                true,
                false,
                1usize,
            )
            }
        } else {
            if benchmark_kind == BenchmarkKind::Locomo {
                let locomo = data_dir.join("locomo").join("locomo10.json");
                if !locomo.exists() {
                    return Err(format!(
                        "LoCoMo dataset missing at {}. Download locomo10.json into data/locomo/",
                        locomo.display()
                    ));
                }
            }
            let (memories, queries) = load_dataset(benchmark_kind, &data_dir, args.limit)?;
            let retrieval = default_profile(benchmark_kind);
            let shards = if matches!(
                benchmark_kind,
                BenchmarkKind::Beam1m | BenchmarkKind::Beam10m
            ) {
                16
            } else {
                1
            };
            (
                memories,
                queries,
                retrieval,
                data_dir,
                false,
                false,
                shards,
            )
        };

    let limit = if args.smoke {
        Some(args.limit.unwrap_or(queries.len().max(1)))
    } else {
        args.limit
    };

    let output_path = eval_data_dir.join("report.md");
    let cfg = RunConfig {
        benchmark: benchmark_kind,
        data_dir: eval_data_dir,
        k: retrieval.k,
        limit,
        concurrency: memlayer_eval::runner::eval_concurrency_from_env(4),
        output_path,
        skip_ingest,
        retrieval,
        trace_path: None,
        shards,
        lexical_judge,
        no_supersede: args.no_supersede,
    };

    let report = memlayer_eval::runner::run(&cfg, memories, queries)
        .await
        .map_err(|e| format!("eval run failed: {e:#}"))?;

    let commit_hash = option_env!("GIT_COMMIT_HASH").unwrap_or("dev");
    let scorecard = Scorecard::from_report(&report, commit_hash);

    if let Some(save_path) = args.save_scorecard {
        scorecard
            .save_to_file(&save_path)
            .map_err(|e| format!("failed to save scorecard to {}: {e}", save_path.display()))?;
        eprintln!("Saved evaluation scorecard to {}", save_path.display());
    }

    match output_fmt {
        Some(OutputFormat::Json) => {
            let json = serde_json::to_string_pretty(&scorecard)
                .map_err(|e| format!("serialize scorecard JSON: {e}"))?;
            println!("{json}");
        }
        _ => {
            println!("{}", scorecard.render_text());
        }
    }

    Ok(())
}

fn smoke_memories() -> Vec<EvalMemory> {
    vec![
        EvalMemory {
            project: "locomo-smoke".into(),
            session_id: "smoke-session".into(),
            obs_type: "note".into(),
            title: "Caroline went to the park".into(),
            content: "Caroline went to the park on Saturday with friends.".into(),
            topic_key: None,
        },
        EvalMemory {
            project: "locomo-smoke".into(),
            session_id: "smoke-session".into(),
            obs_type: "note".into(),
            title: "Weather was sunny".into(),
            content: "It was a sunny afternoon downtown.".into(),
            topic_key: None,
        },
    ]
}

fn smoke_queries() -> Vec<EvalQuery> {
    vec![EvalQuery {
        // infer_project(Locomo, "smoke-1") → locomo-smoke
        id: "smoke-1".into(),
        question: "Where did Caroline go on Saturday?".into(),
        gold_answer: "the park".into(),
        judge_context: None,
        category: None,
        anti_answer: None,
        evidence: Vec::new(),
    }]
}

fn load_dataset(
    kind: BenchmarkKind,
    data_dir: &std::path::Path,
    limit: Option<usize>,
) -> Result<(Vec<EvalMemory>, Vec<EvalQuery>), String> {
    match kind {
        BenchmarkKind::Locomo => {
            let (m, q) = memlayer_eval::datasets::locomo::load(data_dir)
                .map_err(|e| format!("load LoCoMo: {e:#}"))?;
            Ok((
                m,
                memlayer_eval::apply_query_limit(q, limit, true),
            ))
        }
        BenchmarkKind::Longmemeval => {
            let (m, q) = memlayer_eval::datasets::longmemeval::load(data_dir)
                .map_err(|e| format!("load LongMemEval: {e:#}"))?;
            Ok((m, memlayer_eval::apply_query_limit(q, limit, false)))
        }
        BenchmarkKind::Beam1m => {
            let cap = limit.unwrap_or(usize::MAX);
            let (m, q) = memlayer_eval::datasets::beam::generate_all(
                memlayer_eval::datasets::beam::BeamScale::M1,
                cap.min(1000),
            )
            .map_err(|e| format!("generate BEAM-1M: {e:#}"))?;
            Ok((m, q))
        }
        BenchmarkKind::Beam10m => Err(
            "beam-10m requires `eval prepare` first; use the eval binary or --smoke".into(),
        ),
        BenchmarkKind::Staleness => {
            let (m, q) = memlayer_eval::datasets::staleness::load(data_dir)
                .map_err(|e| format!("load staleness: {e:#}"))?;
            Ok((m, memlayer_eval::apply_query_limit(q, limit, false)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_eval_smoke_does_not_invent_accuracy() {
        let args = EvalArgs {
            benchmark: "locomo".into(),
            smoke: true,
            limit: Some(1),
            save_scorecard: None,
            no_supersede: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(run_eval(args, Some(OutputFormat::Json)));
        assert!(res.is_ok(), "{res:?}");
    }

    #[test]
    fn smoke_fixture_gold_is_in_memory() {
        let mems = smoke_memories();
        let q = &smoke_queries()[0];
        assert!(mems
            .iter()
            .any(|m| m.content.to_lowercase().contains(&q.gold_answer)));
        assert_eq!(q.id, "smoke-1");
    }

    #[test]
    fn locomo_without_dataset_errors_when_not_smoke() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let args = EvalArgs {
            benchmark: "locomo".into(),
            smoke: false,
            limit: Some(1),
            save_scorecard: None,
            no_supersede: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        std::env::set_var("MEMLAYER_EVAL_DATA", "/tmp/memlayer-eval-missing-dataset");
        let res = rt.block_on(run_eval(args, Some(OutputFormat::Json)));
        std::env::remove_var("MEMLAYER_EVAL_DATA");
        match res {
            Err(e) => {
                assert!(e.contains("LoCoMo dataset missing"), "{e}");
            }
            Ok(()) => panic!("expected missing-dataset error"),
        }
    }
}
