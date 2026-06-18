//! `obs` subcommand handlers (FR4, FR12.1–FR12.6).
//!
//! Each verb function takes a connected `MemlayerClient<Channel>`, the
//! resolved project name, the chosen [`Formatter`], and the verb-specific
//! args. It builds the matching `memlayer_proto` request, awaits the RPC,
//! and renders the response via the [`Render`] impls in [`crate::render`].
//!
//! Stdin reads for `obs save --content -` and `obs capture-passive --text -`
//! enforce the EC-7 50,000-character cap.

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::Instant;

use memlayer_proto as p;
use tonic::transport::Channel;
use tonic::Status;

use crate::audit::{self, AuditEntry, HitMeta};
use crate::cli::{
    ObsCapturePassiveArgs, ObsContextArgs, ObsDeleteArgs, ObsFactsArgs, ObsGetArgs, ObsListArgs,
    ObsRecentArgs, ObsReextractArgs, ObsSaveArgs, ObsSearchArgs, ObsSuggestTopicKeyArgs,
    ObsTimelineArgs, ObsUpdateArgs, ObsVerb,
};
use crate::exit;
use crate::formatter::{Formatter, Render};

/// EC-7 maximum content length for `obs save` and `capture-passive`, in chars.
pub const MAX_CONTENT_CHARS: usize = 50_000;

pub type Client = p::memlayer_client::MemlayerClient<Channel>;

/// Entry point invoked from `main.rs`. Dispatches on the verb, runs the RPC,
/// and emits output. Returns the appropriate exit code (FR13).
pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    quiet: bool,
    args: ObsVerb,
) -> ExitCode {
    let result = match args {
        ObsVerb::Save(a) => save(client, project_name, fmt, quiet, a).await,
        ObsVerb::Update(a) => update(client, project_name, fmt, a).await,
        ObsVerb::Delete(a) => delete(client, project_name, a).await,
        ObsVerb::Get(a) => get(client, project_name, fmt, a).await,
        ObsVerb::Search(a) => search(client, project_name, fmt, a).await,
        ObsVerb::Recent(a) => recent(client, project_name, fmt, a).await,
        ObsVerb::List(a) => list(client, project_name, fmt, a).await,
        ObsVerb::Context(a) => context(client, project_name, fmt, a).await,
        ObsVerb::Timeline(a) => timeline(client, project_name, fmt, a).await,
        ObsVerb::SuggestTopicKey(a) => suggest_topic_key(client, project_name, fmt, a).await,
        ObsVerb::CapturePassive(a) => capture_passive(client, project_name, fmt, a).await,
        ObsVerb::Facts(a) => facts(client, project_name, fmt, a).await,
        ObsVerb::Reextract(a) => reextract(a).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(VerbError::Status(s)) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
        Err(VerbError::Usage(msg)) => {
            eprintln!("memlayer: {msg}");
            ExitCode::from(exit::USAGE)
        }
        Err(VerbError::Io(e)) => {
            eprintln!("memlayer: i/o error: {e}");
            ExitCode::from(exit::GENERAL)
        }
    }
}

#[derive(Debug)]
enum VerbError {
    Status(Status),
    Usage(String),
    Io(io::Error),
}

impl From<Status> for VerbError {
    fn from(s: Status) -> Self {
        VerbError::Status(s)
    }
}

