//! `memlayer decide` — recommend a decision from stored memories.

use std::io::{self, Write};
use std::process::ExitCode;

use memlayer_proto as p;

use crate::cli::DecideArgs;
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::{Formatter, Render};

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    args: DecideArgs,
) -> ExitCode {
    let req = p::DecideRequest {
        project_name: project_name.to_string(),
        question: args.question,
        limit: args.limit,
        mode: args.mode,
    };
    match client.decide(req).await {
        Ok(resp) => {
            let inner = resp.into_inner();
            if let Err(e) = write_render(&inner, fmt) {
                eprintln!("memlayer: i/o error: {e}");
                return ExitCode::from(exit::GENERAL);
            }
            ExitCode::SUCCESS
        }
        Err(s) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

fn write_render<R: Render>(resp: &R, fmt: Formatter) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    resp.render(fmt, &mut handle)?;
    handle.flush()
}
