//! `Render` impls for the proto response types used by the obs subcommands
//! (spec2-t5).
//!
//! The proto types in `memlayer-proto` are not `serde::Serialize` (prost is
//! built without the serde feature), so each impl builds a
//! `serde_json::Value` by hand for `to_json_value`. The `render_text`
//! variants produce simple aligned tables for TTY use.
//!
//! Most observation fields are reflected verbatim into JSON — the goal is
//! that piped-to-`jq` consumers see the same shape as the underlying
//! `Observation` proto.

use std::io::{self, Write};

use memlayer_proto as p;
use serde_json::{json, Value};

use crate::formatter::Render;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn obs_to_json(o: &p::Observation) -> Value {
    json!({
        "id": o.id,
        "sync_id": o.sync_id,
        "session_id": o.session_id,
        "type": o.r#type,
        "title": o.title,
        "content": o.content,
        "tool_name": o.tool_name,
        "scope": o.scope,
        "created_by": o.created_by,
        "topic_key": o.topic_key,
        "normalized_hash": o.normalized_hash,
        "revision_count": o.revision_count,
        "duplicate_count": o.duplicate_count,
        "last_seen_at": o.last_seen_at,
        "created_at": o.created_at,
        "updated_at": o.updated_at,
        "deleted_at": o.deleted_at,
        "review_after": o.review_after,
        "project_name": o.project_name,
        "code_anchor": o.code_anchor,
    })
}

/// Single-row text rendering used for `obs get`, the `Save` response, etc.
fn write_observation_detail(o: &p::Observation, w: &mut dyn Write) -> io::Result<()> {
    writeln!(w, "id          {}", o.id)?;
    writeln!(w, "sync_id     {}", o.sync_id)?;
    writeln!(w, "type        {}", o.r#type)?;
    writeln!(w, "title       {}", o.title)?;
    writeln!(w, "scope       {}", o.scope)?;
    if let Some(t) = &o.topic_key {
        writeln!(w, "topic_key   {t}")?;
    }
    if let Some(a) = &o.code_anchor {
        writeln!(w, "anchor      {a}")?;
    }
    writeln!(w, "revisions   {}", o.revision_count)?;
    writeln!(w, "created_at  {}", o.created_at)?;
    writeln!(w, "updated_at  {}", o.updated_at)?;
    if let Some(d) = &o.deleted_at {
        writeln!(w, "deleted_at  {d}")?;
    }
    writeln!(w, "content     {}", o.content)?;
    Ok(())
}

/// Multi-row table layout used by Recent/List/Search.
///
/// Columns (must match the skeleton-test assertion in
/// `tests::observation_text_render_matches_columns`):
/// `ID  TYPE  SCOPE  TITLE  SNIPPET`.
fn write_observation_row(o: &p::Observation, w: &mut dyn Write) -> io::Result<()> {
    let snippet = snippet(&o.content, 40);
    writeln!(
        w,
        "{:<6}  {:<10}  {:<8}  {:<32}  {}",
        o.id,
        truncate(&o.r#type, 10),
        truncate(&o.scope, 8),
        truncate(&o.title, 32),
        snippet
    )
}

fn write_table_header(w: &mut dyn Write) -> io::Result<()> {
    writeln!(
        w,
        "{:<6}  {:<10}  {:<8}  {:<32}  {}",
        "ID", "TYPE", "SCOPE", "TITLE", "SNIPPET"
    )
}

fn snippet(s: &str, max: usize) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    truncate(&one_line, max)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------------------
// SaveObservationResponse
// ---------------------------------------------------------------------------

impl Render for p::SaveObservationResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(o) = &self.observation {
            write_observation_detail(o, w)?;
        }
        if !self.similar_observations.is_empty() {
            writeln!(w)?;
            writeln!(w, "Similar (hash-window collisions):")?;
            write_table_header(w)?;
            for o in &self.similar_observations {
                write_observation_row(o, w)?;
            }
        }
        Ok(())
    }

    fn to_json_value(&self) -> Value {
        json!({
            "observation": self.observation.as_ref().map(obs_to_json),
            "similar_observations": self
                .similar_observations
                .iter()
                .map(obs_to_json)
                .collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// GetObservationResponse / UpdateObservationResponse
// ---------------------------------------------------------------------------

impl Render for p::GetObservationResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(o) = &self.observation {
            write_observation_detail(o, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "observation": self.observation.as_ref().map(obs_to_json) })
    }
}

impl Render for p::UpdateObservationResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(o) = &self.observation {
            write_observation_detail(o, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "observation": self.observation.as_ref().map(obs_to_json) })
    }
}