impl From<io::Error> for VerbError {
    fn from(e: io::Error) -> Self {
        VerbError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Verb handlers
// ---------------------------------------------------------------------------

async fn save(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    _quiet: bool,
    a: ObsSaveArgs,
) -> Result<(), VerbError> {
    let started = Instant::now();
    let content = if a.content == "-" {
        read_capped(io::stdin().lock(), MAX_CONTENT_CHARS)
            .map_err(|m| VerbError::Usage(m.to_string()))?
    } else {
        a.content
    };
    let req = p::SaveObservationRequest {
        project_name: project_name.to_string(),
        sync_id: None,
        session_id: a.session.unwrap_or_default(),
        r#type: a.r#type,
        title: a.title,
        content,
        tool_name: None,
        scope: a.scope,
        created_by: None,
        topic_key: a.topic,
    };
    let resp = client.save_observation(req).await?.into_inner();
    write_render(&resp, fmt)?;
    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "obs.save",
        project: Some(project_name),
        result_count: resp.observation.as_ref().map(|_| 1),
        duration_ms: started.elapsed().as_millis(),
        query: None,
        top_hits: None,
    });
    // Inform the user if any conflicting observations were superseded.
    for old in &resp.similar_observations {
        if old.id > 0 {
            eprintln!("  ↳ Superseded observation #{} (soft-deleted)", old.id);
        }
    }
    Ok(())
}

async fn update(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsUpdateArgs,
) -> Result<(), VerbError> {
    let key = parse_obs_key(&a.id);
    let req = p::UpdateObservationRequest {
        project_name: project_name.to_string(),
        key: Some(match key {
            ObsKey::Id(id) => p::update_observation_request::Key::Id(id),
            ObsKey::SyncId(s) => p::update_observation_request::Key::SyncId(s),
        }),
        title: a.title,
        content: a.content,
        topic_key: a.topic,
        scope: a.scope,
        r#type: a.r#type,
    };
    let resp = client.update_observation(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn delete(
    client: &mut Client,
    project_name: &str,
    a: ObsDeleteArgs,
) -> Result<(), VerbError> {
    let key = parse_obs_key(&a.id);
    let req = p::DeleteObservationRequest {
        project_name: project_name.to_string(),
        key: Some(match key {
            ObsKey::Id(id) => p::delete_observation_request::Key::Id(id),
            ObsKey::SyncId(s) => p::delete_observation_request::Key::SyncId(s),
        }),
        hard: a.hard,
    };
    client.delete_observation(req).await?;
    Ok(())
}

async fn get(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsGetArgs,
) -> Result<(), VerbError> {
    let started = Instant::now();
    let key = parse_obs_key(&a.id);
    let req = p::GetObservationRequest {
        project_name: project_name.to_string(),
        key: Some(match key {
            ObsKey::Id(id) => p::get_observation_request::Key::Id(id),
            ObsKey::SyncId(s) => p::get_observation_request::Key::SyncId(s),
        }),
    };
    let resp = client.get_observation(req).await?.into_inner();
    write_render(&resp, fmt)?;
    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "obs.get",
        project: Some(project_name),
        result_count: resp.observation.as_ref().map(|_| 1),
        duration_ms: started.elapsed().as_millis(),
        query: None,
        top_hits: None,
    });
    Ok(())
}

async fn search(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsSearchArgs,
) -> Result<(), VerbError> {
    let started = Instant::now();
    let query_for_audit = a.query.clone();
    let req = p::SearchObservationsRequest {
        project_name: project_name.to_string(),
        query: a.query,
        r#type: a.r#type,
        scope: a.scope,
        all_projects: a.all_projects,
        limit: a.limit,
        mode: Some(a.mode),
        rerank: a.rerank,
    };
    let resp = client.search_observations(req).await?.into_inner();
    write_render(&resp, fmt)?;
    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "obs.search",
        project: Some(project_name),
        result_count: Some(resp.observations.len()),
        duration_ms: started.elapsed().as_millis(),
        query: audit::full_mode_enabled().then_some(query_for_audit),
        top_hits: audit::full_mode_enabled().then(|| {
            resp.observations
                .iter()
                .take(10)
                .map(|o| HitMeta {
                    id: o.id,
                    r#type: o.r#type.clone(),
                    title: o.title.clone(),
                })
                .collect()
        }),
    });
    Ok(())
}

