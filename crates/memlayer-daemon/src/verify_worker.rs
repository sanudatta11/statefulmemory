//! Async verify worker — re-check anchors off the hot path.
//!
//! Same pool shape as resolve/extract: bounded channel, `try_queue` drops on full.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tokio::sync::oneshot;

use memlayer_core::git;
use memlayer_storage::anchor::{self, VerifyState};
use memlayer_storage::write::WriteRequest;
use memlayer_storage::ProjectRegistry;

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
                .name(format!("memlayer-verify-{i}"))
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

fn run_job(registry: &ProjectRegistry, job: &VerifyJob) -> memlayer_core::Result<Vec<AnchorVerdict>> {
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

pub fn apply_verdicts(
    project: &memlayer_storage::ProjectState,
    verdicts: &[AnchorVerdict],
) -> memlayer_core::Result<()> {
    if verdicts.is_empty() {
        return Ok(());
    }
    let verdicts = verdicts.to_vec();
    let (tx, rx) = oneshot::channel();
    project.write.send(WriteRequest::Custom {
        f: Box::new(move |conn| {
            for v in &verdicts {
                // Leave verified_commit untouched when head is unknown.
                let commit: Option<&str> = None;
                anchor::set_verify_state(conn, v.observation_id, v.state, commit)?;
            }
            Ok(())
        }),
        reply: tx,
    })?;
    rx.blocking_recv()
        .map_err(|_| memlayer_core::Error::internal("verify write reply dropped"))??;
    Ok(())
}

/// Apply verdicts and stamp `verified_commit` when state is Verified.
pub fn apply_verdicts_with_head(
    project: &memlayer_storage::ProjectState,
    verdicts: &[AnchorVerdict],
    head: &str,
) -> memlayer_core::Result<()> {
    if verdicts.is_empty() {
        return Ok(());
    }
    let verdicts = verdicts.to_vec();
    let head = head.to_string();
    let (tx, rx) = oneshot::channel();
    project.write.send(WriteRequest::Custom {
        f: Box::new(move |conn| {
            for v in &verdicts {
                let commit = if v.state == VerifyState::Verified {
                    Some(head.as_str())
                } else {
                    None
                };
                anchor::set_verify_state(conn, v.observation_id, v.state, commit)?;
            }
            Ok(())
        }),
        reply: tx,
    })?;
    rx.blocking_recv()
        .map_err(|_| memlayer_core::Error::internal("verify write reply dropped"))??;
    Ok(())
}
