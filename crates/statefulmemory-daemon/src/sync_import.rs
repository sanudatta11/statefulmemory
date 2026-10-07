//! `SyncImport` daemon handler — git-chunk import (Phase 4.5).
//!
//! Reverses [`crate::sync_export`]: read the repo manifest, and for each chunk
//! not yet imported (tracked in `sync_chunks`), read + decompress + parse the
//! JSONL and upsert its rows by `sync_id` (last-writer-wins via `updated_at`).
//! Idempotent: already-imported chunk ids are skipped; a corrupt chunk is
//! logged + skipped (FR2.2) without failing the whole import. All upserts for
//! one chunk + the `record_chunk_imported` run atomically on the write thread.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use tonic::Status;
use tracing::{info, instrument, warn};

use statefulmemory_proto::{SyncImportRequest, SyncImportResponse};
use statefulmemory_storage::sync_state;
use statefulmemory_storage::write::WriteRequest;
use statefulmemory_storage::ProjectState;
use statefulmemory_sync::chunk::{self, ChunkLine};
use statefulmemory_sync::manifest;

use crate::error_map::map;
use crate::service::DaemonState;

/// Resolve the repo path for import: request value, else stored config; must
/// be an existing directory.
#[allow(clippy::result_large_err)]
fn resolve_repo_path(
    state: &Arc<DaemonState>,
    project: &Arc<ProjectState>,
    request_repo_path: Option<&str>,
) -> Result<PathBuf, Status> {
    let raw = if let Some(p) = request_repo_path {
        Some(p.to_string())
    } else {
        state
            .registry
            .get_repo_path(&project.display_name)
            .map_err(crate::error_map::to_status)?
            .map(|p| p.to_string_lossy().into_owned())
    };
    let Some(raw) = raw else {
        return Err(Status::failed_precondition(format!(
            "SyncImport: no repo_path configured for project '{}'. Pass --repo-path.",
            project.normalized
        )));
    };
    let path = state.resolve_user_path(project, &raw, true)?;
    if !path.is_dir() {
        return Err(Status::invalid_argument(format!(
            "repo_path '{}' is not a directory",
            path.display()
        )));
    }
    Ok(path)
}

#[instrument(level = "debug", skip(state, project), fields(project = %project.normalized))]
#[allow(clippy::result_large_err)]
pub async fn run(
    state: &Arc<DaemonState>,
    project: &Arc<ProjectState>,
    req: &SyncImportRequest,
) -> Result<SyncImportResponse, Status> {
    let repo_path = resolve_repo_path(state, project, req.repo_path.as_deref())?;
    if req.repo_path.is_some() {
        if let Err(e) = state
            .registry
            .set_repo_path(&project.display_name, &repo_path)
        {
            warn!(project = %project.normalized, err = %e, "could not persist repo_path");
        }
    }

    let manifest = manifest::read(&repo_path)
        .await
        .map_err(|e| Status::internal(format!("read manifest: {e}")))?;

    // Chunks already imported (idempotency) — skip without re-reading.
    let imported: HashSet<String> = {
        let conn = map(project.open_read_conn())?;
        map(sync_state::list_imported_chunk_ids(&conn))?
            .into_iter()
            .collect()
    };

    let chunks_dir = manifest::chunks_dir(&repo_path);
    let (mut n_chunks, mut n_obs, mut n_sess, mut n_prompts) = (0i64, 0i64, 0i64, 0i64);

    for entry in manifest.chunks {
        if imported.contains(&entry.chunk_id) {
            continue;
        }
        let compressed = match chunk::read_chunk(&chunks_dir, &entry.chunk_id).await {
            Ok(c) => c,
            Err(e) => {
                warn!(chunk = %entry.chunk_id, error = %e, "read chunk failed — skipping");
                continue;
            }
        };
        let jsonl = match chunk::decompress_sync(&compressed) {
            Ok(j) => j,
            Err(e) => {
                warn!(chunk = %entry.chunk_id, error = %e, "decompress failed — skipping");
                continue;
            }
        };
        let lines = match chunk::parse_jsonl(&jsonl) {
            Ok(l) => l,
            Err(e) => {
                warn!(chunk = %entry.chunk_id, error = %e, "corrupt chunk — skipping (FR2.2)");
                continue;
            }
        };

        // Count per kind before moving `lines` into the write closure.
        let (mut co, mut cs, mut cp) = (0i64, 0i64, 0i64);
        for l in &lines {
            match l {
                ChunkLine::Observation(_) => co += 1,
                ChunkLine::Session(_) => cs += 1,
                ChunkLine::Prompt(_) => cp += 1,
            }
        }

        // One atomic write-thread txn per chunk: upsert all rows, then record
        // the chunk as imported (so a crash mid-chunk re-imports idempotently).
        let cid = entry.chunk_id.clone();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        map(project.write.send(WriteRequest::Custom {
            f: Box::new(move |conn| {
                for l in &lines {
                    match l {
                        ChunkLine::Observation(o) => {
                            sync_state::upsert_observation_from_remote(conn, o)?;
                        }
                        ChunkLine::Session(s) => {
                            sync_state::upsert_session_from_remote(conn, s)?;
                        }
                        ChunkLine::Prompt(p) => {
                            sync_state::upsert_prompt_from_remote(conn, p)?;
                        }
                    }
                }
                sync_state::record_chunk_imported(conn, &cid)?;
                Ok(())
            }),
            reply: reply_tx,
        }))?;
        reply_rx
            .await
            .map_err(|_| Status::internal("write thread crashed during SyncImport"))?
            .map_err(|e| Status::internal(format!("SyncImport upsert: {e}")))?;

        n_chunks += 1;
        n_obs += co;
        n_sess += cs;
        n_prompts += cp;
    }

    info!(
        project = %project.normalized,
        chunks = n_chunks,
        observations = n_obs,
        sessions = n_sess,
        prompts = n_prompts,
        "SyncImport complete"
    );
    Ok(SyncImportResponse {
        chunks_imported: n_chunks,
        sessions_imported: n_sess,
        observations_imported: n_obs,
        prompts_imported: n_prompts,
    })
}

/// Entry point called from the `sync_import` RPC handler in `service.rs`.
#[allow(clippy::result_large_err)]
pub async fn handle(
    state: &Arc<DaemonState>,
    req: SyncImportRequest,
) -> Result<SyncImportResponse, Status> {
    if req.project_name.trim().is_empty() {
        return Err(Status::invalid_argument("project_name is required"));
    }
    let project = map(state.registry.get_or_open(&req.project_name))?;
    run(state, &project, &req).await
}
