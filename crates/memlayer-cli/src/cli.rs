//! `clap` derive structures for the full memlayer CLI surface (PRD §3.3).
//!
//! Spec sections: FR1, FR3, FR4–FR11, §11.2.
//!
//! This file is the single source of truth for the noun-verb command tree.
//! Per-command handler modules added in spec2-t5..t7 (`cmd_obs.rs`,
//! `cmd_session.rs`, etc.) accept the parsed args defined here. Verbs that
//! still lack flags below will gain them in those follow-up tasks.

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "memlayer",
    version,
    about = "Persistent memory for AI coding agents",
    long_about = "memlayer stores observations, sessions, and prompts in a per-project SQLite \
                  database served by a user-local daemon over gRPC."
)]
pub struct Cli {
    /// Output format. Default: `text` on a TTY, `json` when piped.
    #[arg(long, global = true, value_enum)]
    pub output: Option<OutputFormat>,

    /// Project name override. Skips detection (FR3.4).
    #[arg(long, global = true, env = "MEMLAYER_PROJECT")]
    pub project: Option<String>,

    /// Disable ANSI color in output (FR1.4). `NO_COLOR` env var also works.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Suppress informational output (FR1.6).
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    Yaml,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage observations (decisions, policies, preferences …).
    Obs(ObsArgs),
    /// Manage agent sessions.
    Session(SessionArgs),
    /// Manage saved user prompts.
    Prompt(PromptArgs),
    /// Manage projects.
    Project(ProjectArgs),
    /// Sync state (export/import wired in Spec 3).
    Sync(SyncArgs),
    /// Daemon lifecycle.
    Daemon(DaemonArgs),
    /// Team / TCP-mode setup (CA generation + token admin).
    Team(TeamArgs),
    /// Tail the daemon log.
    Logs(LogsArgs),
    /// Print version and exit.
    Version,
}

#[derive(Args, Debug)]
pub struct ObsArgs {
    #[command(subcommand)]
    pub verb: ObsVerb,
}

#[derive(Subcommand, Debug)]
pub enum ObsVerb {
    /// Save a new observation.
    Save,
    /// Update an existing observation by id.
    Update,
    /// Soft-delete (or `--hard`) an observation.
    Delete,
    /// Print a single observation.
    Get,
    /// FTS5 search.
    Search,
    /// Recent observations, paginated.
    Recent,
    /// List observations with filters.
    List,
    /// Markdown context summary for prompt injection.
    Context,
    /// Chronological neighbors of an observation.
    Timeline,
    /// Suggest a stable topic key for a candidate observation.
    SuggestTopicKey,
    /// Extract `## Key Learnings:` bullets from stdin.
    CapturePassive,
}

#[derive(Args, Debug)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub verb: SessionVerb,
}

#[derive(Subcommand, Debug)]
pub enum SessionVerb {
    Start,
    End,
    Summary,
    List,
    Get,
    Delete,
}

#[derive(Args, Debug)]
pub struct PromptArgs {
    #[command(subcommand)]
    pub verb: PromptVerb,
}

#[derive(Subcommand, Debug)]
pub enum PromptVerb {
    Save,
    Search,
    Recent,
    Delete,
}

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub verb: ProjectVerb,
}

#[derive(Subcommand, Debug)]
pub enum ProjectVerb {
    List,
    Current,
    Merge,
    Delete,
    Consolidate,
    Prune,
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    #[command(subcommand)]
    pub verb: SyncVerb,
}

#[derive(Subcommand, Debug)]
pub enum SyncVerb {
    /// Sync status — only verb in Spec 2; export/import land in Spec 3.
    Status,
}

#[derive(Args, Debug)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub verb: DaemonVerb,
}

#[derive(Subcommand, Debug)]
pub enum DaemonVerb {
    /// Start the daemon. `--foreground` keeps it attached for tests.
    Start {
        /// Run attached to the terminal; do not auto-spawn (FR2.5).
        #[arg(long)]
        foreground: bool,
    },
    Stop,
    Status,
    Restart,
}

#[derive(Args, Debug)]
pub struct TeamArgs {
    #[command(subcommand)]
    pub verb: TeamVerb,
}

#[derive(Subcommand, Debug)]
pub enum TeamVerb {
    InitCa,
    TokenCreate,
    TokenList,
    TokenRevoke,
}

#[derive(Args, Debug)]
pub struct LogsArgs {
    /// Number of lines to tail. Default 100.
    #[arg(long, default_value_t = 100)]
    pub lines: usize,
    /// Stream new log lines until SIGINT.
    #[arg(short = 'f', long)]
    pub follow: bool,
}

impl OutputFormat {
    pub fn to_formatter(self) -> crate::formatter::Formatter {
        match self {
            OutputFormat::Text => crate::formatter::Formatter::Text,
            OutputFormat::Json => crate::formatter::Formatter::Json,
            OutputFormat::Yaml => crate::formatter::Formatter::Yaml,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_compiles_and_validates() {
        // Trip clap's debug-only validation: panics if the derive structure
        // is malformed (duplicate flags, missing subcommands, etc.).
        Cli::command().debug_assert();
    }

    #[test]
    fn daemon_start_foreground_parses() {
        let cli = Cli::try_parse_from(["memlayer", "daemon", "start", "--foreground"]).unwrap();
        match cli.command {
            Command::Daemon(DaemonArgs { verb: DaemonVerb::Start { foreground } }) => {
                assert!(foreground);
            }
            other => panic!("expected daemon start, got {other:?}"),
        }
    }

    #[test]
    fn version_subcommand_parses() {
        let cli = Cli::try_parse_from(["memlayer", "version"]).unwrap();
        assert!(matches!(cli.command, Command::Version));
    }

    #[test]
    fn output_flag_is_global() {
        let cli = Cli::try_parse_from(["memlayer", "--output", "json", "daemon", "status"]).unwrap();
        assert_eq!(cli.output, Some(OutputFormat::Json));
    }

    #[test]
    fn project_env_picked_up() {
        // env() is wired via `#[arg(env = "MEMLAYER_PROJECT")]`. Verifying
        // the attribute is present is enough — clap reads the env at parse
        // time but we don't need to mutate global state in this test.
        let cmd = Cli::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "project")
            .expect("--project arg defined");
        assert!(arg.get_env().is_some());
    }
}
