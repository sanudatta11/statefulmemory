//! `Decide` RPC: retrieve evidence, resolve open conflicts live, synthesize.

use std::collections::HashSet;

use sha2::{Digest, Sha256};
use tonic::Status;
use tokio::sync::oneshot;

use memlayer_proto::{DecideConflict, DecideEvidence, DecideRequest, DecideResponse};
use memlayer_retrieval::hybrid::HybridMode;
use memlayer_storage::write::{SaveObservationInput, WriteRequest};
use memlayer_storage::{read as read_q, Observation};

use crate::error_map::map;
use crate::resolve_worker::{self, extract_json_object};
use crate::service::MemlayerService;

#[derive(Debug, Clone)]
pub(crate) struct DecideParsed {
    recommendation: String,
    rationale: String,
    confidence: f32,
    evidence: Vec<(i64, String)>,
    should_record: bool,
}

#[allow(clippy::result_large_err)]
pub async fn handle(svc: &MemlayerService, req: DecideRequest) -> Result<DecideResponse, Status> {
    if req.question.trim().is_empty() {
        return Err(Status::invalid_argument("question is required"));
    }
    let k = if req.limit <= 0 {
        12
    } else {
        req.limit.clamp(1, 30)
    };
    let project = map(svc.open_project(&req.project_name))?;
    let conn = map(project.open_read_conn())?;
    let mode = svc.resolved_search_mode(req.mode.as_deref(), &req.project_name);
    let hits: Vec<Observation> = if mode == HybridMode::Hybrid {
        map(svc.hybrid_search(&conn, &req.question, None, None, k, &req.project_name))?
    } else {
        map(read_q::search(&conn, &req.question, None, None, k))?
    };
    drop(conn);

    let ids: Vec<i64> = hits.iter().map(|o| o.id).collect();
    let mut conflicts = load_conflicts(svc, &req.project_name, &ids)?;
    for c in conflicts.iter().filter(|c| c.status == "open") {
        if let Err(e) = resolve_worker::resolve_pair(
            &svc.state.claude_client,
            &svc.state.registry,
            &req.project_name,
            c.a_id,
            c.b_id,
        )
        .await
        {
            tracing::warn!(error = %e, a = c.a_id, b = c.b_id, "decide live resolve failed");
        }
    }
    conflicts = load_conflicts(svc, &req.project_name, &ids)?;

    let prompt = build_decide_prompt(&req.question, &hits, &conflicts);
    let raw = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        svc.state.claude_client.ask(&prompt, "auto"),
    )
    .await
    .map_err(|_| Status::unavailable("decide model timed out"))?
    .map_err(|e| Status::unavailable(e.to_string()))?;

    let parsed = parse_decide_json(&raw).map_err(|e| Status::internal(format!("decide parse: {e}")))?;

    let mut resolution_id = None;
    let mut wrote = false;
    if parsed.should_record && parsed.confidence >= 0.7 {
        let topic = decision_topic_key(&req.question);
        let seed_session = hits
            .first()
            .map(|o| o.session_id.clone())
            .unwrap_or_default();
        let (tx, rx) = oneshot::channel();
        map(project.write.send(WriteRequest::SaveObservation {
            input: SaveObservationInput {
                sync_id: None,
                session_id: seed_session,
                r#type: "resolution".into(),
                title: parsed.recommendation.chars().take(80).collect(),
                content: format!("{}\n\n{}", parsed.recommendation, parsed.rationale),
                tool_name: None,
                scope: "project".into(),
                created_by: Some("memlayer-decide".into()),
                topic_key: Some(topic),
                code_anchor: None,
                dedupe_window_secs: 0,
                max_content_chars: svc.state.max_content_chars,
                    skip_supersede: false,
            },
            reply: tx,
        }))?;
        let saved = rx
            .await
            .map_err(|_| Status::internal("write thread crashed"))?;
        let saved = map(saved)?;
        resolution_id = Some(saved.id);
        wrote = true;
    }

    let evidence = map_evidence(&parsed.evidence, &hits);
    Ok(DecideResponse {
        recommendation: parsed.recommendation,
        rationale: parsed.rationale,
        confidence: parsed.confidence,
        evidence,
        conflicts,
        resolution_observation_id: resolution_id,
        wrote_resolution: wrote,
    })
}

fn decision_topic_key(question: &str) -> String {
    let mut h = Sha256::new();
    h.update(question.trim().as_bytes());
    let hex = format!("{:x}", h.finalize());
    format!("decision/{}", &hex[..12])
}

