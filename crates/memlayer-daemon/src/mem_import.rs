//! `ImportMem` — restore a portable `.mem` snapshot into one project.

use std::path::PathBuf;
use std::sync::Arc;

use tonic::Status;
use tracing::{info, instrument, warn};

use memlayer_proto::{ImportMemRequest, ImportMemResponse};
use memlayer_storage::write::WriteRequest;
use memlayer_sync::mem_archive::{decode, is_encrypted};
use memlayer_sync::snapshot::{self, ApplyReport};

use crate::error_map::map;
use crate::mem_export::{map_sync, SEED_REQUIRED_MSG};
use crate::service::DaemonState;

#[allow(clippy::result_large_err)]
pub async fn handle(
    state: &Arc<DaemonState>,
    req: ImportMemRequest,
) -> Result<ImportMemResponse, Status> {
    if req.project_name.trim().is_empty() {
        return Err(Status::invalid_argument("project_name is required"));
    }
    if !req.file.ends_with(".mem") {
        return Err(Status::invalid_argument("import path must end with .mem"));
    }
    let src = PathBuf::from(&req.file);
    let bytes = tokio::fs::read(&src)
        .await
        .map_err(|e| Status::invalid_argument(format!("read {}: {e}", src.display())))?;

    let seed_encrypted = is_encrypted(&bytes).map_err(map_sync)?;
    let seed_given = req
        .seed_phrase
        .as_deref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if seed_encrypted && !seed_given {
        return Err(Status::invalid_argument(SEED_REQUIRED_MSG));
    }
    let seed_ignored = !seed_encrypted && seed_given;
    if seed_ignored {
        warn!(project = %req.project_name, "ImportMem: seed provided but archive is not seed-encrypted");
    }

    let seed = if seed_encrypted {
        req.seed_phrase.clone()
    } else {
        None
    };
    let payload = tokio::task::spawn_blocking(move || decode(&bytes, seed.as_deref()))
        .await
        .map_err(|e| Status::internal(format!("decode join: {e}")))?
        .map_err(map_sync)?;

    let mode = if req.mode.trim().is_empty() {
        "merge".to_string()
    } else {
        req.mode.clone()
    };
    let project = map(state.registry.get_or_open(&req.project_name))?;
    let report = apply_on_write_thread(&project, payload, mode).await?;

    if let Some(pool) = state.embed_pool.read().as_ref() {
        if let Ok(conn) = project.open_read_conn() {
            if let Ok(rows) = snapshot::imported_observation_ids(&conn) {
                for (obs_id, title, content) in rows {
                    pool.try_queue(crate::embed_worker::EmbedTask {
                        project_name: project.display_name.clone(),
                        obs_id,
                        title,
                        content,
                    });
                }
            }
        }
    }

    info!(
        project = %project.normalized,
        observations = report.observations_imported,
        skipped = report.skipped,
        seed_encrypted,
        seed_ignored,
        "ImportMem complete"
    );

    Ok(ImportMemResponse {
        observations_imported: report.observations_imported,
        sessions_imported: report.sessions_imported,
        prompts_imported: report.prompts_imported,
        facts_imported: report.facts_imported,
        relations_imported: report.relations_imported,
        skipped: report.skipped,
        seed_encrypted,
        seed_ignored,
    })
}

#[instrument(level = "debug", skip(project, payload))]
#[allow(clippy::result_large_err)]
async fn apply_on_write_thread(
    project: &Arc<memlayer_storage::ProjectState>,
    payload: memlayer_sync::mem_archive::ArchivePayload,
    mode: String,
) -> Result<ApplyReport, Status> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let (stat_tx, stat_rx) = std::sync::mpsc::sync_channel(1);
    map(project.write.send(WriteRequest::Custom {
        f: Box::new(move |conn| {
            match snapshot::apply_payload(conn, &payload, &mode) {
                Ok(r) => {
                    let _ = stat_tx.send(r);
                    Ok(())
                }
                Err(e) => Err(memlayer_core::Error::internal(e.to_string())),
            }
        }),
        reply: reply_tx,
    }))?;
    reply_rx
        .await
        .map_err(|_| Status::internal("write thread crashed during ImportMem"))?
        .map_err(|e| Status::internal(format!("import apply: {e}")))?;
    stat_rx
        .recv()
        .map_err(|_| Status::internal("import apply produced no report"))
}