// ---------------------------------------------------------------------------
// DeleteObservationResponse
// ---------------------------------------------------------------------------

impl Render for p::DeleteObservationResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "deleted")
    }
    fn to_json_value(&self) -> Value {
        json!({ "deleted": true })
    }
}

// ---------------------------------------------------------------------------
// SearchObservationsResponse / ListObservationsResponse / RecentObservationsResponse
// ---------------------------------------------------------------------------

impl Render for p::SearchObservationsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        write_table_header(w)?;
        for o in &self.observations {
            write_observation_row(o, w)?;
        }
        if let Some(warn) = &self.warning {
            writeln!(w)?;
            writeln!(w, "warning: {warn}")?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "observations": self.observations.iter().map(obs_to_json).collect::<Vec<_>>(),
            "warning": self.warning,
        })
    }
}

impl Render for p::ListObservationsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        write_table_header(w)?;
        for o in &self.observations {
            write_observation_row(o, w)?;
        }
        if let Some(c) = &self.next_cursor {
            writeln!(w)?;
            writeln!(w, "next_cursor: {}", c.token)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "observations": self.observations.iter().map(obs_to_json).collect::<Vec<_>>(),
            // `next_cursor` is rendered as a bare string token (not a
            // wrapped object) so the JSON shape matches the text output
            // (`next_cursor: <token>`) and so callers can pass it back as
            // `--cursor <token>` without unwrapping.
            "next_cursor": self.next_cursor.as_ref().map(|c| c.token.clone()),
        })
    }
}

impl Render for p::RecentObservationsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        write_table_header(w)?;
        for o in &self.observations {
            write_observation_row(o, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "observations": self.observations.iter().map(obs_to_json).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// ContextResponse / TimelineResponse / SuggestTopicKeyResponse / CapturePassiveResponse
// ---------------------------------------------------------------------------

impl Render for p::ContextResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(snap) = &self.snapshot {
            writeln!(w, "RECENT OBSERVATIONS")?;
            write_table_header(w)?;
            for o in &snap.recent_observations {
                write_observation_row(o, w)?;
            }
            if !snap.active_topics.is_empty() {
                writeln!(w)?;
                writeln!(w, "ACTIVE TOPICS")?;
                writeln!(
                    w,
                    "{:<32}  {:<8}  {:<32}  {}",
                    "TOPIC_KEY", "SCOPE", "LATEST_TITLE", "UPDATED_AT"
                )?;
                for t in &snap.active_topics {
                    writeln!(
                        w,
                        "{:<32}  {:<8}  {:<32}  {}",
                        truncate(&t.topic_key, 32),
                        truncate(&t.scope, 8),
                        truncate(&t.latest_title, 32),
                        t.updated_at
                    )?;
                }
            }
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        // Flatten the optional snapshot so callers always see
        // `recent_observations` and `active_topics` at the top level.
        // When the daemon returns no snapshot, render empty arrays
        // instead of `{"snapshot": null}` so consumers can read the
        // arrays unconditionally.
        let (recents, topics) = match &self.snapshot {
            Some(s) => (
                s.recent_observations.iter().map(obs_to_json).collect::<Vec<_>>(),
                s.active_topics
                    .iter()
                    .map(|t| {
                        json!({
                            "topic_key": t.topic_key,
                            "scope": t.scope,
                            "latest_title": t.latest_title,
                            "updated_at": t.updated_at,
                        })
                    })
                    .collect::<Vec<_>>(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        json!({
            "recent_observations": recents,
            "active_topics": topics,
        })
    }
}

impl Render for p::TimelineResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if !self.before.is_empty() {
            writeln!(w, "BEFORE")?;
            write_table_header(w)?;
            for o in &self.before {
                write_observation_row(o, w)?;
            }
            writeln!(w)?;
        }
        if let Some(o) = &self.anchor {
            writeln!(w, "ANCHOR")?;
            write_observation_detail(o, w)?;
        }
        if !self.after.is_empty() {
            writeln!(w)?;
            writeln!(w, "AFTER")?;
            write_table_header(w)?;
            for o in &self.after {
                write_observation_row(o, w)?;
            }
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "before": self.before.iter().map(obs_to_json).collect::<Vec<_>>(),
            "anchor": self.anchor.as_ref().map(obs_to_json),
            "after": self.after.iter().map(obs_to_json).collect::<Vec<_>>(),
        })
    }
}

impl Render for p::SuggestTopicKeyResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "{}", self.topic_key)
    }
    fn to_json_value(&self) -> Value {
        json!({ "topic_key": self.topic_key })
    }
}

