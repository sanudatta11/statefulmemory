//! Async resolution worker — JSON judge on `conflicts_with` pairs
//! off the save hot path.
//!
//! Same pool shape as [`crate::extract_worker`]: bounded channel, OS threads,
//! current-thread tokio, `try_queue` drops on full.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tokio::sync::oneshot;

use statefulmemory_core::config;
use statefulmemory_extract::claude_cli::{ClaudeCliClient, ClaudeClient};
use statefulmemory_storage::write::{ObservationKey, SaveObservationInput, WriteRequest};
use statefulmemory_storage::{read as read_q, ProjectRegistry};

use crate::extract_worker::QueueResult;

const QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct ResolveJob {
    pub project: String,
    pub old_id: i64,
    pub new_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveAction {
    KeepNew,
    KeepOld,
    KeepBoth,
    Synthesize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolveVerdict {
    pub action: ResolveAction,
    pub statement: String,
    pub confidence: f64,
}

#[derive(Clone)]
pub struct ResolveWorkerPool {
    tx: Sender<ResolveJob>,
}

impl ResolveWorkerPool {
    pub fn spawn(
        client: Arc<dyn ClaudeClient>,
        registry: Arc<ProjectRegistry>,
        n_workers: usize,
    ) -> Self {
        let (tx, rx) = bounded(QUEUE_CAPACITY);
        let n = n_workers.max(1);
        for i in 0..n {
            let rx = rx.clone();
            let client = client.clone();
            let registry = registry.clone();
            thread::Builder::new()
                .name(format!("statefulmemory-resolve-{i}"))
                .spawn(move || run_loop(rx, client, registry))
                .expect("spawn resolve worker thread");
        }
        Self { tx }
    }

    pub fn spawn_with_real_client(registry: Arc<ProjectRegistry>, n_workers: usize) -> Self {
        Self::spawn(Arc::new(ClaudeCliClient::new()), registry, n_workers)
    }

    pub fn try_queue(&self, job: ResolveJob) -> QueueResult {
        match self.tx.try_send(job) {
            Ok(_) => QueueResult::Queued,
            Err(TrySendError::Full(j)) => {
                tracing::warn!(
                    old_id = j.old_id,
                    new_id = j.new_id,
                    project = %j.project,
                    "resolve queue full — dropping job (capacity={QUEUE_CAPACITY})"
                );
                QueueResult::Dropped
            }
            Err(TrySendError::Disconnected(_)) => QueueResult::Disconnected,
        }
    }
}

fn run_loop(
    rx: Receiver<ResolveJob>,
    client: Arc<dyn ClaudeClient>,
    registry: Arc<ProjectRegistry>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "resolve worker tokio runtime init failed; thread exiting");
            return;
        }
    };
    while let Ok(job) = rx.recv() {
        if let Err(e) = rt.block_on(resolve_pair(
            &client,
            &registry,
            &job.project,
            job.old_id,
            job.new_id,
        )) {
            tracing::warn!(
                old_id = job.old_id,
                new_id = job.new_id,
                project = %job.project,
                error = %e,
                "resolve job failed"
            );
        }
    }
}

pub fn build_resolve_prompt(
    old_title: &str,
    old_body: &str,
    new_title: &str,
    new_body: &str,
) -> String {
    format!(
        "You resolve conflicting project memories.\n\
         Return JSON only:\n\
         {{\"action\":\"keep_new\"|\"keep_old\"|\"keep_both\"|\"synthesize\",\"statement\":\"...\",\"confidence\":0.0}}\n\n\
         OLD:\n{old_title}\n{old_body}\n\n\
         NEW:\n{new_title}\n{new_body}\n"
    )
}

pub fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&raw[start..=end])
}

pub fn parse_resolve_json(raw: &str) -> anyhow::Result<ResolveVerdict> {
    let slice = extract_json_object(raw)
        .ok_or_else(|| anyhow::anyhow!("no JSON object in resolve output"))?;
    let v: serde_json::Value = serde_json::from_str(slice)?;
    let action_s = v
        .get("action")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let action = match action_s.as_str() {
        "keep_new" => ResolveAction::KeepNew,
        "keep_old" => ResolveAction::KeepOld,
        "keep_both" => ResolveAction::KeepBoth,
        "synthesize" => ResolveAction::Synthesize,
        other => anyhow::bail!("unknown resolve action '{other}'"),
    };
    let statement = v
        .get("statement")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let confidence = v.get("confidence").and_then(|x| x.as_f64()).unwrap_or(0.0);
    Ok(ResolveVerdict {
        action,
        statement,
        confidence,
    })
}

/// Live judge used by the worker and by Decide (sync path).
pub async fn resolve_pair(
    client: &Arc<dyn ClaudeClient>,
    registry: &ProjectRegistry,
    project: &str,
    old_id: i64,
    new_id: i64,
) -> anyhow::Result<ResolveVerdict> {
    let cfg = config::load_resolved(Some(project));
    let timeout = Duration::from_secs(cfg.conflict.timeout_secs.max(1));
    let model = cfg.conflict.model.cli_model_id();

    let project_state = registry.get_or_open(project)?;
    let conn = project_state.open_read_conn()?;
    let old = read_q::get(&conn, &ObservationKey::Id(old_id))?;
    let new = read_q::get(&conn, &ObservationKey::Id(new_id))?;
    drop(conn);

    if let Some(verdict) = try_laya_resolve(&cfg, &old, &new).await {
        apply_verdict(&project_state, &old, &new, &verdict).await?;
        return Ok(verdict);
    }

    let prompt = build_resolve_prompt(&old.title, &old.content, &new.title, &new.content);
    let raw = tokio::time::timeout(timeout, client.ask(&prompt, model))
        .await
        .map_err(|_| anyhow::anyhow!("resolve judge timed out"))??;
    let verdict = parse_resolve_json(&raw)?;
    crate::laya::log_teacher(
        "resolve",
        &prompt,
        &serde_json::json!({ "source": "claude" }),
        &serde_json::json!({
            "action": format!("{:?}", verdict.action),
            "confidence": verdict.confidence,
        }),
    );
    apply_verdict(&project_state, &old, &new, &verdict).await?;
    Ok(verdict)
}

