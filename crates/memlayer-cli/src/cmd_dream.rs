//! `memlayer dream` — review-only consolidation scan (Dream-lite).
//!
//! The daemon runs the heuristic scan and returns proposals; this module
//! renders a review report. Nothing is applied — applying a supersession is
//! the user's explicit action via `obs delete` / a future `--apply`.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use memlayer_proto as p;

use crate::cli::{DreamRunArgs, DreamVerb};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::Formatter;

pub async fn dispatch(
    client: &mut Client,
    project: &str,
    fmt: Formatter,
    verb: DreamVerb,
) -> ExitCode {
    let result = match verb {
        DreamVerb::Run(args) => run(client, project, fmt, args).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("memlayer: {msg}");
            ExitCode::from(exit::GENERAL)
        }
    }
}

async fn run(
    client: &mut Client,
    project: &str,
    fmt: Formatter,
    args: DreamRunArgs,
) -> Result<(), String> {
    let resp = client
        .dream_scan(p::DreamScanRequest {
            project_name: project.into(),
        })
        .await
        .map_err(|e| e.message().to_string())?
        .into_inner();

    match fmt {
        Formatter::Json | Formatter::Yaml => {
            let v = serde_json::json!({
                "observations_scanned": resp.observations_scanned,
                "proposals": resp.proposals.iter().map(|p| serde_json::json!({
                    "kind": p.kind,
                    "keep_id": p.keep_id,
                    "keep_title": p.keep_title,
                    "drop_id": p.drop_id,
                    "drop_title": p.drop_title,
                    "confidence": p.confidence,
                    "reason": p.reason,
                })).collect::<Vec<_>>(),
            });
            let text = if fmt == Formatter::Yaml {
                serde_yaml::to_string(&v).unwrap_or_default()
            } else {
                serde_json::to_string_pretty(&v).unwrap_or_default()
            };
            stdout_write(format!("{text}\n"))?;
        }
        Formatter::Text => {
            let mut s = String::new();
            use std::fmt::Write as _;
            writeln!(
                s,
                "Dream-lite review ({} observations scanned, {} proposals, review-only — nothing applied)",
                resp.observations_scanned,
                resp.proposals.len(),
            )
            .expect("fmt to String");
            for p in &resp.proposals {
                let conf = p.confidence * 100.0;
                match p.kind.as_str() {
                    "duplicate" => writeln!(
                        s,
                        "  [duplicate] {} #{} ~ {} #{} (will keep newer) — {:.0}%",
                        p.drop_title, p.drop_id, p.keep_title, p.keep_id, conf
                    )
                    .expect("fmt to String"),
                    _ => writeln!(
                        s,
                        "  [supersede] #{} <- #{} — {:.0}% — {}",
                        p.drop_id, p.keep_id, conf, p.reason
                    )
                    .expect("fmt to String"),
                }
            }
            if resp.proposals.is_empty() {
                writeln!(s, "  no proposals — memory looks clean").expect("fmt to String");
            }
            emit_text(&s, args.out.as_ref())?;
        }
    }
    Ok(())
}

/// Write `text` to `--out` if given, else stdout.
fn emit_text(text: &str, out: Option<&PathBuf>) -> Result<(), String> {
    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        eprintln!("wrote {}", path.display());
    } else {
        stdout_write(text.to_string())?;
    }
    Ok(())
}

fn stdout_write(text: String) -> Result<(), String> {
    let mut handle = std::io::stdout().lock();
    handle
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    handle.flush().map_err(|e| e.to_string())
}
