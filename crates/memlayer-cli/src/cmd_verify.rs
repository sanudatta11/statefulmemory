//! `memlayer verify` — re-check code anchors against git HEAD.

use std::io::{self, Write};
use std::process::ExitCode;

use memlayer_proto as p;
use tonic::transport::Channel;

use crate::cli::VerifyArgs;
use crate::exit;
use crate::formatter::Formatter;

type Client = p::memlayer_client::MemlayerClient<Channel>;

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    args: VerifyArgs,
) -> ExitCode {
    let req = p::VerifyAnchorsRequest {
        project_name: project_name.to_string(),
        observation_id: args.id,
    };
    match client.verify_anchors(req).await {
        Ok(resp) => {
            let r = resp.into_inner();
            if args.quiet {
                return ExitCode::SUCCESS;
            }
            match fmt {
                Formatter::Json | Formatter::Yaml => {
                    let v = serde_json::json!({
                        "verified": r.verified,
                        "stale": r.stale,
                        "invalidated": r.invalidated,
                        "unprovable": r.unprovable,
                        "unanchored": r.unanchored,
                        "changed_ids": r.changed_ids,
                        "results": r.results.iter().map(|x| serde_json::json!({
                            "id": x.id,
                            "state": x.state,
                            "title": x.title,
                        })).collect::<Vec<_>>(),
                    });
                    if fmt == Formatter::Yaml {
                        if let Ok(s) = serde_yaml::to_string(&v) {
                            println!("{s}");
                        }
                    } else {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    }
                }
                Formatter::Text => {
                    let mut out = io::stdout().lock();
                    let _ = writeln!(
                        out,
                        "verified {}  stale {}  invalidated {}  unprovable {}  unanchored {}",
                        r.verified, r.stale, r.invalidated, r.unprovable, r.unanchored
                    );
                    for row in r.results.iter().filter(|x| {
                        matches!(x.state.as_str(), "stale" | "invalidated" | "unprovable")
                    }) {
                        let _ = writeln!(
                            out,
                            "  [{}] #{} {}",
                            row.state, row.id, row.title
                        );
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(s) => {
            if !args.quiet {
                eprintln!("memlayer verify: {}", s.message());
            }
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}
