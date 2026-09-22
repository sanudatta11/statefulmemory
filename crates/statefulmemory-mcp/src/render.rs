//! Structured JSON rendering for MCP tool results.
//!
//! Tools return compact JSON (not prose) so agents can branch on fields.
//! Content bodies are truncated to keep tool payloads small.

use statefulmemory_proto::{Fact, GraphEntity, GraphQueryResponse, Observation};
use serde_json::{json, Value};

/// Max characters of observation `content` included in list/search hits.
const SNIPPET_CHARS: usize = 240;

/// Compact observation for search/recent lists.
pub fn observation_hit(obs: &Observation) -> Value {
    json!({
        "id": obs.id,
        "type": obs.r#type,
        "title": obs.title,
        "scope": obs.scope,
        "topic_key": obs.topic_key,
        "snippet": truncate(&obs.content, SNIPPET_CHARS),
        "created_at": obs.created_at,
        "project_name": obs.project_name,
        "supersedes_ids": obs.supersedes_ids,
        "superseded_count": obs.superseded_count,
        "verify_state": obs.verify_state,
    })
}

/// Fuller observation for context briefing rows.
pub fn observation_brief(obs: &Observation) -> Value {
    json!({
        "id": obs.id,
        "type": obs.r#type,
        "title": obs.title,
        "scope": obs.scope,
        "topic_key": obs.topic_key,
        "content": truncate(&obs.content, SNIPPET_CHARS * 2),
        "created_at": obs.created_at,
        "updated_at": obs.updated_at,
        "supersedes_ids": obs.supersedes_ids,
        "superseded_count": obs.superseded_count,
        "verify_state": obs.verify_state,
    })
}

pub fn fact_row(fact: &Fact) -> Value {
    json!({
        "id": fact.id,
        "obs_id": fact.obs_id,
        "subject": fact.subject,
        "predicate": fact.predicate,
        "object": fact.object,
        "temporal": fact.temporal,
        "salience": fact.salience,
        "extracted_by": fact.extracted_by,
    })
}

fn entity_label(e: &GraphEntity) -> String {
    format!("{}:{}", e.kind, e.name)
}

pub fn graph_view(resp: &GraphQueryResponse) -> Value {
    let by_id: std::collections::HashMap<i64, &GraphEntity> =
        resp.entities.iter().map(|e| (e.id, e)).collect();
    let label = |id: i64| -> String {
        by_id
            .get(&id)
            .map(|e| entity_label(e))
            .unwrap_or_else(|| id.to_string())
    };
    let seed = resp.entities.first().map(entity_label);
    let edges = resp
        .edges
        .iter()
        .map(|e| {
            let mut row = json!({
                "from": label(e.from_id),
                "relation": e.relation,
                "to": label(e.to_id),
                "weight": e.weight,
            });
            if e.src_observation_id != 0 {
                row["src_observation_id"] = json!(e.src_observation_id);
            }
            row
        })
        .collect::<Vec<_>>();
    let mut text = format!(
        "memory_graph_query: {} edges from {}",
        resp.edges.len(),
        seed.as_deref().unwrap_or("unknown")
    );
    for e in &resp.edges {
        let line = format!(
            "\n{} --[{} w={:.2}]--> {}{}",
            label(e.from_id),
            e.relation,
            e.weight,
            label(e.to_id),
            if e.src_observation_id != 0 {
                format!(" (obs {})", e.src_observation_id)
            } else {
                String::new()
            }
        );
        text.push_str(&line);
    }
    json!({
        "seed": seed,
        "edges": edges,
        "header": text,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_adds_ellipsis() {
        let s = "a".repeat(300);
        let t = truncate(&s, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn observation_hit_shape() {
        let obs = Observation {
            id: 7,
            r#type: "decision".into(),
            title: "use pgx".into(),
            content: "Team prefers raw SQL".into(),
            scope: "project".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            ..Default::default()
        };
        let v = observation_hit(&obs);
        assert_eq!(v["id"], 7);
        assert_eq!(v["type"], "decision");
        assert_eq!(v["snippet"], "Team prefers raw SQL");
    }
}