async fn recent(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsRecentArgs,
) -> Result<(), VerbError> {
    let started = Instant::now();
    let req = p::RecentObservationsRequest {
        project_name: project_name.to_string(),
        limit: a.limit,
        scope: a.scope,
    };
    let resp = client.recent_observations(req).await?.into_inner();
    write_render(&resp, fmt)?;
    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "obs.recent",
        project: Some(project_name),
        result_count: Some(resp.observations.len()),
        duration_ms: started.elapsed().as_millis(),
        query: None,
        top_hits: audit::full_mode_enabled().then(|| {
            resp.observations
                .iter()
                .take(10)
                .map(|o| HitMeta {
                    id: o.id,
                    r#type: o.r#type.clone(),
                    title: o.title.clone(),
                })
                .collect()
        }),
    });
    Ok(())
}

async fn list(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsListArgs,
) -> Result<(), VerbError> {
    let req = p::ListObservationsRequest {
        project_name: project_name.to_string(),
        r#type: a.r#type,
        scope: a.scope,
        created_by: None,
        due_for_review: a.due_for_review,
        limit: a.limit,
        cursor: a.cursor.map(|t| p::Cursor { token: t }),
        session_id: None,
    };
    let resp = client.list_observations(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn context(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsContextArgs,
) -> Result<(), VerbError> {
    let started = Instant::now();
    // For JSON/YAML output keep the original ContextResponse (callers may
    // depend on the structured shape). For text output we emit a richer
    // briefing: latest session summary + decisions due for review + recent.
    if fmt != Formatter::Text {
        let req = p::ContextRequest {
            project_name: project_name.to_string(),
            recent_limit: a.limit,
            mode: Some(a.mode.clone()),
            rerank: a.rerank.clone(),
            query: a.query.clone(),
        };
        let resp = client.context(req).await?.into_inner();
        let recent_count = resp
            .snapshot
            .as_ref()
            .map(|s| s.recent_observations.len())
            .unwrap_or(0);
        write_render(&resp, fmt)?;
        audit::record(&AuditEntry {
            ts: audit::now_rfc3339(),
            command: "obs.context",
            project: Some(project_name),
            result_count: Some(recent_count),
            duration_ms: started.elapsed().as_millis(),
            query: None,
            top_hits: None,
        });
        return Ok(());
    }

    // Text mode: compose briefing from three RPCs.
    use std::io::Write;
    let stdout = io::stdout();
    let mut h = stdout.lock();

    // 1. Latest session summary (search by topic_key prefix via title hit).
    let summary_req = p::SearchObservationsRequest {
        project_name: project_name.to_string(),
        query: "session-summary".into(),
        r#type: Some("note".into()),
        scope: None,
        limit: 1,
        all_projects: false,
        mode: None,
        rerank: None,
    };
    let summaries = client.search_observations(summary_req).await?.into_inner();
    if let Some(latest) = summaries.observations.first() {
        if latest
            .topic_key
            .as_deref()
            .map(|t| t.starts_with("session-summary/"))
            .unwrap_or(false)
        {
            writeln!(h, "# Last session summary")?;
            writeln!(h, "{}", latest.content.trim_end())?;
            writeln!(h)?;
        }
    }

    // 2. Decisions due for review.
    let due_req = p::ListObservationsRequest {
        project_name: project_name.to_string(),
        r#type: None,
        scope: None,
        created_by: None,
        due_for_review: true,
        limit: 10,
        cursor: None,
        session_id: None,
    };
    let due = client.list_observations(due_req).await?.into_inner();
    if !due.observations.is_empty() {
        writeln!(h, "# Pending review")?;
        for o in &due.observations {
            writeln!(h, "- [#{}] {} ({})", o.id, o.title, o.r#type)?;
        }
        writeln!(h)?;
    }

    // 3. Recent observations.
    let req = p::ContextRequest {
        project_name: project_name.to_string(),
        recent_limit: a.limit,
        mode: Some(a.mode.clone()),
        rerank: a.rerank.clone(),
        query: a.query.clone(),
    };
    let resp = client.context(req).await?.into_inner();
    let recent_count = resp
        .snapshot
        .as_ref()
        .map(|s| s.recent_observations.len())
        .unwrap_or(0);
    resp.render(Formatter::Text, &mut h)?;
    h.flush()?;
    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "obs.context",
        project: Some(project_name),
        result_count: Some(recent_count),
        duration_ms: started.elapsed().as_millis(),
        query: None,
        top_hits: None,
    });
    Ok(())
}

async fn timeline(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsTimelineArgs,
) -> Result<(), VerbError> {
    let key = parse_obs_key(&a.id);
    let req = p::TimelineRequest {
        project_name: project_name.to_string(),
        anchor: Some(match key {
            ObsKey::Id(id) => p::timeline_request::Anchor::Id(id),
            ObsKey::SyncId(s) => p::timeline_request::Anchor::SyncId(s),
        }),
        before: a.before,
        after: a.after,
    };
    let resp = client.timeline(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn suggest_topic_key(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsSuggestTopicKeyArgs,
) -> Result<(), VerbError> {
    let req = p::SuggestTopicKeyRequest {
        project_name: project_name.to_string(),
        title: a.title,
        r#type: a.r#type,
        scope: a.scope,
    };
    let resp = client.suggest_topic_key(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn capture_passive(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ObsCapturePassiveArgs,
) -> Result<(), VerbError> {
    let text = if a.text == "-" {
        read_capped(io::stdin().lock(), MAX_CONTENT_CHARS)
            .map_err(|m| VerbError::Usage(m.to_string()))?
    } else {
        a.text
    };
    let session_id = a.session.unwrap_or_default();
    let req = p::CapturePassiveRequest {
        project_name: project_name.to_string(),
        text,
        session_id: session_id.clone(),
    };
    let resp = client.capture_passive(req).await?.into_inner();

    // FR12.6 / SC-15: the daemon returns the extracted snippets; the CLI
    // fans each one out to SaveObservation as a `learning`-type observation
    // attached to the current session. Returning early with an empty list
    // gives EC-3 (no `## Key Learnings:` header) the right shape: zero
    // saves, exit 0.
    let mut saved: Vec<p::Observation> = Vec::with_capacity(resp.snippets.len());
    for (i, snippet) in resp.snippets.iter().enumerate() {
        let title = snippet
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                let mut t = s.to_string();
                if t.chars().count() > 60 {
                    t = t.chars().take(60).collect::<String>() + "…";
                }
                t
            })
            .unwrap_or_else(|| format!("learning {}", i + 1));
        let save_req = p::SaveObservationRequest {
            project_name: project_name.to_string(),
            sync_id: None,
            session_id: session_id.clone(),
            r#type: "learning".to_string(),
            title,
            content: snippet.clone(),
            tool_name: None,
            scope: "project".to_string(),
            created_by: None,
            topic_key: None,
        };
        let s = client.save_observation(save_req).await?.into_inner();
        if let Some(o) = s.observation {
            saved.push(o);
        }
    }

    let response = p::CapturePassiveResponse { snippets: resp.snippets };
    let combined = crate::render::CapturePassiveOutcome {
        response,
        saved,
    };
    write_render(&combined, fmt)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObsKey {
    Id(i64),
    SyncId(String),
}

fn parse_obs_key(s: &str) -> ObsKey {
    match s.parse::<i64>() {
        Ok(n) => ObsKey::Id(n),
        Err(_) => ObsKey::SyncId(s.to_string()),
    }
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}

/// Read up to `max_chars` characters from `r`. Returns
/// `Err(error_message)` when the input exceeds the cap or is not valid
/// UTF-8. Used for `obs save --content -` and `obs capture-passive --text -`
/// per EC-7.
pub fn read_capped<R: Read>(r: R, max_chars: usize) -> Result<String, String> {
    // Cap byte length at 4 * max_chars + 1 so any 4-byte UTF-8 sequence still
    // fits, but oversized inputs trip the limit.
    let max_bytes = max_chars
        .checked_mul(4)
        .and_then(|n| n.checked_add(1))
        .unwrap_or(usize::MAX);
    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
    r.take(max_bytes as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read stdin: {e}"))?;
    let s = String::from_utf8(buf).map_err(|_| "stdin is not valid UTF-8".to_string())?;
    let n_chars = s.chars().count();
    if n_chars > max_chars {
        return Err(format!(
            "stdin exceeds {max_chars}-character cap (got at least {n_chars} chars)"
        ));
    }
    Ok(s)
}

/// Stub implementation for `obs facts <id>`. The full implementation lands
/// in rp-t10 (daemon `GetFacts` handler + render). For now we surface a
/// "not yet implemented" error so the CLI flag wiring (rp-t7) lands without
/// a non-exhaustive match.
async fn facts(
    _client: &mut Client,
    _project_name: &str,
    _fmt: Formatter,
    _a: ObsFactsArgs,
) -> Result<(), VerbError> {
    Err(VerbError::Usage(
        "obs facts is not yet wired up (lands in rp-t10)".into(),
    ))
}

/// Stub implementation for `obs reextract`. Lands in rp-t13 as a
/// deferred-feature stub; full implementation is a future spec.
async fn reextract(_a: ObsReextractArgs) -> Result<(), VerbError> {
    Err(VerbError::Usage(
        "obs reextract is deferred to a future spec; flip extract.enabled=true \
         in ~/.memlayer/config.toml to extract facts on new saves"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;
    use std::io::Cursor;

    #[test]
    fn stdin_size_cap_rejects_over_50k() {
        // Build an input that's strictly larger than the cap.
        let body = "a".repeat(MAX_CONTENT_CHARS + 1);
        let r = Cursor::new(body.as_bytes());
        let err = read_capped(r, MAX_CONTENT_CHARS).expect_err("expected EC-7 cap rejection");
        assert!(
            err.contains("50000-character cap")
                || err.contains(&format!("{MAX_CONTENT_CHARS}-character cap")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn stdin_under_cap_returns_full_content() {
        let body = "x".repeat(100);
        let r = Cursor::new(body.as_bytes());
        let got = read_capped(r, MAX_CONTENT_CHARS).unwrap();
        assert_eq!(got.len(), 100);
    }

    #[test]
    fn stdin_rejects_invalid_utf8() {
        let bad = vec![0x80, 0x81, 0x82];
        let r = Cursor::new(&bad[..]);
        let err = read_capped(r, MAX_CONTENT_CHARS).expect_err("expected utf8 rejection");
        assert!(err.contains("UTF-8"), "unexpected error: {err}");
    }

    #[test]
    fn default_scope_is_project() {
        // FR4: `obs save` defaults --scope to "project".
        let cli = Cli::try_parse_from([
            "memlayer",
            "obs",
            "save",
            "--title",
            "t",
            "--content",
            "c",
        ])
        .expect("parse");
        match cli.command {
            Command::Obs(args) => match args.verb {
                ObsVerb::Save(s) => {
                    assert_eq!(s.scope, "project", "default --scope must be 'project'");
                    assert_eq!(s.r#type, "note", "default --type must be 'note'");
                }
                other => panic!("expected Save, got {other:?}"),
            },
            other => panic!("expected Obs, got {other:?}"),
        }
    }

    #[test]
    fn obs_key_parses_numeric_as_id() {
        match parse_obs_key("42") {
            ObsKey::Id(n) => assert_eq!(n, 42),
            other => panic!("expected Id, got {other:?}"),
        }
    }

    #[test]
    fn obs_key_parses_non_numeric_as_sync_id() {
        match parse_obs_key("a1b2-c3d4") {
            ObsKey::SyncId(s) => assert_eq!(s, "a1b2-c3d4"),
            other => panic!("expected SyncId, got {other:?}"),
        }
    }
}