async fn try_laya_resolve(
    cfg: &config::StatefulMemoryConfig,
    old: &statefulmemory_storage::Observation,
    new: &statefulmemory_storage::Observation,
) -> Option<ResolveVerdict> {
    if !cfg.laya.enabled || !cfg.laya.conflict {
        return None;
    }
    let client = std::sync::Arc::new(crate::laya::LayaClient::new(&cfg.laya));
    let state = crate::laya_schemas::resolve_state(&old.title, &old.content, &new.title, &new.content);
    let questions = crate::laya_schemas::resolve_questions();
    let model = cfg.laya.model_decide.clone();
    let client_c = std::sync::Arc::clone(&client);
    let state_c = state.clone();
    let questions_c = questions.clone();
    let pred = tokio::task::spawn_blocking(move || client_c.predict(&state_c, &questions_c, &model))
        .await
        .ok()?
        .ok()?;

    let action_s = client.choice_value(&pred, "action")?;
    let action = match action_s {
        "keep_new" => ResolveAction::KeepNew,
        "keep_old" => ResolveAction::KeepOld,
        "keep_both" => ResolveAction::KeepBoth,
        "synthesize" => ResolveAction::Synthesize,
        _ => return None,
    };
    let confidence = client.score_value(&pred, "confidence").unwrap_or(0.6);
    let verdict = ResolveVerdict {
        action,
        statement: String::new(),
        confidence,
    };
    crate::laya::log_teacher(
        "resolve",
        &state,
        &questions,
        &serde_json::json!({
            "action": action_s,
            "confidence": confidence,
            "source": "laya",
        }),
    );
    Some(verdict)
}

async fn apply_verdict(
    project: &statefulmemory_storage::ProjectState,
    old: &statefulmemory_storage::Observation,
    new: &statefulmemory_storage::Observation,
    verdict: &ResolveVerdict,
) -> anyhow::Result<()> {
    match verdict.action {
        ResolveAction::KeepBoth => return Ok(()),
        ResolveAction::KeepNew => {
            soft_delete(project, old.id).await?;
            add_resolved_by(project, old.id, new.id).await?;
        }
        ResolveAction::KeepOld => {
            soft_delete(project, new.id).await?;
            add_resolved_by(project, new.id, old.id).await?;
        }
        ResolveAction::Synthesize => {
            let statement = if verdict.statement.is_empty() {
                format!("{}\n\n(also: {})", new.content, old.content)
            } else {
                verdict.statement.clone()
            };
            let saved = save_resolution(project, new, &statement).await?;
            add_resolved_by(project, old.id, saved.id).await?;
            add_resolved_by(project, new.id, saved.id).await?;
        }
    }
    Ok(())
}

async fn soft_delete(project: &statefulmemory_storage::ProjectState, id: i64) -> anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    project.write.send(WriteRequest::SoftDeleteObservation {
        key: ObservationKey::Id(id),
        reply: tx,
    })?;
    rx.await
        .map_err(|_| anyhow::anyhow!("write thread crashed"))??;
    Ok(())
}

async fn add_resolved_by(
    project: &statefulmemory_storage::ProjectState,
    source_id: i64,
    target_id: i64,
) -> anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    project.write.send(WriteRequest::Custom {
        f: Box::new(move |conn| {
            statefulmemory_storage::add_relation(conn, source_id, target_id, "resolved_by", 0.9)?;
            Ok(())
        }),
        reply: tx,
    })?;
    rx.await
        .map_err(|_| anyhow::anyhow!("write thread crashed"))??;
    Ok(())
}

async fn save_resolution(
    project: &statefulmemory_storage::ProjectState,
    seed: &statefulmemory_storage::Observation,
    statement: &str,
) -> anyhow::Result<statefulmemory_storage::Observation> {
    let title: String = statement.chars().take(80).collect();
    let (tx, rx) = oneshot::channel();
    project.write.send(WriteRequest::SaveObservation {
        input: SaveObservationInput {
            sync_id: None,
            session_id: seed.session_id.clone(),
            r#type: "resolution".into(),
            title,
            content: statement.to_string(),
            tool_name: None,
            scope: seed.scope.clone(),
            created_by: Some("statefulmemory-resolve".into()),
            topic_key: Some(format!("resolution/{}", seed.id)),
            code_anchor: None,
            dedupe_window_secs: 0,
            max_content_chars: 50_000,
            skip_supersede: false,
        },
        reply: tx,
    })?;
    let obs = rx
        .await
        .map_err(|_| anyhow::anyhow!("write thread crashed"))??;
    Ok(obs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_synthesize() {
        let v = parse_resolve_json(
            r#"{"action":"synthesize","statement":"Use SQLite","confidence":0.9}"#,
        )
        .unwrap();
        assert_eq!(v.action, ResolveAction::Synthesize);
        assert_eq!(v.statement, "Use SQLite");
        assert!((v.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn parse_strips_markdown_fence() {
        let raw = "```json\n{\"action\":\"keep_both\",\"statement\":\"\",\"confidence\":0.5}\n```";
        let v = parse_resolve_json(raw).unwrap();
        assert_eq!(v.action, ResolveAction::KeepBoth);
    }

    #[test]
    fn parse_unknown_action_errors() {
        assert!(parse_resolve_json(r#"{"action":"explode"}"#).is_err());
    }
}
