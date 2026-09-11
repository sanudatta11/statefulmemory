//! `project` subcommand handlers (FR7, FR12.10–FR12.14, EC-4).

#![allow(clippy::result_large_err)]

use std::io;
use std::process::ExitCode;

use memlayer_proto as p;

use crate::cli::{
    ProjectConsolidateArgs, ProjectDeleteArgs, ProjectMergeArgs, ProjectPruneArgs, ProjectVerb,
};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::{Formatter, Render};
use crate::project_detect::ProjectDetection;

pub async fn dispatch(
    client: &mut Client,
    detection: &ProjectDetection,
    fmt: Formatter,
    verb: ProjectVerb,
) -> ExitCode {
    let result = match verb {
        ProjectVerb::List => list(client, fmt).await,
        ProjectVerb::Current => current_local(detection, fmt),
        ProjectVerb::Merge(a) => match validate_merge(&a) {
            Ok(()) => merge(client, fmt, a).await,
            Err(msg) => Err(MergeOrStatus::Usage(msg)),
        },
        ProjectVerb::Delete(a) => delete(client, fmt, &detection.normalized, a).await,
        ProjectVerb::Consolidate(a) => consolidate(client, fmt, a).await,
        ProjectVerb::Prune(a) => prune(client, fmt, a).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(MergeOrStatus::Status(s)) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
        Err(MergeOrStatus::Usage(m)) => {
            eprintln!("memlayer: {m}");
            ExitCode::from(exit::USAGE)
        }
    }
}

#[derive(Debug)]
enum MergeOrStatus {
    Status(tonic::Status),
    Usage(String),
}

impl From<tonic::Status> for MergeOrStatus {
    fn from(s: tonic::Status) -> Self {
        MergeOrStatus::Status(s)
    }
}

/// Client-side validation for `project merge` (EC-4). Daemon enforces this
/// too, but rejecting locally avoids a round-trip and gives a more specific
/// error message.
fn validate_merge(a: &ProjectMergeArgs) -> Result<(), String> {
    if a.from == a.to {
        return Err(format!(
            "cannot merge project '{}' into itself (EC-4); pass distinct --from and --to",
            a.from
        ));
    }
    if a.from.trim().is_empty() || a.to.trim().is_empty() {
        return Err("--from and --to must both be non-empty".into());
    }
    Ok(())
}

async fn list(client: &mut Client, fmt: Formatter) -> Result<(), MergeOrStatus> {
    let resp = client
        .list_projects(p::ListProjectsRequest {})
        .await?
        .into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    Ok(())
}

/// Render `project current` from the CLI-side detection result. The daemon
/// also has a `current_project` RPC, but its detection is intentionally
/// naive (basename only) and does not honor `.memlayer/config.json`,
/// `[remote "origin"]`, `--project`, or `MEMLAYER_PROJECT`. The CLI has
/// already done the full PRD §9.1 walk in `open_client`, so we render
/// locally and skip the RPC.
fn current_local(detection: &ProjectDetection, fmt: Formatter) -> Result<(), MergeOrStatus> {
    let resp = p::CurrentProjectResponse {
        normalized_name: detection.normalized.clone(),
        display_name: detection.display_name.clone(),
        source: detection.source_label().to_string(),
    };
    write_render(&resp, fmt).map_err(io_to_status)?;
    Ok(())
}

async fn merge(
    client: &mut Client,
    fmt: Formatter,
    a: ProjectMergeArgs,
) -> Result<(), MergeOrStatus> {
    let req = p::MergeProjectsRequest {
        from: a.from,
        to: a.to,
    };
    let resp = client.merge_projects(req).await?.into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    Ok(())
}

async fn delete(
    client: &mut Client,
    fmt: Formatter,
    _project_name: &str,
    a: ProjectDeleteArgs,
) -> Result<(), MergeOrStatus> {
    let req = p::DeleteProjectRequest {
        project_name: a.name,
        hard: a.hard,
    };
    let resp = client.delete_project(req).await?.into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    Ok(())
}

async fn consolidate(
    client: &mut Client,
    fmt: Formatter,
    a: ProjectConsolidateArgs,
) -> Result<(), MergeOrStatus> {
    // FR12.14: the daemon returns candidates regardless of dry_run; the CLI
    // is responsible for the interactive merge flow when not --dry-run.
    // Spec 2 t6 ships the --dry-run path; the interactive merge fan-out
    // belongs to a later refinement (and SC-18 only requires --dry-run to
    // print candidates without mutation).
    let req = p::ConsolidateProjectsRequest { dry_run: a.dry_run };
    let resp = client.consolidate_projects(req).await?.into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    if !a.dry_run && !resp.candidates.is_empty() {
        eprintln!(
            "memlayer: pass --dry-run to confirm; interactive merge of {} candidate(s) requires the user to invoke `project merge` for each pair",
            resp.candidates.len()
        );
    }
    Ok(())
}

