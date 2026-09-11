// Generated with AI Coding Rules Hub
//! Live `SyncStatus` computation.
//!
//! Spec sections: FR3.1, SC-12.
//!
//! Reads the on-disk manifest to count total/unseen chunks, queries `sync_chunks`
//! for the import timestamp, and surfaces per-project last-error strings stored
//! in `DaemonState`.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::instrument;

use memlayer_proto::{SyncStatusRequest, SyncStatusResponse};
use memlayer_storage::{sync_state, ProjectState};
use memlayer_sync::manifest;
use tonic::Status;

use crate::error_map::map;
use crate::service::DaemonState;

/// Compute a live `SyncStatusResponse` for the given project.
///
/// Returns `SyncStatusResponse::default()` (all-zero, no errors) when the
/// project has never been exported (EC-1).
#[instrument(level = "debug", skip(state, project), fields(project = %project.normalized))]
#[allow(clippy::result_large_err)]
pub async fn compute(
    state: &Arc<DaemonState>,
    project: &Arc<ProjectState>,
    _req: &SyncStatusRequest,
) -> Result<SyncStatusResponse, Status> {
    // Resolve repo_path from stored config.
    let repo_path: Option<PathBuf> = state
        .registry
        .get_repo_path(&project.display_name)
        .unwrap_or(None);

    // Manifest-derived fields (requires filesystem access — run sync).
    let (total_exported_chunks, unseen_chunk_count, manifest_last_export_at) =
        if let Some(ref root) = repo_path {
            let manifest = manifest::read(root).await.unwrap_or_default();
            let conn = map(project.open_read_conn())?;
            let imported_ids =
                map(sync_state::list_imported_chunk_ids(&conn))?;
            let unseen = manifest
                .chunks
                .iter()
                .filter(|c| !imported_ids.contains(&c.chunk_id))
                .count() as i32;
            let last_exp = manifest.chunks.last().map(|c| c.created_at.clone());
            (manifest.chunks.len() as i64, unseen, last_exp)
        } else {
            (0i64, 0i32, None)
        };

    // Import-derived fields (DB query).
    let (total_imported_chunks, last_import_at) = {
        let conn = map(project.open_read_conn())?;
        let count = map(sync_state::count_imported_chunks(&conn))?;
        let last = map(sync_state::last_imported_at(&conn))?;
        (count, last)
    };

    // Per-project last error from DaemonState (set by SyncExport/SyncImport handlers).
    let last_error = state
        .last_sync_errors
        .lock()
        .get(&project.normalized)
        .cloned();

    // last_export_at: prefer DaemonState in-memory value (set by SyncExport),
    // fall back to manifest entry timestamp.
    let last_export_at = state
        .last_export_at
        .lock()
        .get(&project.normalized)
        .cloned()
        .or(manifest_last_export_at);

    Ok(SyncStatusResponse {
        last_export_at,
        last_import_at,
        unseen_chunk_count,
        last_error,
        total_exported_chunks,
        total_imported_chunks,
    })
}
