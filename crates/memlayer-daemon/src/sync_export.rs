// Generated with AI Coding Rules Hub
//! `SyncExport` daemon handler.
//!
//! Spec sections: FR1.1, FR1.2, FR1.3, EC-7, EC-8, SC-1, SC-2, SC-14, SC-15,
//! NFR1, NFR5, NFR6.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tonic::Status;
use tracing::{debug, info, instrument, warn};

use memlayer_proto::{SyncExportRequest, SyncExportResponse};
use memlayer_storage::{sync_state, ProjectState};
use memlayer_sync::{chunk, manifest};

use crate::error_map::map;
use crate::service::DaemonState;

/// Execute a `SyncExport` for one project.
///
/// Returns an all-zero `SyncExportResponse` with `chunk_id = None` when
/// there are no unexported rows (FR1.2, EC-1).
#[instrument(level = "debug", skip(state, project), fields(project = %project.normalized))]
pub async fn run(
    state: &Arc<DaemonState>,
    project: &Arc<ProjectState>,
    req: &SyncExportRequest,
) -> Result<SyncExportResponse, Status> {
    // Resolve repo_path: prefer request field, fall back to stored config.
    let repo_path = resolve_repo_path(state, project, req.repo_path.as_deref())?;
    let Some(repo_path) = repo_path else {
        warn!(
            project = %project.normalized,
            "SyncExport: no repo_path available (EC-8) — skipping git-chunk export"
        );
        return Err(Status::failed_precondition(format!(
            "SyncExport: no repo_path configured for project '{}'. \
             Pass --repo-path or run 'sync export --repo-path <dir>' once to persist it.",
            project.normalized
        )));
    };

    // Persist repo_path if it came from the request (OQ-3).
    if let Some(rp) = req.repo_path.as_deref() {
        if let Err(e) = state.registry.set_repo_path(&project.display_name, Path::new(rp)) {
            warn!(project = %project.normalized, err = %e, "could not persist repo_path");
        }
    }

    // Acquire per-project export mutex (EC-7: prevents double-export races).
    let mutex = {
        let mut map = state.export_mutexes.lock();
        map.entry(project.normalized.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _guard = mutex.lock().await;

    // SELECT unexported rows on the read path (lock held — prevents racing
    // SELECT from another concurrent SyncExport; write-thread serialises the
    // mark_exported UPDATE below).
    let (observations, sessions, prompts) = {
        let conn = map(project.open_read_conn())?;
        map(sync_state::select_unexported_for_export(&conn))?
    };

    // FR1.2: no unexported rows → empty response, no file written.
    if observations.is_empty() && sessions.is_empty() && prompts.is_empty() {
        debug!(project = %project.normalized, "no unexported rows; returning empty response");
        return Ok(SyncExportResponse {
            chunk_id: None,
            sessions_exported: 0,
            observations_exported: 0,
            prompts_exported: 0,
        });
    }

    let n_obs = observations.len() as i64;
    let n_sess = sessions.len() as i64;
    let n_prompts = prompts.len() as i64;

    // Build JSONL (NFR5: streaming build, no full-DB load).
    let jsonl = map(chunk::build_jsonl(&observations, &sessions, &prompts)
        .map_err(|e| memlayer_core::error::Error::internal(e.to_string())))?;

    let cid = chunk::chunk_id(&jsonl);

    // Compress in a blocking task (NFR6: never block the async reactor).
    let compressed = chunk::compress(jsonl)
        .await
        .map_err(|e| Status::internal(format!("zstd compress: {e}")))?;

    // Atomic chunk write (SC-14, EH-2).
    let chunks_dir = manifest::chunks_dir(&repo_path);
    chunk::write_chunk_atomic(&chunks_dir, &cid, compressed)
        .await
        .map_err(|e| Status::internal(format!("write chunk: {e}")))?;

    // Append manifest entry.
    let entry = manifest::ManifestEntry {
        chunk_id: cid.clone(),
        created_at: Utc::now().to_rfc3339(),
        observations: n_obs,
        sessions: n_sess,
        prompts: n_prompts,
    };
    manifest::append_atomic(&repo_path, entry)
        .await
        .map_err(|e| Status::internal(format!("manifest append: {e}")))?;

    // Mark rows as exported + record chunk in sync_chunks (FR1.1 items 7-8).
    // Both run through WriteRequest::Custom so they happen atomically on the
    // write thread under the write lock.
    let exported_ids = sync_state::ExportedIds {
        observations: observations.iter().map(|o| o.id).collect(),
        sessions: sessions.iter().map(|s| s.id.clone()).collect(),
        prompts: prompts.iter().map(|p| p.id).collect(),
    };
    let now = Utc::now().to_rfc3339();
    let cid_for_write = cid.clone();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    map(project.write.send(memlayer_storage::write::WriteRequest::Custom {
        f: Box::new(move |conn| {
            sync_state::mark_exported(conn, &exported_ids, &now)?;
            sync_state::record_chunk_imported(conn, &cid_for_write)?;
            Ok(())
        }),
        reply: reply_tx,
    }))?;
    reply_rx
        .await
        .map_err(|_| Status::internal("write thread crashed during mark_exported"))?
        .map_err(|e| Status::internal(format!("mark_exported: {e}")))?;

    // Update in-memory last_export_at for SyncStatus (FR3.1).
    {
        let ts = Utc::now().to_rfc3339();
        state
            .last_export_at
            .lock()
            .insert(project.normalized.clone(), ts);
    }

    info!(
        project = %project.normalized,
        chunk_id = %cid,
        observations = n_obs,
        sessions = n_sess,
        prompts = n_prompts,
        "SyncExport complete"
    );

    Ok(SyncExportResponse {
        chunk_id: Some(cid),
        sessions_exported: n_sess,
        observations_exported: n_obs,
        prompts_exported: n_prompts,
    })
}

/// Resolve the `repo_path` for export: prefer the request-supplied value, then
/// the stored config value, then `None`.
fn resolve_repo_path(
    state: &Arc<DaemonState>,
    project: &Arc<ProjectState>,
    request_repo_path: Option<&str>,
) -> Result<Option<PathBuf>, Status> {
    if let Some(rp) = request_repo_path {
        let p = PathBuf::from(rp);
        if !p.is_dir() {
            return Err(Status::invalid_argument(format!(
                "repo_path '{}' is not a directory",
                p.display()
            )));
        }
        return Ok(Some(p));
    }
    // Fall back to stored config.
    Ok(state
        .registry
        .get_repo_path(&project.display_name)
        .unwrap_or(None))
}

/// Entry point called from the `sync_export` RPC handler in `service.rs`.
pub async fn handle(
    state: &Arc<DaemonState>,
    req: SyncExportRequest,
) -> Result<SyncExportResponse, Status> {
    let project_name = if req.project_name.trim().is_empty() {
        return Err(Status::invalid_argument("project_name is required"));
    } else {
        &req.project_name
    };
    let project = map(state.registry.get_or_open(project_name))?;
    run(state, &project, &req).await
}