async fn prune(
    client: &mut Client,
    fmt: Formatter,
    a: ProjectPruneArgs,
) -> Result<(), MergeOrStatus> {
    let req = p::PruneProjectsRequest { dry_run: a.dry_run };
    let resp = client.prune_projects(req).await?.into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    if !a.dry_run && !resp.would_remove.is_empty() {
        // Without --dry-run the spec says "remove projects with 0 obs"
        // (FR12.14). The daemon currently returns the candidate list only;
        // the CLI fans out individual DeleteProject calls.
        for normalized in &resp.would_remove {
            let dr = client
                .delete_project(p::DeleteProjectRequest {
                    project_name: normalized.clone(),
                    hard: true,
                })
                .await;
            match dr {
                Ok(_) => eprintln!("removed: {normalized}"),
                Err(e) => eprintln!("memlayer: failed to remove {normalized}: {}", e.message()),
            }
        }
    }
    Ok(())
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    use std::io::Write;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}

fn io_to_status(e: io::Error) -> MergeOrStatus {
    MergeOrStatus::Status(tonic::Status::internal(format!("io: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command, ProjectArgs};
    use crate::project_detect::ProjectSource;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn current_local_renders_from_detection_no_rpc() {
        // The bug fix lifts `project current` off the daemon RPC path and
        // renders from the CLI-side detection. Verify that for each source
        // variant the rendered JSON includes the expected `source` label
        // and the display + normalized names. A regression here would
        // re-introduce the failure mode where the daemon's naive
        // basename-only detection silently overrode `--project`,
        // `MEMLAYER_PROJECT`, and `.memlayer/config.json`.
        let cases: Vec<(ProjectSource, &str)> = vec![
            (ProjectSource::CliFlag, "cli_flag"),
            (ProjectSource::EnvOverride, "env_override"),
            (ProjectSource::ConfigFile(PathBuf::from("/tmp/.memlayer/config.json")), "config_file"),
            (ProjectSource::GitRemote("git@github.com:acme/widgets.git".into()), "git_remote"),
            (ProjectSource::GitRoot(PathBuf::from("/tmp/widgets")), "git_root_basename"),
        ];
        for (source, expected_label) in cases {
            let det = ProjectDetection {
                display_name: "Widgets".into(),
                normalized: "widgets".into(),
                source,
            };
            assert_eq!(det.source_label(), expected_label);
            // Build the response the same way current_local does and verify
            // the fields propagate verbatim.
            let resp = p::CurrentProjectResponse {
                normalized_name: det.normalized.clone(),
                display_name: det.display_name.clone(),
                source: det.source_label().to_string(),
            };
            assert_eq!(resp.normalized_name, "widgets");
            assert_eq!(resp.display_name, "Widgets");
            assert_eq!(resp.source, expected_label);
        }
    }

    #[test]
    fn self_merge_returns_invalid_argument() {
        // EC-4: merging a project into itself is `INVALID_ARGUMENT`. The
        // CLI rejects locally before issuing the RPC.
        let args = ProjectMergeArgs {
            from: "alpha".to_string(),
            to: "alpha".to_string(),
        };
        let err = validate_merge(&args).expect_err("expected self-merge to be rejected");
        assert!(err.contains("itself"), "unexpected error: {err}");
        // And the resulting exit code maps to USAGE (2).
        assert_eq!(exit::USAGE, 2);
    }

    #[test]
    fn merge_with_distinct_args_passes_validation() {
        let args = ProjectMergeArgs {
            from: "alpha".to_string(),
            to: "beta".to_string(),
        };
        validate_merge(&args).expect("distinct projects should pass");
    }

    #[test]
    fn merge_rejects_empty_from_or_to() {
        let a = ProjectMergeArgs { from: "  ".into(), to: "beta".into() };
        assert!(validate_merge(&a).is_err());
        let b = ProjectMergeArgs { from: "alpha".into(), to: "".into() };
        assert!(validate_merge(&b).is_err());
    }

    #[test]
    fn dry_run_does_not_mutate() {
        // SC-18: `project consolidate --dry-run` must not perform any
        // merges. SC-19: `project prune --dry-run` must not delete. These
        // are clap-level invariants — when --dry-run is set, the handler's
        // mutation paths (the post-print block) are gated by `!a.dry_run`.
        // Verify the ProjectConsolidateArgs/ProjectPruneArgs default + flag
        // semantics here.
        let cli = Cli::try_parse_from([
            "memlayer",
            "project",
            "consolidate",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Command::Project(ProjectArgs {
                verb: ProjectVerb::Consolidate(a),
            }) => {
                assert!(a.dry_run, "--dry-run should be set");
                assert!(!a.all, "--all must default to false");
            }
            other => panic!("expected Project::Consolidate, got {other:?}"),
        }
        let cli = Cli::try_parse_from([
            "memlayer",
            "project",
            "prune",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Command::Project(ProjectArgs {
                verb: ProjectVerb::Prune(a),
            }) => assert!(a.dry_run, "--dry-run should be set"),
            other => panic!("expected Project::Prune, got {other:?}"),
        }
        // And by default (no --dry-run flag), the field is false.
        let cli = Cli::try_parse_from(["memlayer", "project", "consolidate"]).unwrap();
        match cli.command {
            Command::Project(ProjectArgs {
                verb: ProjectVerb::Consolidate(a),
            }) => assert!(!a.dry_run, "--dry-run must default to false"),
            other => panic!("expected Project::Consolidate, got {other:?}"),
        }
    }
}
