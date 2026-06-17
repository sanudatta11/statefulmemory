//! `session` subcommand handlers (FR5, FR12.7, FR12.8).

use std::io;
use std::process::ExitCode;
use std::time::Instant;

use memlayer_proto as p;

use crate::audit::{self, AuditEntry};
use crate::cli::{
    SessionDeleteArgs, SessionEndArgs, SessionGetArgs, SessionListArgs, SessionStartArgs,
    SessionSummarizeArgs, SessionSummaryArgs, SessionVerb,
};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::{Formatter, Render};

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    verb: SessionVerb,
) -> ExitCode {
    let result: Result<(), tonic::Status> = match verb {
        SessionVerb::Start(a) => start(client, project_name, fmt, a).await,
        SessionVerb::End(a) => end(client, project_name, fmt, a).await,
        SessionVerb::Summary(a) => summary(client, project_name, fmt, a).await,
        SessionVerb::Summarize(a) => summarize(client, project_name, fmt, a).await,
        SessionVerb::List(a) => list(client, project_name, fmt, a).await,
        SessionVerb::Get(a) => get(client, project_name, fmt, a).await,
        SessionVerb::Delete(a) => delete(client, project_name, a).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(s) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

async fn start(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionStartArgs,
) -> Result<(), tonic::Status> {
    let req = p::StartSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
        directory: a.directory.unwrap_or_default(),
    };
    let resp = client.start_session(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn end(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionEndArgs,
) -> Result<(), tonic::Status> {
    let req = p::EndSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
        summary: a.summary,
    };
    let resp = client.end_session(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn summary(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionSummaryArgs,
) -> Result<(), tonic::Status> {
    let req = p::SaveSessionSummaryRequest {
        project_name: project_name.to_string(),
        id: a.id,
        summary: a.content,
    };
    let resp = client.save_session_summary(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

/// Engram-style session rollup. Two modes:
/// - default (--auto): list observations by session_id, group by topic_key
///   (or title fallback), pick latest per group, render Engram-shaped markdown.
/// - --content -: read agent-supplied prose from stdin and use it verbatim.
/// Both paths save as a single observation with topic_key="session-summary/<id>"
/// so the V3 supersession path dedups re-runs (and supersedes the auto rollup
/// when an agent later writes a richer prose version).
async fn summarize(
    client: &mut Client,
    project_name: &str,
    _fmt: Formatter,
    a: SessionSummarizeArgs,
) -> Result<(), tonic::Status> {
    use std::io::Read;

    let started = Instant::now();
    let session_id = a.id.clone();

    // Decide body source.
    let body: String = match a.content.as_deref() {
        Some("-") => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| tonic::Status::internal(format!("read stdin: {e}")))?;
            buf
        }
        Some(literal) => literal.to_string(),
        None => {
            // --auto path: pull observations belonging to the session and roll up.
            let req = p::ListObservationsRequest {
                project_name: project_name.to_string(),
                r#type: None,
                scope: None,
                created_by: None,
                due_for_review: false,
                limit: 200,
                cursor: None,
                session_id: Some(session_id.clone()),
            };
            let resp = client.list_observations(req).await?.into_inner();
            if resp.observations.is_empty() {
                eprintln!(
                    "memlayer: session {session_id} has no observations — nothing to summarize"
                );
                return Ok(());
            }
            render_rollup(&session_id, &resp.observations)
        }
    };

    // Save with topic_key="session-summary/<id>" so re-runs supersede in place.
    let topic = format!("session-summary/{session_id}");
    let req = p::SaveObservationRequest {
        project_name: project_name.to_string(),
        sync_id: None,
        session_id: session_id.clone(),
        r#type: "note".into(),
        title: format!("Session summary {session_id}"),
        content: body,
        tool_name: None,
        scope: "project".into(),
        created_by: Some(if a.auto && a.content.is_none() {
            "memlayer-summarize/auto"
        } else {
            "memlayer-summarize/agent"
        }
        .into()),
        topic_key: Some(topic),
    };
    let resp = client.save_observation(req).await?.into_inner();
    if let Some(obs) = resp.observation {
        println!("Saved session summary as observation #{}", obs.id);
    }
    for old in &resp.similar_observations {
        if old.id > 0 {
            eprintln!("  ↳ Superseded prior summary #{} (soft-deleted)", old.id);
        }
    }
    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "session.summarize",
        project: Some(project_name),
        result_count: Some(1),
        duration_ms: started.elapsed().as_millis(),
        query: None,
        top_hits: None,
    });
    Ok(())
}

/// Group observations by topic_key (fallback: lowercase-stripped title), pick
/// the latest non-deleted per group, partition by `type`, and render an
/// Engram-shaped markdown body.
fn render_rollup(session_id: &str, obs: &[p::Observation]) -> String {
    use std::collections::BTreeMap;

    // Group by (topic_key or title-key) → latest observation.
    let mut by_topic: BTreeMap<String, &p::Observation> = BTreeMap::new();
    for o in obs {
        let key = o
            .topic_key
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                o.title
                    .trim()
                    .to_lowercase()
                    .chars()
                    .filter(|c| !c.is_whitespace() || *c == ' ')
                    .collect()
            });
        // Skip prior session summaries — don't fold them back in.
        if key.starts_with("session-summary/") {
            continue;
        }
        match by_topic.get(&key) {
            Some(existing) if existing.created_at >= o.created_at => {}
            _ => {
                by_topic.insert(key, o);
            }
        }
    }
    let latest: Vec<&p::Observation> = by_topic.into_values().collect();

    let mut by_type: BTreeMap<&str, Vec<&p::Observation>> = BTreeMap::new();
    for o in &latest {
        by_type.entry(o.r#type.as_str()).or_default().push(o);
    }

    let one_line = |s: &str| -> String {
        s.lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .chars()
            .take(160)
            .collect()
    };

    let mut out = String::new();
    out.push_str(&format!("## Goal\nSession {session_id}\n\n"));

    let section = |title: &str, key: &str, by_type: &BTreeMap<&str, Vec<&p::Observation>>| -> String {
        let mut s = String::new();
        if let Some(items) = by_type.get(key) {
            if !items.is_empty() {
                s.push_str(&format!("## {title}\n"));
                for o in items {
                    s.push_str(&format!("- [#{}] {} — {}\n", o.id, o.title, one_line(&o.content)));
                }
                s.push('\n');
            }
        }
        s
    };

    out.push_str(&section("Decisions", "decision", &by_type));
    out.push_str(&section("Patterns", "pattern", &by_type));
    out.push_str(&section("Fixes", "fix", &by_type));
    out.push_str(&section("Feedback", "feedback", &by_type));
    out.push_str(&section("Notes", "note", &by_type));

    out
}

async fn list(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionListArgs,
) -> Result<(), tonic::Status> {
    let req = p::ListSessionsRequest {
        project_name: project_name.to_string(),
        limit: a.limit,
        cursor: a.cursor.map(|t| p::Cursor { token: t }),
    };
    let resp = client.list_sessions(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn get(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionGetArgs,
) -> Result<(), tonic::Status> {
    let req = p::GetSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
    };
    let resp = client.get_session(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn delete(
    client: &mut Client,
    project_name: &str,
    a: SessionDeleteArgs,
) -> Result<(), tonic::Status> {
    let req = p::DeleteSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
    };
    client.delete_session(req).await?;
    Ok(())
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    use std::io::Write;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}
