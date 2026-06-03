//! `prompt` subcommand handlers (FR6, FR12.9).

use std::io;
use std::process::ExitCode;

use memlayer_proto as p;

use crate::cli::{PromptDeleteArgs, PromptRecentArgs, PromptSaveArgs, PromptSearchArgs, PromptVerb};
use crate::cmd_obs::{read_capped, Client, MAX_CONTENT_CHARS};
use crate::exit;
use crate::formatter::{Formatter, Render};

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    verb: PromptVerb,
) -> ExitCode {
    let result = match verb {
        PromptVerb::Save(a) => save(client, project_name, fmt, a).await,
        PromptVerb::Search(a) => search(client, project_name, fmt, a).await,
        PromptVerb::Recent(a) => recent(client, project_name, fmt, a).await,
        PromptVerb::Delete(a) => delete(client, project_name, a).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(VerbError::Status(s)) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
        Err(VerbError::Usage(m)) => {
            eprintln!("memlayer: {m}");
            ExitCode::from(exit::USAGE)
        }
    }
}

#[derive(Debug)]
enum VerbError {
    Status(tonic::Status),
    Usage(String),
}

impl From<tonic::Status> for VerbError {
    fn from(s: tonic::Status) -> Self {
        VerbError::Status(s)
    }
}

async fn save(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: PromptSaveArgs,
) -> Result<(), VerbError> {
    let content = if a.content == "-" {
        read_capped(io::stdin().lock(), MAX_CONTENT_CHARS).map_err(VerbError::Usage)?
    } else {
        a.content
    };
    let req = p::SavePromptRequest {
        project_name: project_name.to_string(),
        sync_id: None,
        session_id: a.session.unwrap_or_default(),
        content,
    };
    let resp = client.save_prompt(req).await?.into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    Ok(())
}

async fn search(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: PromptSearchArgs,
) -> Result<(), VerbError> {
    let req = p::SearchPromptsRequest {
        project_name: project_name.to_string(),
        query: a.query,
        limit: a.limit,
    };
    let resp = client.search_prompts(req).await?.into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    Ok(())
}

async fn recent(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: PromptRecentArgs,
) -> Result<(), VerbError> {
    let req = p::RecentPromptsRequest {
        project_name: project_name.to_string(),
        limit: a.limit,
    };
    let resp = client.recent_prompts(req).await?.into_inner();
    write_render(&resp, fmt).map_err(io_to_status)?;
    Ok(())
}

async fn delete(
    client: &mut Client,
    project_name: &str,
    a: PromptDeleteArgs,
) -> Result<(), VerbError> {
    let key = match a.id.parse::<i64>() {
        Ok(n) => p::delete_prompt_request::Key::Id(n),
        Err(_) => p::delete_prompt_request::Key::SyncId(a.id),
    };
    let req = p::DeletePromptRequest {
        project_name: project_name.to_string(),
        key: Some(key),
    };
    client.delete_prompt(req).await?;
    Ok(())
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    use std::io::Write;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}

fn io_to_status(e: io::Error) -> VerbError {
    VerbError::Status(tonic::Status::internal(format!("io: {e}")))
}
