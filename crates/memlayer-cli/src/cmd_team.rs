//! `team` subcommand handlers (FR10).
//!
//! - `team init-ca <dir>`: generate a self-signed CA + leaf cert for TCP-mode
//!   hosting. Writes `ca.pem`, `server.pem`, `server-key.pem`. Refuses to
//!   overwrite without `--force` (EC-9). Local only — no daemon RPC.
//! - `team token-create / token-list / token-revoke`: admin-only RPCs that
//!   talk to the daemon. UDS mode passes the admin guard implicitly
//!   (filesystem mode 0600); TCP mode requires an admin bearer token.

#![allow(clippy::result_large_err)]

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use memlayer_proto as p;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
use thiserror::Error;

use crate::cli::{TeamTokenCreateArgs, TeamTokenRevokeArgs, TeamVerb};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::{Formatter, Render};

pub async fn dispatch(
    client: Option<&mut Client>,
    fmt: Formatter,
    verb: TeamVerb,
) -> ExitCode {
    let result: Result<(), TeamError> = match verb {
        TeamVerb::InitCa(a) => init_ca(&a.dir, a.force).map_err(TeamError::Init),
        TeamVerb::TokenCreate(a) => match client {
            Some(c) => token_create(c, fmt, a).await,
            None => Err(TeamError::ClientUnavailable),
        },
        TeamVerb::TokenList => match client {
            Some(c) => token_list(c, fmt).await,
            None => Err(TeamError::ClientUnavailable),
        },
        TeamVerb::TokenRevoke(a) => match client {
            Some(c) => token_revoke(c, a).await,
            None => Err(TeamError::ClientUnavailable),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(TeamError::Init(e)) => {
            eprintln!("memlayer: {e}");
            ExitCode::from(exit::GENERAL)
        }
        Err(TeamError::Status(s)) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
        Err(TeamError::Io(e)) => {
            eprintln!("memlayer: i/o error: {e}");
            ExitCode::from(exit::GENERAL)
        }
        Err(TeamError::ClientUnavailable) => {
            eprintln!("memlayer: token admin requires the daemon to be running");
            ExitCode::from(exit::DAEMON_UNREACHABLE)
        }
    }
}

#[derive(Debug, Error)]
enum TeamError {
    #[error("{0}")]
    Init(InitCaError),
    #[error("{0}")]
    Status(tonic::Status),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("daemon unavailable")]
    ClientUnavailable,
}

impl From<tonic::Status> for TeamError {
    fn from(s: tonic::Status) -> Self {
        TeamError::Status(s)
    }
}

// ---------------------------------------------------------------------------
// init-ca
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum InitCaError {
    #[error("output file exists: {0} (pass --force to overwrite)")]
    Exists(std::path::PathBuf),
    #[error("rcgen: {0}")]
    Rcgen(String),
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

const CA_PEM: &str = "ca.pem";
const SERVER_PEM: &str = "server.pem";
const SERVER_KEY_PEM: &str = "server-key.pem";

/// Generate a self-signed CA + leaf cert (FR10, SC-20). Writes three PEM
/// files into `dir`. Refuses to overwrite any existing PEM unless `force`.
pub fn init_ca(dir: &Path, force: bool) -> Result<(), InitCaError> {
    fs::create_dir_all(dir)?;
    let ca_path = dir.join(CA_PEM);
    let server_path = dir.join(SERVER_PEM);
    let server_key_path = dir.join(SERVER_KEY_PEM);
    if !force {
        for p in [&ca_path, &server_path, &server_key_path] {
            if p.exists() {
                return Err(InitCaError::Exists(p.clone()));
            }
        }
    }

    // 1. CA: self-signed, basic constraints CA:TRUE.
    let ca_key = KeyPair::generate().map_err(|e| InitCaError::Rcgen(e.to_string()))?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new())
        .map_err(|e| InitCaError::Rcgen(e.to_string()))?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "memlayer-team-ca");
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|e| InitCaError::Rcgen(e.to_string()))?;

    // 2. Leaf, signed by the CA. SAN = localhost (TCP-mode binds locally
    //    by default; ops can re-issue with their own SANs later).
    let leaf_key = KeyPair::generate().map_err(|e| InitCaError::Rcgen(e.to_string()))?;
    let mut leaf_params = CertificateParams::new(vec!["localhost".to_string()])
        .map_err(|e| InitCaError::Rcgen(e.to_string()))?;
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .map_err(|e| InitCaError::Rcgen(e.to_string()))?;

    fs::write(&ca_path, ca_cert.pem())?;
    fs::write(&server_path, leaf_cert.pem())?;
    fs::write(&server_key_path, leaf_key.serialize_pem())?;
    crate::info!("wrote {}", ca_path.display());
    crate::info!("wrote {}", server_path.display());
    crate::info!("wrote {}", server_key_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// token verbs
// ---------------------------------------------------------------------------

async fn token_create(
    client: &mut Client,
    fmt: Formatter,
    a: TeamTokenCreateArgs,
) -> Result<(), TeamError> {
    let req = p::CreateTokenRequest {
        name: a.name,
        is_admin: a.admin,
    };
    let resp = client.create_token(req).await?.into_inner();
    write_render(&resp, fmt).map_err(TeamError::Io)?;
    Ok(())
}

async fn token_list(client: &mut Client, fmt: Formatter) -> Result<(), TeamError> {
    let resp = client
        .list_tokens(p::ListTokensRequest {})
        .await?
        .into_inner();
    write_render(&resp, fmt).map_err(TeamError::Io)?;
    Ok(())
}

async fn token_revoke(client: &mut Client, a: TeamTokenRevokeArgs) -> Result<(), TeamError> {
    client
        .revoke_token(p::RevokeTokenRequest { name: a.name })
        .await?;
    Ok(())
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    use std::io::Write;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_ca_writes_three_pem_files() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("certs");
        init_ca(&dir, false).expect("init_ca");
        assert!(dir.join(CA_PEM).is_file(), "ca.pem missing");
        assert!(dir.join(SERVER_PEM).is_file(), "server.pem missing");
        assert!(dir.join(SERVER_KEY_PEM).is_file(), "server-key.pem missing");

        // Sanity: PEM files start with "-----BEGIN ".
        for name in [CA_PEM, SERVER_PEM, SERVER_KEY_PEM] {
            let body = fs::read_to_string(dir.join(name)).unwrap();
            assert!(
                body.starts_with("-----BEGIN "),
                "{name} is not a PEM file: starts with {:?}",
                body.chars().take(20).collect::<String>()
            );
        }
    }

    #[test]
    fn init_ca_refuses_overwrite_without_force() {
        let td = TempDir::new().unwrap();
        let dir = td.path().to_path_buf();
        // First call writes the files.
        init_ca(&dir, false).expect("first init_ca");
        // Second call without --force must refuse.
        let err = init_ca(&dir, false).expect_err("second init_ca should refuse");
        match err {
            InitCaError::Exists(p) => assert!(
                p.ends_with(CA_PEM) || p.ends_with(SERVER_PEM) || p.ends_with(SERVER_KEY_PEM)
            ),
            other => panic!("expected Exists, got {other:?}"),
        }
        // With --force, second call succeeds.
        init_ca(&dir, true).expect("init_ca --force");
    }

    #[test]
    fn init_ca_creates_missing_directory() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("nested").join("certs");
        assert!(!dir.exists());
        init_ca(&dir, false).unwrap();
        assert!(dir.is_dir());
    }
}
