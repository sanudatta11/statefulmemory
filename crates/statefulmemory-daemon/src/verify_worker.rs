//! Async verify worker — re-check anchors off the hot path.
//!
//! Same pool shape as resolve/extract: bounded channel, `try_queue` drops on full.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tokio::sync::oneshot;

use statefulmemory_core::git;
use statefulmemory_storage::anchor::{self, VerifyState};
use statefulmemory_storage::write::WriteRequest;
use statefulmemory_storage::ProjectRegistry;

use crate::extract_worker::QueueResult;
use crate::verify::{verify_anchors, AnchorVerdict};

const QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct VerifyJob {
    pub project: String,
    pub repo_path: PathBuf,
    /// When set, only verify this observation; otherwise all anchored rows.
    pub observation_id: Option<i64>,
}

#[derive(Clone)]
pub struct VerifyWorkerPool {
    tx: Sender<VerifyJob>,
}

impl VerifyWorkerPool {
    pub fn spawn(registry: Arc<ProjectRegistry>, n_workers: usize) -> Self {
        let (tx, rx) = bounded(QUEUE_CAPACITY);
        let n = n_workers.max(1);
        for i in 0..n {
            let rx = rx.clone();
            let registry = registry.clone();
            thread::Builder::new()
                .name(format!("statefulmemory-verify-{i}"))
                .spawn(move || run_loop(rx, registry))
                .expect("spawn verify worker thread");
        }
        Self { tx }
    }

    pub fn try_queue(&self, job: VerifyJob) -> QueueResult {
        match self.tx.try_send(job) {
            Ok(_) => QueueResult::Queued,
            Err(TrySendError::Full(_)) => {
                tracing::warn!("verify queue full; dropping job");
                QueueResult::Dropped
            }
            Err(TrySendError::Disconnected(_)) => QueueResult::Dropped,
        }
    }
}

fn run_loop(rx: Receiver<VerifyJob>, registry: Arc<ProjectRegistry>) {
    while let Ok(job) = rx.recv() {
        if let Err(e) = run_job(&registry, &job) {
            tracing::warn!(project = %job.project, error = %e, "verify job failed");
        }
    }
}

fn run_job(
    registry: &ProjectRegistry,
    job: &VerifyJob,
) -> statefulmemory_core::Result<Vec<AnchorVerdict>> {
    if !git::is_repo(&job.repo_path) {
        return Ok(vec![]);
    }
    let head = match git::head_sha(&job.repo_path) {
        Ok(h) => h,
        Err(_) => return Ok(vec![]),
    };
    let project = registry.get_or_open(&job.project)?;
    let conn = project.open_read_conn()?;
    let pairs = match job.observation_id {
        Some(id) => {
            let anchors = anchor::anchors_for(&conn, id)?;
            anchors.into_iter().map(|a| (id, a)).collect::<Vec<_>>()
        }
        None => {
            let listed = anchor::list_anchored(&conn, 10_000)?;
            listed
                .into_iter()
                .flat_map(|(id, anchors)| anchors.into_iter().map(move |a| (id, a)))
                .collect()
        }
    };
    drop(conn);
    let verdicts = verify_anchors(&job.repo_path, &head, &pairs);
    apply_verdicts_with_head(&project, &verdicts, &head)?;
    Ok(verdicts)
}

fn send_verdict_write(
    project: &statefulmemory_storage::ProjectState,
    verdicts: &[AnchorVerdict],
    head: Option<&str>,
) -> statefulmemory_core::Result<oneshot::Receiver<statefulmemory_core::Result<()>>> {
    let verdicts = verdicts.to_vec();
    let head = head.map(str::to_owned);
    let (tx, rx) = oneshot::channel();
    project.write.send(WriteRequest::Custom {
        f: Box::new(move |conn| {
            for v in &verdicts {
                let commit = if v.state == VerifyState::Verified {
                    head.as_deref()
                } else {
                    None
                };
                anchor::set_verify_state(conn, v.observation_id, v.state, commit)?;
            }
            Ok(())
        }),
        reply: tx,
    })?;
    Ok(rx)
}

pub fn apply_verdicts(
    project: &statefulmemory_storage::ProjectState,
    verdicts: &[AnchorVerdict],
) -> statefulmemory_core::Result<()> {
    if verdicts.is_empty() {
        return Ok(());
    }
    let rx = send_verdict_write(project, verdicts, None)?;
    rx.blocking_recv()
        .map_err(|_| statefulmemory_core::Error::internal("verify write reply dropped"))??;
    Ok(())
}

/// Apply verdicts and stamp `verified_commit` when state is Verified.
pub fn apply_verdicts_with_head(
    project: &statefulmemory_storage::ProjectState,
    verdicts: &[AnchorVerdict],
    head: &str,
) -> statefulmemory_core::Result<()> {
    if verdicts.is_empty() {
        return Ok(());
    }
    let rx = send_verdict_write(project, verdicts, Some(head))?;
    rx.blocking_recv()
        .map_err(|_| statefulmemory_core::Error::internal("verify write reply dropped"))??;
    Ok(())
}

pub async fn apply_verdicts_with_head_async(
    project: &statefulmemory_storage::ProjectState,
    verdicts: &[AnchorVerdict],
    head: &str,
) -> statefulmemory_core::Result<()> {
    if verdicts.is_empty() {
        return Ok(());
    }
    let rx = send_verdict_write(project, verdicts, Some(head))?;
    rx.await
        .map_err(|_| statefulmemory_core::Error::internal("verify write reply dropped"))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn apply_verdicts_with_head_async_awaits_write_reply() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("verify.db");
        let write = statefulmemory_storage::write::spawn_write_thread(
            "verify".into(),
            db_path.clone(),
            1,
            Duration::from_millis(1),
            None,
        )
        .unwrap();
        let project = statefulmemory_storage::ProjectState {
            normalized: "verify".into(),
            display_name: "verify".into(),
            db_path,
            write,
            read_in_flight: parking_lot::Mutex::new(0),
        };
        let verdicts = vec![AnchorVerdict {
            observation_id: 1,
            state: VerifyState::Stale,
            reason: "test",
        }];

        apply_verdicts_with_head_async(&project, &verdicts, "head")
            .await
            .unwrap();
    }
}
