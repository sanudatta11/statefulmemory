//! `session` subcommand handlers (FR5, FR12.7, FR12.8).

use std::io;
use std::process::ExitCode;

use memlayer_proto as p;

use crate::cli::{
    SessionDeleteArgs, SessionEndArgs, SessionGetArgs, SessionListArgs, SessionStartArgs,
    SessionSummaryArgs, SessionVerb,
};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::{Formatter, Render};

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    verb: SessionVerb,
) -> ExitCode {
    let result: Result<(), tonic::Status> = match verb {
        SessionVerb::Start(a) => start(client, project_name, fmt, a).await,
        SessionVerb::End(a) => end(client, project_name, fmt, a).await,
        SessionVerb::Summary(a) => summary(client, project_name, fmt, a).await,
        SessionVerb::List(a) => list(client, project_name, fmt, a).await,
        SessionVerb::Get(a) => get(client, project_name, fmt, a).await,
        SessionVerb::Delete(a) => delete(client, project_name, a).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(s) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

async fn start(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionStartArgs,
) -> Result<(), tonic::Status> {
    let req = p::StartSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
        directory: a.directory.unwrap_or_default(),
    };
    let resp = client.start_session(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn end(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionEndArgs,
) -> Result<(), tonic::Status> {
    let req = p::EndSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
        summary: a.summary,
    };
    let resp = client.end_session(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn summary(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionSummaryArgs,
) -> Result<(), tonic::Status> {
    let req = p::SaveSessionSummaryRequest {
        project_name: project_name.to_string(),
        id: a.id,
        summary: a.content,
    };
    let resp = client.save_session_summary(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn list(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionListArgs,
) -> Result<(), tonic::Status> {
    let req = p::ListSessionsRequest {
        project_name: project_name.to_string(),
        limit: a.limit,
        cursor: a.cursor.map(|t| p::Cursor { token: t }),
    };
    let resp = client.list_sessions(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn get(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: SessionGetArgs,
) -> Result<(), tonic::Status> {
    let req = p::GetSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
    };
    let resp = client.get_session(req).await?.into_inner();
    write_render(&resp, fmt)?;
    Ok(())
}

async fn delete(
    client: &mut Client,
    project_name: &str,
    a: SessionDeleteArgs,
) -> Result<(), tonic::Status> {
    let req = p::DeleteSessionRequest {
        project_name: project_name.to_string(),
        id: a.id,
    };
    client.delete_session(req).await?;
    Ok(())
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    use std::io::Write;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}
