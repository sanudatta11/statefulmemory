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

fn obs_to_json(o: &p::Observation) -> Value {
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
            "next_cursor": self.next_cursor.as_ref().map(|c| json!({ "token": c.token })),
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
        let snap = self.snapshot.as_ref().map(|s| {
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
        json!({ "snapshot": snap })
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
}