impl Render for p::CapturePassiveResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        for snip in &self.snippets {
            writeln!(w, "- {snip}")?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "snippets": self.snippets })
    }
}

/// CLI-side aggregate for `obs capture-passive`: wraps the daemon's
/// snippet list with the observations actually persisted by the
/// per-snippet SaveObservation fan-out (FR12.6 / SC-15).
pub struct CapturePassiveOutcome {
    pub response: p::CapturePassiveResponse,
    pub saved: Vec<p::Observation>,
}

impl Render for CapturePassiveOutcome {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.saved.is_empty() {
            writeln!(w, "no key learnings found")?;
            return Ok(());
        }
        writeln!(w, "saved {} observation(s):", self.saved.len())?;
        write_table_header(w)?;
        for o in &self.saved {
            write_observation_row(o, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "snippets": self.response.snippets,
            "saved": self.saved.iter().map(obs_to_json).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

fn session_to_json(s: &p::Session) -> Value {
    json!({
        "id": s.id,
        "directory": s.directory,
        "started_at": s.started_at,
        "ended_at": s.ended_at,
        "summary": s.summary,
    })
}

fn write_session_detail(s: &p::Session, w: &mut dyn Write) -> io::Result<()> {
    writeln!(w, "id          {}", s.id)?;
    writeln!(w, "directory   {}", s.directory)?;
    writeln!(w, "started_at  {}", s.started_at)?;
    if let Some(t) = &s.ended_at {
        writeln!(w, "ended_at    {t}")?;
    }
    if let Some(t) = &s.summary {
        writeln!(w, "summary     {t}")?;
    }
    Ok(())
}

fn write_session_table_header(w: &mut dyn Write) -> io::Result<()> {
    writeln!(
        w,
        "{:<24}  {:<20}  {:<20}  {}",
        "ID", "STARTED_AT", "ENDED_AT", "DIRECTORY"
    )
}

fn write_session_row(s: &p::Session, w: &mut dyn Write) -> io::Result<()> {
    writeln!(
        w,
        "{:<24}  {:<20}  {:<20}  {}",
        truncate(&s.id, 24),
        truncate(&s.started_at, 20),
        s.ended_at.as_deref().map(|t| truncate(t, 20)).unwrap_or_else(|| "-".to_string()),
        truncate(&s.directory, 40)
    )
}

impl Render for p::StartSessionResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(s) = &self.session {
            write_session_detail(s, w)?;
        }
        if let Some(snap) = &self.context {
            writeln!(w)?;
            writeln!(w, "INLINE CONTEXT — {} recent observations", snap.recent_observations.len())?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        let snap = self.context.as_ref().map(|s| {
            json!({
                "recent_observations": s.recent_observations.iter().map(obs_to_json).collect::<Vec<_>>(),
                "active_topics": s
                    .active_topics
                    .iter()
                    .map(|t| json!({
                        "topic_key": t.topic_key,
                        "scope": t.scope,
                        "latest_title": t.latest_title,
                        "updated_at": t.updated_at,
                    }))
                    .collect::<Vec<_>>(),
            })
        });
        json!({
            "session": self.session.as_ref().map(session_to_json),
            "context": snap,
        })
    }
}

impl Render for p::EndSessionResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(s) = &self.session {
            write_session_detail(s, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "session": self.session.as_ref().map(session_to_json) })
    }
}

impl Render for p::SaveSessionSummaryResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(s) = &self.session {
            write_session_detail(s, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "session": self.session.as_ref().map(session_to_json) })
    }
}

impl Render for p::GetSessionResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(s) = &self.session {
            write_session_detail(s, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "session": self.session.as_ref().map(session_to_json) })
    }
}

impl Render for p::ListSessionsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        write_session_table_header(w)?;
        for s in &self.sessions {
            write_session_row(s, w)?;
        }
        if let Some(c) = &self.next_cursor {
            writeln!(w)?;
            writeln!(w, "next_cursor: {}", c.token)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "sessions": self.sessions.iter().map(session_to_json).collect::<Vec<_>>(),
            "next_cursor": self.next_cursor.as_ref().map(|c| c.token.clone()),
        })
    }
}

