//! `ExportMem` — write a portable `.mem` snapshot of one project.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tonic::Status;
use tracing::{info, instrument};

use memlayer_proto::{ExportMemRequest, ExportMemResponse};
use memlayer_sync::mem_archive::{encode, EncodeParams};
use memlayer_sync::snapshot;

use crate::error_map::map;
use crate::service::DaemonState;

pub const SEED_REQUIRED_MSG: &str = "this archive is seed-encrypted and cannot be imported without the seed phrase. \
pass --seed-file or --seed-phrase (the same phrase used at export).";

const SEED_TOO_SHORT: &str = "seed phrase must be at least 12 characters after trim";

#[allow(clippy::result_large_err)]
pub async fn handle(
    state: &Arc<DaemonState>,
    req: ExportMemRequest,
) -> Result<ExportMemResponse, Status> {
    if req.project_name.trim().is_empty() {
        return Err(Status::invalid_argument("project_name is required"));
    }
    if !req.file.ends_with(".mem") {
        return Err(Status::invalid_argument(
            "export path must end with .mem",
        ));
    }
    let dest = PathBuf::from(&req.file);
    let project = map(state.registry.get_or_open(&req.project_name))?;
    run(&project, dest, req.seed_phrase.filter(|s| !s.is_empty())).await
}

#[instrument(level = "debug", skip(project, seed), fields(project = %project.normalized))]
#[allow(clippy::result_large_err)]
async fn run(
    project: &Arc<memlayer_storage::ProjectState>,
    dest: PathBuf,
    seed: Option<String>,
) -> Result<ExportMemResponse, Status> {
    let payload = {
        let conn = map(project.open_read_conn())?;
        snapshot::dump_payload(&conn, &project.normalized, &Utc::now().to_rfc3339())
            .map_err(map_sync)?
    };

    let n_obs = payload.observations.len() as i64;
    let n_sess = payload.sessions.len() as i64;
    let n_prompts = payload.prompts.len() as i64;
    let n_facts = payload.facts.len() as i64;
    let n_rels = payload.relations.len() as i64;

    let mut salt = [0u8; 16];
    let mut aead_nonce = [0u8; 24];
    getrandom::getrandom(&mut salt)
        .map_err(|e| Status::internal(format!("getrandom salt: {e}")))?;
    getrandom::getrandom(&mut aead_nonce)
        .map_err(|e| Status::internal(format!("getrandom nonce: {e}")))?;
    let params = EncodeParams {
        salt,
        aead_nonce,
        seed,
    };

    let bytes = tokio::task::spawn_blocking(move || encode(&payload, &params))
        .await
        .map_err(|e| Status::internal(format!("encode join: {e}")))?
        .map_err(map_sync)?;

    write_atomic(&dest, &bytes).map_err(|e| Status::internal(format!("write archive: {e}")))?;

    let nbytes = bytes.len() as i64;
    info!(
        project = %project.normalized,
        path = %dest.display(),
        observations = n_obs,
        bytes = nbytes,
        "ExportMem complete"
    );

    Ok(ExportMemResponse {
        file: dest.to_string_lossy().into_owned(),
        observations: n_obs,
        sessions: n_sess,
        prompts: n_prompts,
        facts: n_facts,
        relations: n_rels,
        bytes: nbytes,
    })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension("mem.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[allow(clippy::result_large_err)]
pub(crate) fn map_sync(e: memlayer_sync::SyncError) -> Status {
    use memlayer_sync::SyncError;
    match e {
        SyncError::SeedRequired => Status::invalid_argument(SEED_REQUIRED_MSG),
        SyncError::InvalidSeed => {
            Status::invalid_argument("invalid seed phrase or corrupt archive")
        }
        SyncError::SeedTooShort => Status::invalid_argument(SEED_TOO_SHORT),
        SyncError::NotArchive => Status::invalid_argument("not a memlayer archive"),
        SyncError::Truncated => Status::invalid_argument("memlayer archive truncated"),
        SyncError::ChecksumMismatch => Status::invalid_argument("memlayer archive checksum mismatch"),
        SyncError::UnsupportedVersion(v) => {
            Status::invalid_argument(format!("unsupported memlayer archive version {v}"))
        }
        other => Status::internal(other.to_string()),
    }
}
