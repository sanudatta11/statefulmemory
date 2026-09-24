use std::process::ExitCode;
use std::sync::Arc;

use statefulmemory_client::channel;
use statefulmemory_ui::{GrpcBackend, UiAssets, UiBackend, UiConfig};

use crate::cli::UiArgs;
use crate::cmd_daemon::default_autospawn_config;
use crate::exit;

const UI_HTML: &str = include_str!("../../../dashboard/dist/index.html");
const UI_SCRIPT: &str = include_str!("../../../dashboard/dist/assets/app.js");
const UI_STYLE: &str = include_str!("../../../dashboard/dist/assets/app.css");

pub async fn dispatch(project_flag: Option<String>, args: &UiArgs) -> ExitCode {
    let config = match UiConfig::new(args.host.clone(), args.port) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(exit::USAGE);
        }
    };
    let project = {
        let env_override = std::env::var("STATEFULMEMORY_PROJECT")
            .ok()
            .filter(|value| !value.is_empty());
        let cwd = match std::env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("statefulmemory: getcwd: {error}");
                return ExitCode::from(exit::GENERAL);
            }
        };
        match crate::project_detect::detect_in(&cwd, env_override, project_flag) {
            Ok(detection) => detection.normalized,
            Err(error) => {
                eprintln!("statefulmemory: {error}");
                return ExitCode::from(error.exit_code());
            }
        }
    };

    let socket = statefulmemory_core::paths::socket_path();
    if let Err(error) = crate::autospawn::ensure_running(&default_autospawn_config()).await {
        eprintln!("statefulmemory: {error}");
        return ExitCode::from(error.exit_code());
    }
    let channel = match channel::connect_uds(&socket) {
        Ok(channel) => channel,
        Err(error) => {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(exit::DAEMON_UNREACHABLE);
        }
    };
    let backend: Arc<dyn UiBackend> = Arc::new(GrpcBackend::new(channel));
    let running = match statefulmemory_ui::bind_with_assets(
        config,
        project,
        backend,
        UiAssets {
            html: UI_HTML,
            script: UI_SCRIPT,
            style: UI_STYLE,
        },
    ) {
        Ok(running) => running,
        Err(error) => {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(exit::GENERAL);
        }
    };
    println!(
        "statefulmemory UI → {}     hit Ctrl-C to stop",
        running.launch_url()
    );
    let thread = std::thread::spawn(move || running.serve());
    match thread.join() {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("statefulmemory: ui: {error}");
            ExitCode::from(exit::GENERAL)
        }
        Err(_) => {
            eprintln!("statefulmemory: ui worker terminated unexpectedly");
            ExitCode::from(exit::GENERAL)
        }
    }
}
