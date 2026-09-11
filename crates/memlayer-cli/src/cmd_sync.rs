//! `sync` subcommand handlers (FR9).
//!
//! Spec 2 ships only `sync status`; export/import verbs land in Spec 3.

#![allow(clippy::result_large_err)]

use std::io;
use std::process::ExitCode;

use memlayer_proto as p;

use crate::cli::{SyncStatusArgs, SyncVerb};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::{Formatter, Render};

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    verb: SyncVerb,
) -> ExitCode {
    let result = match verb {
        SyncVerb::Status(a) => status(client, project_name, fmt, a).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(s) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

async fn status(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SyncStatusArgs,
) -> Result<(), tonic::Status> {
    let req = p::SyncStatusRequest {
        project_name: Some(a.project.unwrap_or_else(|| project_name.to_string())),
    };
    let resp = client.sync_status(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    use std::io::Write;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}