impl Render for p::DeleteSessionResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "deleted")
    }
    fn to_json_value(&self) -> Value {
        json!({ "deleted": true })
    }
}

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

fn prompt_to_json(p: &p::Prompt) -> Value {
    json!({
        "id": p.id,
        "sync_id": p.sync_id,
        "session_id": p.session_id,
        "content": p.content,
        "created_at": p.created_at,
    })
}

fn write_prompt_detail(p: &p::Prompt, w: &mut dyn Write) -> io::Result<()> {
    writeln!(w, "id          {}", p.id)?;
    writeln!(w, "sync_id     {}", p.sync_id)?;
    writeln!(w, "session_id  {}", p.session_id)?;
    writeln!(w, "created_at  {}", p.created_at)?;
    writeln!(w, "content     {}", p.content)?;
    Ok(())
}

fn write_prompt_table_header(w: &mut dyn Write) -> io::Result<()> {
    writeln!(
        w,
        "{:<6}  {:<24}  {:<20}  {}",
        "ID", "SESSION", "CREATED_AT", "CONTENT"
    )
}

fn write_prompt_row(p: &p::Prompt, w: &mut dyn Write) -> io::Result<()> {
    writeln!(
        w,
        "{:<6}  {:<24}  {:<20}  {}",
        p.id,
        truncate(&p.session_id, 24),
        truncate(&p.created_at, 20),
        snippet(&p.content, 60),
    )
}

impl Render for p::SavePromptResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if let Some(pr) = &self.prompt {
            write_prompt_detail(pr, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "prompt": self.prompt.as_ref().map(prompt_to_json) })
    }
}

impl Render for p::SearchPromptsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        write_prompt_table_header(w)?;
        for pr in &self.prompts {
            write_prompt_row(pr, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "prompts": self.prompts.iter().map(prompt_to_json).collect::<Vec<_>>() })
    }
}

impl Render for p::RecentPromptsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        write_prompt_table_header(w)?;
        for pr in &self.prompts {
            write_prompt_row(pr, w)?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "prompts": self.prompts.iter().map(prompt_to_json).collect::<Vec<_>>() })
    }
}

impl Render for p::DeletePromptResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "deleted")
    }
    fn to_json_value(&self) -> Value {
        json!({ "deleted": true })
    }
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

impl Render for p::ListProjectsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "{:<24}  {:<24}  {:<6}  {:<6}  {:<6}  {}",
            "NAME", "DISPLAY", "OBS", "SES", "PRM", "CREATED_AT"
        )?;
        for proj in &self.projects {
            writeln!(
                w,
                "{:<24}  {:<24}  {:<6}  {:<6}  {:<6}  {}",
                truncate(&proj.normalized_name, 24),
                truncate(&proj.display_name, 24),
                proj.observation_count,
                proj.session_count,
                proj.prompt_count,
                proj.created_at
            )?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "projects": self.projects.iter().map(|p| json!({
                "normalized_name": p.normalized_name,
                "display_name": p.display_name,
                "observation_count": p.observation_count,
                "session_count": p.session_count,
                "prompt_count": p.prompt_count,
                "created_at": p.created_at,
            })).collect::<Vec<_>>(),
        })
    }
}

impl Render for p::CurrentProjectResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "name      {}", self.normalized_name)?;
        writeln!(w, "display   {}", self.display_name)?;
        writeln!(w, "source    {}", self.source)?;
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "normalized_name": self.normalized_name,
            "display_name": self.display_name,
            "source": self.source,
        })
    }
}

impl Render for p::MergeProjectsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "observations migrated: {}", self.observations_migrated)?;
        writeln!(w, "sessions migrated:     {}", self.sessions_migrated)?;
        writeln!(w, "prompts migrated:      {}", self.prompts_migrated)?;
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "observations_migrated": self.observations_migrated,
            "sessions_migrated": self.sessions_migrated,
            "prompts_migrated": self.prompts_migrated,
        })
    }
}

impl Render for p::DeleteProjectResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "deleted")
    }
    fn to_json_value(&self) -> Value {
        json!({ "deleted": true })
    }
}

