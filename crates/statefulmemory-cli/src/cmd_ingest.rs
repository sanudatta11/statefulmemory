//! `smem ingest repo` — index git-tracked text files into durable memory.
//!
//! Puts the monorepo *outside* the KV cache so `context compile` can pack a
//! host window from path-anchored observations (Wave 2).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use sha2::{Digest, Sha256};
use statefulmemory_core::git;
use statefulmemory_proto as p;

use crate::cli::IngestRepoArgs;
use crate::cmd_obs::{Client, MAX_CONTENT_CHARS};
use crate::exit;
use crate::formatter::Formatter;

const DEFAULT_EXTS: &[&str] = &[
    "rs", "md", "toml", "txt", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt", "c", "h", "cpp",
    "hpp", "css", "html", "yml", "yaml", "json", "sh", "sql", "proto",
];

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: IngestRepoArgs,
) -> ExitCode {
    let started = Instant::now();
    let root = a
        .path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    if !git::is_repo(&root) {
        eprintln!(
            "statefulmemory ingest repo: {} is not a git work tree",
            root.display()
        );
        return ExitCode::from(exit::USAGE);
    }

    let files = match git::tracked_files(&root) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("statefulmemory ingest repo: {e}");
            return ExitCode::from(exit::GENERAL);
        }
    };

    let exts: Vec<String> = a
        .ext
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_else(|| DEFAULT_EXTS.iter().map(|s| (*s).to_string()).collect());

    let max_bytes = a.max_bytes.unwrap_or(256 * 1024) as u64;
    let session_id = a
        .session
        .clone()
        .unwrap_or_else(|| format!("ingest-{}", chrono::Utc::now().format("%Y%m%d")));

    let mut saved = 0u32;
    let mut skipped = 0u32;
    let mut truncated = 0u32;
    let mut errors = 0u32;
    let mut examples: Vec<String> = Vec::new();

    for rel in &files {
        let path = root.join(rel);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !exts.iter().any(|e| e == &ext) {
            skipped += 1;
            continue;
        }
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if !meta.is_file() || meta.len() > max_bytes {
            skipped += 1;
            continue;
        }
        let raw = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if raw.contains(&0) {
            skipped += 1;
            continue;
        }
        let mut content = String::from_utf8_lossy(&raw).into_owned();
        if content.trim().is_empty() {
            skipped += 1;
            continue;
        }
        if content.chars().count() > MAX_CONTENT_CHARS {
            content = content.chars().take(MAX_CONTENT_CHARS).collect();
            truncated += 1;
        }

        let sync_id = stable_sync_id(project_name, rel, &content);
        let obs_type = if ext == "md" { "doc" } else { "note" };
        let req = p::SaveObservationRequest {
            project_name: project_name.to_string(),
            sync_id: Some(sync_id),
            session_id: session_id.clone(),
            r#type: obs_type.into(),
            title: rel.clone(),
            content,
            tool_name: Some("ingest.repo".into()),
            scope: "project".into(),
            created_by: Some("smem-ingest".into()),
            topic_key: Some(format!("ingest/file/{rel}")),
            code_anchor: Some(rel.clone()),
            anchors: vec![rel.clone()],
        };

        if a.dry_run {
            saved += 1;
            if examples.len() < 5 {
                examples.push(rel.clone());
            }
            continue;
        }

        match client.save_observation(req).await {
            Ok(_) => {
                saved += 1;
                if examples.len() < 5 {
                    examples.push(rel.clone());
                }
            }
            Err(s) => {
                errors += 1;
                eprintln!("  skip {rel}: {}", s.message());
            }
        }
    }

    if matches!(fmt, Formatter::Json | Formatter::Yaml) {
        let out = serde_json::json!({
            "root": root.display().to_string(),
            "saved": saved,
            "skipped": skipped,
            "truncated": truncated,
            "errors": errors,
            "dry_run": a.dry_run,
            "examples": examples,
            "duration_ms": started.elapsed().as_millis(),
        });
        match fmt {
            Formatter::Json => println!("{}", serde_json::to_string_pretty(&out).unwrap()),
            Formatter::Yaml => {
                // minimal yaml via json for now
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
            }
            Formatter::Text => unreachable!(),
        }
    } else {
        println!(
            "ingest repo {} — saved={saved} skipped={skipped} truncated={truncated} errors={errors}{}",
            root.display(),
            if a.dry_run { " (dry-run)" } else { "" }
        );
        for e in &examples {
            println!("  · {e}");
        }
        if !a.dry_run && saved > 0 {
            println!("Next: smem context compile --query \"…\" --window 200k");
        }
    }

    if errors > 0 {
        ExitCode::from(exit::GENERAL)
    } else {
        ExitCode::SUCCESS
    }
}

fn stable_sync_id(project: &str, path: &str, content: &str) -> String {
    let mut h = Sha256::new();
    h.update(project.as_bytes());
    h.update(b"\0");
    h.update(path.as_bytes());
    h.update(b"\0");
    h.update(content.as_bytes());
    let dig = h.finalize();
    // UUID-shaped hex for readability / uniqueness (not RFC UUID).
    format!(
        "ingest-{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        dig[0], dig[1], dig[2], dig[3], dig[4], dig[5], dig[6], dig[7], dig[8], dig[9], dig[10],
        dig[11], dig[12], dig[13], dig[14], dig[15]
    )
}

#[allow(dead_code)]
fn path_under(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
}
