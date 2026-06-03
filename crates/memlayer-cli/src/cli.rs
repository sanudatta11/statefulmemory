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
    Save(ObsSaveArgs),
    /// Update an existing observation by id.
    Update(ObsUpdateArgs),
    /// Soft-delete (or `--hard`) an observation.
    Delete(ObsDeleteArgs),
    /// Print a single observation.
    Get(ObsGetArgs),
    /// FTS5 search.
    Search(ObsSearchArgs),
    /// Recent observations.
    Recent(ObsRecentArgs),
    /// List observations with filters.
    List(ObsListArgs),
    /// Markdown context summary for prompt injection.
    Context(ObsContextArgs),
    /// Chronological neighbors of an observation.
    Timeline(ObsTimelineArgs),
    /// Suggest a stable topic key for a candidate observation.
    SuggestTopicKey(ObsSuggestTopicKeyArgs),
    /// Extract `## Key Learnings:` bullets from a text block.
    CapturePassive(ObsCapturePassiveArgs),
}

#[derive(Args, Debug)]
pub struct ObsSaveArgs {
    /// Title.
    #[arg(long)]
    pub title: String,
    /// Content body. Pass `-` to read from stdin (capped at 50,000 chars per EC-7).
    #[arg(long)]
    pub content: String,
    /// Observation type: decision, policy, preference, note, learning, …
    #[arg(long, default_value = "note")]
    pub r#type: String,
    /// Visibility scope. SC-12 / FR4 default.
    #[arg(long, default_value = "project")]
    pub scope: String,
    /// Stable topic key for upsert semantics (FR12.5).
    #[arg(long)]
    pub topic: Option<String>,
    /// Session id this observation belongs to.
    #[arg(long)]
    pub session: Option<String>,
}

#[derive(Args, Debug)]
pub struct ObsUpdateArgs {
    /// Observation id (numeric DB id) or sync_id.
    pub id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub content: Option<String>,
    #[arg(long)]
    pub r#type: Option<String>,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long)]
    pub topic: Option<String>,
}

#[derive(Args, Debug)]
pub struct ObsDeleteArgs {
    pub id: String,
    /// Hard-delete (DELETE row + FTS entry) instead of soft-delete.
    #[arg(long)]
    pub hard: bool,
}

#[derive(Args, Debug)]
pub struct ObsGetArgs {
    pub id: String,
}

#[derive(Args, Debug)]
pub struct ObsSearchArgs {
    /// FTS5 query string.
    pub query: String,
    #[arg(long)]
    pub r#type: Option<String>,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long, default_value_t = 10)]
    pub limit: i32,
    /// Search across all projects (capped at 32 per EC-10).
    #[arg(long)]
    pub all_projects: bool,
}

#[derive(Args, Debug)]
pub struct ObsRecentArgs {
    #[arg(long, default_value_t = 10)]
    pub limit: i32,
    #[arg(long)]
    pub scope: Option<String>,
}

#[derive(Args, Debug)]
pub struct ObsListArgs {
    #[arg(long)]
    pub r#type: Option<String>,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long, default_value_t = 10)]
    pub limit: i32,
    /// Opaque cursor token from a previous list page.
    #[arg(long)]
    pub cursor: Option<String>,
    /// Only show decisions whose `review_after` is in the past (SC-27).
    #[arg(long)]
    pub due_for_review: bool,
}

#[derive(Args, Debug)]
pub struct ObsContextArgs {
    #[arg(long, default_value_t = 10)]
    pub limit: i32,
}

#[derive(Args, Debug)]
pub struct ObsTimelineArgs {
    pub id: String,
    #[arg(long, default_value_t = 5)]
    pub before: i32,
    #[arg(long, default_value_t = 5)]
    pub after: i32,
}

#[derive(Args, Debug)]
pub struct ObsSuggestTopicKeyArgs {
    #[arg(long)]
    pub title: String,
    #[arg(long, default_value = "note")]
    pub r#type: String,
    #[arg(long, default_value = "project")]
    pub scope: String,
}

#[derive(Args, Debug)]
pub struct ObsCapturePassiveArgs {
    /// Markdown text to scan. Pass `-` to read from stdin (50k cap).
    #[arg(long)]
    pub text: String,
    #[arg(long)]
    pub session: Option<String>,
}

#[derive(Args, Debug)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub verb: SessionVerb,
}