impl Render for p::ConsolidateProjectsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "{:<32}  {:<32}  {}",
            "FROM", "TO", "SIMILARITY"
        )?;
        for c in &self.candidates {
            writeln!(
                w,
                "{:<32}  {:<32}  {:.3}",
                truncate(&c.from, 32),
                truncate(&c.to, 32),
                c.similarity
            )?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "candidates": self.candidates.iter().map(|c| json!({
                "from": c.from,
                "to": c.to,
                "similarity": c.similarity,
            })).collect::<Vec<_>>(),
        })
    }
}

impl Render for p::PruneProjectsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        for n in &self.would_remove {
            writeln!(w, "- {n}")?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "would_remove": self.would_remove })
    }
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

impl Render for p::SyncStatusResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "last_export_at       {}", self.last_export_at.as_deref().unwrap_or("-"))?;
        writeln!(w, "last_import_at       {}", self.last_import_at.as_deref().unwrap_or("-"))?;
        writeln!(w, "unseen_chunk_count   {}", self.unseen_chunk_count)?;
        writeln!(w, "last_error           {}", self.last_error.as_deref().unwrap_or("-"))?;
        writeln!(w, "total_exported       {}", self.total_exported_chunks)?;
        writeln!(w, "total_imported       {}", self.total_imported_chunks)?;
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "last_export_at": self.last_export_at,
            "last_import_at": self.last_import_at,
            "unseen_chunk_count": self.unseen_chunk_count,
            "last_error": self.last_error,
            "total_exported_chunks": self.total_exported_chunks,
            "total_imported_chunks": self.total_imported_chunks,
        })
    }
}

// ---------------------------------------------------------------------------
// Daemon (status), Tokens (FR8, FR10) — added in spec2-t7
// ---------------------------------------------------------------------------

impl Render for p::DaemonStatusResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "version          {}", self.version)?;
        writeln!(w, "pid              {}", self.pid)?;
        writeln!(w, "started_at       {}", self.started_at)?;
        writeln!(w, "read_only_mode   {}", self.read_only_mode)?;
        writeln!(w, "in_flight_rpcs   {}", self.in_flight_rpcs)?;
        writeln!(w, "cached_projects  {}", self.cached_projects)?;
        writeln!(w, "cache_hit_ratio  {:.3}", self.cache_hit_ratio)?;
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "version": self.version,
            "pid": self.pid,
            "started_at": self.started_at,
            "read_only_mode": self.read_only_mode,
            "in_flight_rpcs": self.in_flight_rpcs,
            "cached_projects": self.cached_projects,
            "cache_hit_ratio": self.cache_hit_ratio,
        })
    }
}

impl Render for p::CreateTokenResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        // SC-21: token-create prints the 64-hex token to stdout, exactly once.
        // Never log this string — it's the only copy of the secret.
        writeln!(w, "{}", self.secret)
    }
    fn to_json_value(&self) -> Value {
        json!({ "secret": self.secret })
    }
}

impl Render for p::ListTokensResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "{:<24}  {:<5}  {:<20}  {}",
            "NAME", "ADMIN", "CREATED_AT", "REVOKED_AT"
        )?;
        for t in &self.tokens {
            writeln!(
                w,
                "{:<24}  {:<5}  {:<20}  {}",
                truncate(&t.name, 24),
                t.is_admin,
                truncate(&t.created_at, 20),
                t.revoked_at.as_deref().unwrap_or("-")
            )?;
        }
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({
            "tokens": self.tokens.iter().map(|t| json!({
                "name": t.name,
                "is_admin": t.is_admin,
                "created_at": t.created_at,
                "revoked_at": t.revoked_at,
            })).collect::<Vec<_>>(),
        })
    }
}

impl Render for p::RevokeTokenResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "revoked")
    }
    fn to_json_value(&self) -> Value {
        json!({ "revoked": true })
    }
}

// ---------------------------------------------------------------------------
// Atomic facts (rp-t10).
// ---------------------------------------------------------------------------

fn fact_to_json(f: &p::Fact) -> Value {
    json!({
        "id": f.id,
        "obs_id": f.obs_id,
        "subject": f.subject,
        "predicate": f.predicate,
        "object": f.object,
        "temporal": f.temporal,
        "salience": f.salience,
        "superseded_by": f.superseded_by,
        "extracted_by": f.extracted_by,
        "extracted_at": f.extracted_at,
    })
}