#[allow(clippy::result_large_err)]
fn load_conflicts(
    svc: &MemlayerService,
    project_name: &str,
    ids: &[i64],
) -> Result<Vec<DecideConflict>, Status> {
    let project = map(svc.open_project(project_name))?;
    let conn = map(project.open_read_conn())?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        let rels = map(memlayer_storage::get_relations_for_observation(&conn, *id))?;
        for r in rels {
            if r.relation_type == "conflicts_with" {
                let a = r.source_id.min(r.target_id);
                let b = r.source_id.max(r.target_id);
                if !seen.insert((a, b)) {
                    continue;
                }
                let status = if pair_resolved(&conn, a, b) {
                    "resolved"
                } else {
                    "open"
                };
                out.push(DecideConflict {
                    a_id: a,
                    b_id: b,
                    relation: "conflicts_with".into(),
                    status: status.into(),
                });
            }
        }
    }
    Ok(out)
}

fn pair_resolved(conn: &rusqlite::Connection, a: i64, b: i64) -> bool {
    let Ok(rels_a) = memlayer_storage::get_relations_for_observation(conn, a) else {
        return false;
    };
    let Ok(rels_b) = memlayer_storage::get_relations_for_observation(conn, b) else {
        return false;
    };
    rels_a
        .iter()
        .chain(rels_b.iter())
        .any(|r| r.relation_type == "resolved_by")
}

fn build_decide_prompt(question: &str, hits: &[Observation], conflicts: &[DecideConflict]) -> String {
    let mut body = String::new();
    body.push_str("You recommend a decision from stored project memories.\n");
    body.push_str("Return JSON only:\n");
    body.push_str(
        "{\"recommendation\":\"...\",\"rationale\":\"...\",\"confidence\":0.0,\
         \"evidence\":[{\"id\":1,\"role\":\"supports\"}],\"should_record\":true}\n\n",
    );
    body.push_str("Question: ");
    body.push_str(question);
    body.push_str("\n\nMemories:\n");
    for o in hits {
        let snippet: String = o.content.chars().take(400).collect();
        body.push_str(&format!("#{} [{}] {}\n{}\n\n", o.id, o.r#type, o.title, snippet));
    }
    if !conflicts.is_empty() {
        body.push_str("Conflicts:\n");
        for c in conflicts {
            body.push_str(&format!("#{} ~ #{} ({})\n", c.a_id, c.b_id, c.status));
        }
    }
    body
}

pub(crate) fn parse_decide_json(raw: &str) -> anyhow::Result<DecideParsed> {
    let slice =
        extract_json_object(raw).ok_or_else(|| anyhow::anyhow!("no JSON object in decide output"))?;
    let v: serde_json::Value = serde_json::from_str(slice)?;
    let recommendation = v
        .get("recommendation")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let rationale = v
        .get("rationale")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let confidence = v
        .get("confidence")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0) as f32;
    let should_record = v
        .get("should_record")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let mut evidence = Vec::new();
    if let Some(arr) = v.get("evidence").and_then(|x| x.as_array()) {
        for e in arr {
            let id = e.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
            let role = e
                .get("role")
                .and_then(|x| x.as_str())
                .unwrap_or("context")
                .to_string();
            if id > 0 {
                evidence.push((id, role));
            }
        }
    }
    Ok(DecideParsed {
        recommendation,
        rationale,
        confidence,
        evidence,
        should_record,
    })
}

fn map_evidence(parsed: &[(i64, String)], hits: &[Observation]) -> Vec<DecideEvidence> {
    parsed
        .iter()
        .map(|(id, role)| {
            let hit = hits.iter().find(|o| o.id == *id);
            DecideEvidence {
                observation_id: *id,
                title: hit.map(|o| o.title.clone()).unwrap_or_default(),
                content: hit
                    .map(|o| o.content.chars().take(400).collect())
                    .unwrap_or_default(),
                role: role.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decide_fills_defaults() {
        let v = parse_decide_json(
            r#"{"recommendation":"Stay on SQLite","rationale":"Local first","confidence":0.82,"should_record":true,"evidence":[{"id":12}]}"#,
        )
        .unwrap();
        assert_eq!(v.recommendation, "Stay on SQLite");
        assert!((v.confidence - 0.82).abs() < 0.001);
        assert_eq!(v.evidence[0], (12, "context".into()));
        assert!(v.should_record);
    }

    #[test]
    fn topic_key_is_stable_prefix() {
        let a = decision_topic_key("Should we keep SQLite?");
        let b = decision_topic_key("Should we keep SQLite?");
        assert_eq!(a, b);
        assert!(a.starts_with("decision/"));
        assert_eq!(a.len(), "decision/".len() + 12);
    }
}