#[derive(Subcommand, Debug)]
pub enum SessionVerb {
    /// Start (or resume) a session.
    Start(SessionStartArgs),
    /// Mark a session ended, optionally with a summary.
    End(SessionEndArgs),
    /// Save a structured summary onto an existing session.
    Summary(SessionSummaryArgs),
    /// List recent sessions, paginated.
    List(SessionListArgs),
    /// Print one session.
    Get(SessionGetArgs),
    /// Delete a session. FAILED_PRECONDITION if observations reference it (FR12.8).
    Delete(SessionDeleteArgs),
}

#[derive(Args, Debug)]
pub struct SessionStartArgs {
    pub id: String,
    /// Working directory the session is rooted at.
    #[arg(long)]
    pub directory: Option<String>,
}

#[derive(Args, Debug)]
pub struct SessionEndArgs {
    pub id: String,
    #[arg(long)]
    pub summary: Option<String>,
}

#[derive(Args, Debug)]
pub struct SessionSummaryArgs {
    pub id: String,
    #[arg(long)]
    pub content: String,
}

#[derive(Args, Debug)]
pub struct SessionListArgs {
    #[arg(long, default_value_t = 10)]
    pub limit: i32,
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Args, Debug)]
pub struct SessionGetArgs {
    pub id: String,
}

#[derive(Args, Debug)]
pub struct SessionDeleteArgs {
    pub id: String,
}

#[derive(Args, Debug)]
pub struct PromptArgs {
    #[command(subcommand)]
    pub verb: PromptVerb,
}

#[derive(Subcommand, Debug)]
pub enum PromptVerb {
    /// Save a user prompt.
    Save(PromptSaveArgs),
    /// FTS5 search across saved prompts.
    Search(PromptSearchArgs),
    /// Recent prompts, paginated.
    Recent(PromptRecentArgs),
    /// Permanently delete a prompt (FR12.9 — no soft-delete).
    Delete(PromptDeleteArgs),
}

#[derive(Args, Debug)]
pub struct PromptSaveArgs {
    /// Prompt body. Pass `-` to read from stdin (50k cap).
    #[arg(long)]
    pub content: String,
    #[arg(long)]
    pub session: Option<String>,
}

#[derive(Args, Debug)]
pub struct PromptSearchArgs {
    pub query: String,
    #[arg(long, default_value_t = 10)]
    pub limit: i32,
}

#[derive(Args, Debug)]
pub struct PromptRecentArgs {
    #[arg(long, default_value_t = 10)]
    pub limit: i32,
}

#[derive(Args, Debug)]
pub struct PromptDeleteArgs {
    /// Prompt id (numeric DB id) or sync_id.
    pub id: String,
}

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub verb: ProjectVerb,
}

#[derive(Subcommand, Debug)]
pub enum ProjectVerb {
    /// List every project on disk with row counts.
    List,
    /// Show the project name detected from cwd.
    Current,
    /// Merge `from` into `to` (atomic; soft-deletes source observations).
    Merge(ProjectMergeArgs),
    /// Delete a project. `--hard` removes the DB file (admin-only TCP).
    Delete(ProjectDeleteArgs),
    /// Find projects whose names look alike (jaro_winkler ≥ 0.85).
    Consolidate(ProjectConsolidateArgs),
    /// List projects with zero active observations.
    Prune(ProjectPruneArgs),
}

#[derive(Args, Debug)]
pub struct ProjectMergeArgs {
    #[arg(long)]
    pub from: String,
    #[arg(long)]
    pub to: String,
}

#[derive(Args, Debug)]
pub struct ProjectDeleteArgs {
    pub name: String,
    #[arg(long)]
    pub hard: bool,
}

#[derive(Args, Debug)]
pub struct ProjectConsolidateArgs {
    /// Show candidates without performing merges.
    #[arg(long)]
    pub dry_run: bool,
    /// Include similar pairs from every project, not just the current one's neighborhood.
    #[arg(long)]
    pub all: bool,
}

#[derive(Args, Debug)]
pub struct ProjectPruneArgs {
    /// Show candidates without removing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    #[command(subcommand)]
    pub verb: SyncVerb,
}

#[derive(Subcommand, Debug)]
pub enum SyncVerb {
    /// Sync status — only verb in Spec 2; export/import land in Spec 3.
    Status(SyncStatusArgs),
}

#[derive(Args, Debug)]
pub struct SyncStatusArgs {
    /// Filter to a single project; defaults to the detected project.
    #[arg(long)]
    pub project: Option<String>,
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