impl Render for p::GetFactsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.facts.is_empty() {
            writeln!(w, "(no facts)")?;
            return Ok(());
        }
        let obs_id = self.facts[0].obs_id;
        writeln!(w, "Facts for observation #{obs_id}:")?;
        for f in &self.facts {
            let temporal = f
                .temporal
                .as_deref()
                .map(|t| format!(" @ {t}"))
                .unwrap_or_default();
            let superseded = if f.superseded_by.is_some() {
                " [superseded]"
            } else {
                ""
            };
            writeln!(
                w,
                "  - ({}, {}, {}){} [extracted by {}, {}]{}",
                f.subject, f.predicate, f.object, temporal, f.extracted_by, f.extracted_at,
                superseded,
            )?;
        }
        Ok(())
    }

    fn to_json_value(&self) -> Value {
        json!({
            "facts": self.facts.iter().map(fact_to_json).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Observation history (Spec 5).
// ---------------------------------------------------------------------------

impl Render for p::GetObservationHistoryResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.entries.is_empty() {
            writeln!(w, "(no history found)")?;
            return Ok(());
        }
        writeln!(w, "Observation History Lineage (oldest → newest):")?;
        let len = self.entries.len();
        for (i, entry) in self.entries.iter().enumerate() {
            let obs = match &entry.observation {
                Some(o) => o,
                None => continue,
            };
            let branch = if i == len - 1 { "└── " } else { "├── " };
            let status = if obs.deleted_at.is_some() {
                format!("[superseded {}]", obs.deleted_at.as_deref().unwrap_or(""))
            } else {
                "[active]".to_string()
            };
            writeln!(
                w,
                "{}#{} ({}) {:?} {}",
                branch, obs.id, obs.r#type, obs.title, status,
            )?;
            if let Some(sup_id) = entry.superseded_by_id {
                let pad = if i == len - 1 { "    " } else { "│   " };
                writeln!(w, "{}↑ superseded by #{}", pad, sup_id)?;
            }
        }
        Ok(())
    }

    fn to_json_value(&self) -> Value {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                json!({
                    "observation": e.observation.as_ref().map(obs_to_json),
                    "superseded_by_id": e.superseded_by_id,
                })
            })
            .collect();
        json!({ "entries": entries })
    }
}

// ---------------------------------------------------------------------------
// Observation relations (Phase 4).
// ---------------------------------------------------------------------------

impl Render for p::GetObservationRelationsResponse {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.relations.is_empty() {
            writeln!(w, "(no relations found)")?;
            return Ok(());
        }
        writeln!(
            w,
            "{:<6}  {:<6}  {:<16}  {:<10}  CREATED_AT",
            "SOURCE", "TARGET", "RELATION", "CONFIDENCE"
        )?;
        writeln!(w, "{}", "-".repeat(60))?;
        for r in &self.relations {
            writeln!(
                w,
                "{:<6}  {:<6}  {:<16}  {:<10.2}  {}",
                r.source_id, r.target_id, r.relation_type, r.confidence, r.created_at
            )?;
        }
        Ok(())
    }

    fn to_json_value(&self) -> Value {
        json!({
            "relations": self.relations.iter().map(|r| json!({
                "id": r.id,
                "source_id": r.source_id,
                "target_id": r.target_id,
                "relation_type": r.relation_type,
                "confidence": r.confidence,
                "created_at": r.created_at,
            })).collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_observation() -> p::Observation {
        p::Observation {
            id: 42,
            sync_id: "sync-abc".to_string(),
            session_id: "s1".to_string(),
            r#type: "decision".to_string(),
            title: "Use jaro_winkler for project consolidate".to_string(),
            content: "Picked strsim::jaro_winkler over Levenshtein.".to_string(),
            tool_name: None,
            scope: "project".to_string(),
            created_by: None,
            topic_key: Some("decision/consolidate-similarity".to_string()),
            normalized_hash: Some("h".to_string()),
            revision_count: 2,
            duplicate_count: 1,
            last_seen_at: None,
            created_at: "2026-06-03T10:00:00Z".to_string(),
            updated_at: "2026-06-03T10:00:00Z".to_string(),
            deleted_at: None,
            review_after: None,
            project_name: None,
            code_anchor: None,
        }
    }

    #[test]
    fn observation_text_render_matches_columns() {
        let resp = p::RecentObservationsResponse {
            observations: vec![sample_observation()],
        };
        let mut buf: Vec<u8> = Vec::new();
        resp.render_text(&mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Header row carries the canonical column names.
        let first_line = s.lines().next().unwrap();
        for col in ["ID", "TYPE", "SCOPE", "TITLE", "SNIPPET"] {
            assert!(
                first_line.contains(col),
                "header '{first_line}' missing column '{col}'"
            );
        }
        // Data row contains the observation's id, type, scope, and title.
        let body = s.lines().nth(1).unwrap();
        assert!(body.contains("42"));
        assert!(body.contains("decision"));
        assert!(body.contains("project"));
        assert!(body.contains("Use jaro_winkler"));
    }

    #[test]
    fn observation_json_render_round_trips() {
        let resp = p::GetObservationResponse {
            observation: Some(sample_observation()),
        };
        let v = resp.to_json_value();
        assert_eq!(v["observation"]["id"], 42);
        assert_eq!(v["observation"]["sync_id"], "sync-abc");
        assert_eq!(v["observation"]["type"], "decision");
        assert_eq!(v["observation"]["scope"], "project");
    }

    #[test]
    fn delete_response_renders_brief() {
        let r = p::DeleteObservationResponse {};
        let mut buf = Vec::new();
        r.render_text(&mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "deleted\n");
    }

    #[test]
    fn search_response_warning_appears_in_text() {
        let r = p::SearchObservationsResponse {
            observations: vec![],
            warning: Some("capped at 32 projects".to_string()),
        };
        let mut buf = Vec::new();
        r.render_text(&mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("warning: capped"));
    }

    #[test]
    fn capture_passive_renders_bullets() {
        let r = p::CapturePassiveResponse {
            snippets: vec!["alpha".to_string(), "beta".to_string()],
        };
        let mut buf = Vec::new();
        r.render_text(&mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "- alpha\n- beta\n");
    }

    #[test]
    fn list_response_json_shape_pins_cursor_as_string() {
        // TS-21 pinned: `next_cursor` must be a bare string (or null), not
        // a `{"token": …}` object. CLI consumers feed the value back in via
        // `--cursor`, which expects the raw token. Wrapping it in an
        // object silently breaks pagination loops that read
        // `v["next_cursor"].as_str()`.
        let resp_with = p::ListObservationsResponse {
            observations: vec![sample_observation()],
            next_cursor: Some(p::Cursor { token: "abc123".into() }),
        };
        let v = resp_with.to_json_value();
        assert_eq!(v["next_cursor"], json!("abc123"));
        assert!(v["observations"].is_array());

        let resp_without = p::ListObservationsResponse {
            observations: vec![],
            next_cursor: None,
        };
        let v = resp_without.to_json_value();
        assert!(v["next_cursor"].is_null(), "absent cursor must render as null");
    }

    #[test]
    fn context_response_json_shape_is_flat() {
        // TS-9 pinned: `recent_observations` and `active_topics` must sit
        // at the top level of the `obs context` JSON output, not nested
        // under a `snapshot` key. Callers reading the response should not
        // have to unwrap an optional snapshot — when the daemon returns
        // no snapshot we emit empty arrays instead of `{"snapshot": null}`.
        let snap = p::ContextSnapshot {
            recent_observations: vec![sample_observation()],
            active_topics: vec![p::TopicSummary {
                topic_key: "k".into(),
                scope: "project".into(),
                latest_title: "t".into(),
                updated_at: "2026-06-03T10:00:00Z".into(),
            }],
        };
        let resp = p::ContextResponse { snapshot: Some(snap) };
        let v = resp.to_json_value();
        assert!(v["recent_observations"].is_array());
        assert_eq!(v["recent_observations"].as_array().unwrap().len(), 1);
        assert!(v["active_topics"].is_array());
        assert!(v["snapshot"].is_null(), "snapshot wrapper must not appear");

        // Empty-snapshot path: arrays still present, not null.
        let resp_empty = p::ContextResponse { snapshot: None };
        let v = resp_empty.to_json_value();
        assert_eq!(v["recent_observations"], json!([]));
        assert_eq!(v["active_topics"], json!([]));
    }

    #[test]
    fn get_facts_text_renders_triple_with_metadata() {
        let resp = p::GetFactsResponse {
            facts: vec![
                p::Fact {
                    id: 1,
                    obs_id: 42,
                    subject: "team".into(),
                    predicate: "prefers".into(),
                    object: "raw SQL via pgx".into(),
                    temporal: None,
                    salience: 0.9,
                    superseded_by: None,
                    extracted_by: "haiku".into(),
                    extracted_at: "2026-06-18T00:00:00Z".into(),
                },
                p::Fact {
                    id: 2,
                    obs_id: 42,
                    subject: "team".into(),
                    predicate: "rejected".into(),
                    object: "GORM".into(),
                    temporal: Some("2025-Q4".into()),
                    salience: 0.5,
                    superseded_by: None,
                    extracted_by: "haiku".into(),
                    extracted_at: "2026-06-18T00:00:01Z".into(),
                },
            ],
        };
        let mut out = Vec::new();
        resp.render_text(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Facts for observation #42"), "got: {s}");
        assert!(s.contains("(team, prefers, raw SQL via pgx)"));
        assert!(s.contains("(team, rejected, GORM) @ 2025-Q4"));
        assert!(s.contains("haiku"));
    }

    #[test]
    fn get_facts_text_handles_empty() {
        let resp = p::GetFactsResponse { facts: vec![] };
        let mut out = Vec::new();
        resp.render_text(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("(no facts)"), "got: {s}");
    }

    #[test]
    fn get_facts_json_includes_all_fields() {
        let resp = p::GetFactsResponse {
            facts: vec![p::Fact {
                id: 1,
                obs_id: 42,
                subject: "x".into(),
                predicate: "y".into(),
                object: "z".into(),
                temporal: Some("now".into()),
                salience: 0.7,
                superseded_by: Some(99),
                extracted_by: "sonnet".into(),
                extracted_at: "2026-06-18T00:00:00Z".into(),
            }],
        };
        let v = resp.to_json_value();
        let f = &v["facts"][0];
        assert_eq!(f["id"], 1);
        assert_eq!(f["obs_id"], 42);
        assert_eq!(f["subject"], "x");
        assert_eq!(f["temporal"], "now");
        assert_eq!(f["salience"], 0.7);
        assert_eq!(f["superseded_by"], 99);
        assert_eq!(f["extracted_by"], "sonnet");
    }

    #[test]
    fn get_facts_text_marks_superseded() {
        let resp = p::GetFactsResponse {
            facts: vec![p::Fact {
                id: 5,
                obs_id: 9,
                subject: "team".into(),
                predicate: "prefers".into(),
                object: "diesel".into(),
                temporal: None,
                salience: 0.5,
                superseded_by: Some(7),
                extracted_by: "haiku".into(),
                extracted_at: "2026-06-18T00:00:00Z".into(),
            }],
        };
        let mut out = Vec::new();
        resp.render_text(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("[superseded]"), "got: {s}");
    }

    fn sample_obs_with_id(id: i64, deleted: bool) -> p::Observation {
        p::Observation {
            id,
            sync_id: format!("sync-{id}"),
            session_id: "s1".into(),
            r#type: "decision".into(),
            title: format!("title-{id}"),
            content: "body".into(),
            tool_name: None,
            scope: "project".into(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 1,
            duplicate_count: 0,
            last_seen_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            deleted_at: if deleted { Some("2026-06-01T00:00:00Z".into()) } else { None },
            review_after: None,
            project_name: None,
            code_anchor: None,
        }
    }

    #[test]
    fn history_text_renders_chain_oldest_to_newest() {
        let resp = p::GetObservationHistoryResponse {
            entries: vec![
                p::ObservationHistoryEntry {
                    observation: Some(sample_obs_with_id(1, true)),
                    superseded_by_id: Some(2),
                },
                p::ObservationHistoryEntry {
                    observation: Some(sample_obs_with_id(2, false)),
                    superseded_by_id: None,
                },
            ],
        };
        let mut out = Vec::new();
        resp.render_text(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("#1"), "got: {s}");
        assert!(s.contains("superseded"), "got: {s}");
        assert!(s.contains("#2"), "got: {s}");
        assert!(s.contains("[active]"), "got: {s}");
    }

    #[test]
    fn history_json_includes_entries_and_superseded_by_id() {
        let resp = p::GetObservationHistoryResponse {
            entries: vec![
                p::ObservationHistoryEntry {
                    observation: Some(sample_obs_with_id(10, false)),
                    superseded_by_id: Some(11),
                },
            ],
        };
        let v = resp.to_json_value();
        assert_eq!(v["entries"][0]["superseded_by_id"], 11);
        assert_eq!(v["entries"][0]["observation"]["id"], 10);
    }

    #[test]
    fn history_text_empty() {
        let resp = p::GetObservationHistoryResponse { entries: vec![] };
        let mut out = Vec::new();
        resp.render_text(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("no history"), "got: {s}");
    }
}
