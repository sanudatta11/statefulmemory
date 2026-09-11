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

    /// Project name override. Skips detection (FR3.4). The
    /// `MEMLAYER_PROJECT` env var is honored too, but `main.rs` reads it
    /// directly so detection can attribute the source as `env_override`
    /// rather than `cli_flag` when only the env var is set.
    #[arg(long, global = true)]
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
    /// Install skills, agent rules, hooks, and MCP registration
    /// (`memlayer mcp`) for detected coding agents (skills.sh-style).
    /// Use `--all` or `--agent` to override auto-detection.
    Install(InstallArgs),
    /// Remove memlayer skill files, settings patches, and MCP registration.
    Uninstall,
    /// Wipe all stored observations and stop the daemon.
    Clean,
    /// Lifecycle hooks invoked by Claude Code / agent runtimes.
    Hook(HookArgs),
    /// Show / read / write retrieval-pipeline config (extract / rerank /
    /// embed). Backed by `~/.memlayer/config.toml` (global) and
    /// `~/.memlayer/projects/<name>.config.toml` (per-project).
    Config(ConfigArgs),
    /// Re-index all observations into the vector store. Skeleton verb in
    /// v1 — prints manual instructions; full implementation in a follow-up
    /// spec.
    Reindex(ReindexArgs),
    /// Run the local stdio MCP server, exposing memory tools to MCP-capable
    /// agents (Claude Code, Windsurf). Speaks MCP on stdout; logs to stderr.
    Mcp,
    /// Run accuracy and latency retrieval evaluation benchmark (LoCoMo, LongMemEval, BEAM).
    Eval(EvalArgs),
    /// Run database integrity audit and auto-repair routines.
    Doctor(DoctorArgs),
    /// Launch interactive TUI observation browser.
    Tui(TuiArgs),
    /// Print version and exit.
    Version,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Attempt non-destructive automatic repair of schema, FTS indexes, and orphan vectors.
    #[arg(long, aliases = ["auto-repair", "auto_repair"])]
    pub repair: bool,
}

#[derive(Args, Debug)]
pub struct TuiArgs {
    /// Initial search query filter for TUI browser.
    #[arg(short = 's', long)]
    pub query: Option<String>,
}

#[derive(Args, Debug)]
pub struct EvalArgs {
    /// Benchmark to evaluate: locomo, longmemeval, beam1m, beam10m. Default: locomo.
    #[arg(long, default_value = "locomo")]
    pub benchmark: String,

    /// Run in smoke mode (stops after 5 queries or --limit N).
    #[arg(long)]
    pub smoke: bool,

    /// Limit maximum number of queries evaluated.
    #[arg(long)]
    pub limit: Option<usize>,

    /// Save JSON benchmark scorecard to specified file path.
    #[arg(long)]
    pub save_scorecard: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Install for every known agent (skip auto-detection).
    #[arg(long)]
    pub all: bool,

    /// Target specific agents (repeatable). Example: `--agent cursor --agent claude-code`.
    /// Accepted ids: claude-code, cursor, windsurf, antigravity, opencode,
    /// kimi-code, zcode, agents, vscode, copilot-cli, copilot, gemini, codex, amazon-q.
    #[arg(long = "agent", short = 'a', value_name = "AGENT")]
    pub agents: Vec<String>,
}

#[derive(Args, Debug)]
pub struct HookArgs {
    #[command(subcommand)]
    pub verb: HookVerb,
}

#[derive(Subcommand, Debug)]
pub enum HookVerb {
    /// PreToolUse hook: surface relevant prior observations before the
    /// agent runs Grep / Read. Always exits 0 so the agent's tool call
    /// is never blocked.
    PreTool(PreToolArgs),
    /// SessionStart hook: verify the daemon is up (repairing a stale socket
    /// if needed), then print the memory briefing. Always exits 0.
    SessionStart(SessionStartHookArgs),
}

#[derive(Args, Debug)]
pub struct SessionStartHookArgs {
    /// Number of recent observations to include in the briefing.
    #[arg(long, default_value_t = 20)]
    pub limit: i32,
}

#[derive(Args, Debug)]
pub struct PreToolArgs {
    /// Which tool the agent is about to invoke (Grep | Read).
    #[arg(long)]
    pub tool: String,
    /// Grep pattern argument (--tool Grep only).
    #[arg(long)]
    pub pattern: Option<String>,
    /// Read file path (--tool Read only).
    #[arg(long)]
    pub path: Option<String>,
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
    /// Print atomic facts attached to an observation (retrieval-promotion).
    Facts(ObsFactsArgs),
    /// Stub: re-extract facts from observations since a date.
    Reextract(ObsReextractArgs),
    /// Print the full supersession history of an observation (oldest → newest).
    History(ObsHistoryArgs),
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
    /// Optional code anchor (e.g. "src/auth.rs::validate_token::42").
    #[arg(long)]
    pub anchor: Option<String>,
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
    #[arg(long)]
    pub anchor: Option<String>,
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
    /// Retrieval mode: `bm25` (default for v1.x back-compat) or `hybrid`
    /// (BM25 + dense ANN top-30 fused via RRF). Retrieval-promotion SC-3,
    /// SC-4.
    #[arg(long, default_value = "bm25", value_parser = ["bm25", "hybrid"])]
    pub mode: String,
    /// Optional reranker model: `haiku` or `sonnet`. Hard 5s timeout per
    /// SC-5; on timeout the un-reranked hybrid result is returned.
    #[arg(long, value_parser = ["haiku", "sonnet"])]
    pub rerank: Option<String>,
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
    /// Optional query to focus the context window on. When set together
    /// with `--mode hybrid`, the daemon RRF-fuses BM25 and dense matches.
    #[arg(long)]
    pub query: Option<String>,
    /// Retrieval mode for the context window: `bm25` (default) or `hybrid`.
    #[arg(long, default_value = "bm25", value_parser = ["bm25", "hybrid"])]
    pub mode: String,
    /// Optional reranker model: `haiku` or `sonnet`. 5s timeout, falls
    /// back on error.
    #[arg(long, value_parser = ["haiku", "sonnet"])]
    pub rerank: Option<String>,
    /// Optional code anchor to filter context by code path/symbol.
    #[arg(long)]
    pub anchor: Option<String>,
}

#[derive(Args, Debug)]
pub struct ObsFactsArgs {
    /// Observation id (numeric DB id) or sync_id.
    pub id: String,
}

#[derive(Args, Debug)]
pub struct ObsReextractArgs {
    /// Re-extract facts from observations created on or after this date
    /// (RFC-3339). Skeleton verb in v1; emits a deferred-feature notice.
    #[arg(long)]
    pub since: Option<String>,
}

#[derive(Args, Debug)]
pub struct ObsHistoryArgs {
    /// Numeric observation id whose supersession history to display.
    pub id: String,
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

// -----------------------------------------------------------------------------
// `memlayer config` — view / edit retrieval-pipeline tunables (rp-t11).
// -----------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub verb: ConfigVerb,
}

#[derive(Subcommand, Debug)]
pub enum ConfigVerb {
    /// Print the resolved config (env > project > global > defaults). Use
    /// `--raw` to dump the global TOML file verbatim instead.
    Show(ConfigShowArgs),
    /// Print one resolved value, e.g. `memlayer config get extract.model`.
    Get(ConfigGetArgs),
    /// Set a value in the global config (or per-project with `--project`).
    /// Atomic write: tempfile + rename.
    Set(ConfigSetArgs),
}

#[derive(Args, Debug)]
pub struct ConfigShowArgs {
    /// Print the global config.toml file verbatim instead of the resolved
    /// merged view.
    #[arg(long)]
    pub raw: bool,
    /// Resolve config for this project (overlays
    /// `~/.memlayer/projects/<name>.config.toml` on top of the global
    /// file). Defaults to no project (global view only).
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Args, Debug)]
pub struct ConfigGetArgs {
    /// Dotted key, e.g. `extract.model`, `rerank.timeout_secs`,
    /// `embed.workers`.
    pub key: String,
    /// Resolve in the context of this project.
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Args, Debug)]
pub struct ConfigSetArgs {
    /// Dotted key, e.g. `extract.model`.
    pub key: String,
    /// New value. Parsed as bool (`true`/`false`), integer, or string in
    /// that order.
    pub value: String,
    /// Write to the per-project file
    /// `~/.memlayer/projects/<name>.config.toml` instead of the global one.
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Args, Debug)]
pub struct ReindexArgs {
    /// Reserved for future use; the v1 stub ignores all flags and prints
    /// manual reindex instructions.
    #[arg(long)]
    pub force: bool,
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
    /// Generate a rolled-up session summary from observations and persist
    /// it as a project-scoped note (Engram-style auto-rollup).
    Summarize(SessionSummarizeArgs),
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
pub struct SessionSummarizeArgs {
    /// Session id to summarize.
    pub id: String,
    /// Mark this as an auto-generated rollup (default true). Set --no-auto for
    /// agent-supplied prose mode.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub auto: bool,
    /// Read agent-supplied summary content from stdin (`-`) or as a literal
    /// string. When set, overrides the heuristic auto-rollup body.
    #[arg(long)]
    pub content: Option<String>,
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
    ForceStart,
}

#[derive(Args, Debug)]
pub struct TeamArgs {
    #[command(subcommand)]
    pub verb: TeamVerb,
}

#[derive(Subcommand, Debug)]
pub enum TeamVerb {
    /// Generate a self-signed CA + leaf cert for TCP-mode hosting.
    InitCa(TeamInitCaArgs),
    /// Mint a fresh bearer token (admin-only TCP).
    TokenCreate(TeamTokenCreateArgs),
    /// List active token names + admin flags (no secrets).
    TokenList,
    /// Revoke a token by name.
    TokenRevoke(TeamTokenRevokeArgs),
}

#[derive(Args, Debug)]
pub struct TeamInitCaArgs {
    /// Output directory for ca.pem, server.pem, server-key.pem.
    pub dir: std::path::PathBuf,
    /// Overwrite existing PEM files (EC-9).
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct TeamTokenCreateArgs {
    #[arg(long)]
    pub name: String,
    /// Mark this token as admin (gates Shutdown / token RPCs / DeleteProject --hard).
    #[arg(long)]
    pub admin: bool,
}

#[derive(Args, Debug)]
pub struct TeamTokenRevokeArgs {
    pub name: String,
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
    fn mcp_subcommand_parses() {
        let cli = Cli::try_parse_from(["memlayer", "mcp"]).unwrap();
        assert!(matches!(cli.command, Command::Mcp));
    }

    #[test]
    fn hook_session_start_parses_with_default_limit() {
        let cli = Cli::try_parse_from(["memlayer", "hook", "session-start"]).unwrap();
        match cli.command {
            Command::Hook(h) => match h.verb {
                HookVerb::SessionStart(a) => assert_eq!(a.limit, 20),
                other => panic!("expected SessionStart, got {other:?}"),
            },
            other => panic!("expected Hook, got {other:?}"),
        }
    }

    #[test]
    fn hook_session_start_limit_override_parses() {
        let cli =
            Cli::try_parse_from(["memlayer", "hook", "session-start", "--limit", "5"]).unwrap();
        match cli.command {
            Command::Hook(h) => match h.verb {
                HookVerb::SessionStart(a) => assert_eq!(a.limit, 5),
                other => panic!("expected SessionStart, got {other:?}"),
            },
            other => panic!("expected Hook, got {other:?}"),
        }
    }

    #[test]
    fn daemon_force_start_parses() {
        let cli = Cli::try_parse_from(["memlayer", "daemon", "force-start"]).unwrap();
        match cli.command {
            Command::Daemon(DaemonArgs { verb: DaemonVerb::ForceStart }) => {}
            other => panic!("expected daemon force-start, got {other:?}"),
        }
    }

    #[test]
    fn output_flag_is_global() {
        let cli = Cli::try_parse_from(["memlayer", "--output", "json", "daemon", "status"]).unwrap();
        assert_eq!(cli.output, Some(OutputFormat::Json));
    }

    #[test]
    fn project_flag_parses_without_env_binding() {
        // The clap-level `env=` binding was removed deliberately so that
        // `main.rs` can read MEMLAYER_PROJECT separately and attribute the
        // detection source as `env_override` (not `cli_flag`). Verify the
        // arg is still defined and accepts a value, but no longer reads
        // from the environment.
        let cmd = Cli::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "project")
            .expect("--project arg defined");
        assert!(arg.get_env().is_none(), "project arg should not have env binding");
        let cli = Cli::try_parse_from(["memlayer", "--project", "explicit", "version"]).unwrap();
        assert_eq!(cli.project.as_deref(), Some("explicit"));
    }

    fn parse_obs_search(args: &[&str]) -> ObsSearchArgs {
        let mut full = vec!["memlayer", "obs", "search"];
        full.extend_from_slice(args);
        let cli = Cli::try_parse_from(full).expect("obs search must parse");
        match cli.command {
            Command::Obs(o) => match o.verb {
                ObsVerb::Search(a) => a,
                other => panic!("expected Search, got {other:?}"),
            },
            other => panic!("expected Obs, got {other:?}"),
        }
    }

    #[test]
    fn obs_search_default_mode_bm25() {
        let a = parse_obs_search(&["the query"]);
        assert_eq!(a.mode, "bm25", "back-compat default per SC-4");
        assert!(a.rerank.is_none());
    }

    #[test]
    fn obs_search_mode_hybrid_parses() {
        let a = parse_obs_search(&["q", "--mode", "hybrid"]);
        assert_eq!(a.mode, "hybrid");
    }

    #[test]
    fn obs_search_rerank_haiku_parses() {
        let a = parse_obs_search(&["q", "--rerank", "haiku"]);
        assert_eq!(a.rerank.as_deref(), Some("haiku"));
    }

    #[test]
    fn obs_search_rerank_sonnet_parses() {
        let a = parse_obs_search(&["q", "--rerank", "sonnet"]);
        assert_eq!(a.rerank.as_deref(), Some("sonnet"));
    }

    #[test]
    fn obs_search_invalid_mode_rejected() {
        let res = Cli::try_parse_from(["memlayer", "obs", "search", "q", "--mode", "lexical"]);
        assert!(res.is_err(), "value_parser must reject 'lexical'");
    }

    #[test]
    fn obs_search_invalid_rerank_rejected() {
        let res = Cli::try_parse_from(["memlayer", "obs", "search", "q", "--rerank", "opus"]);
        assert!(res.is_err(), "value_parser must reject 'opus'");
    }

    #[test]
    fn obs_context_flags_parse() {
        let cli = Cli::try_parse_from([
            "memlayer", "obs", "context", "--query", "deploy", "--mode", "hybrid", "--rerank", "haiku",
        ])
        .unwrap();
        match cli.command {
            Command::Obs(o) => match o.verb {
                ObsVerb::Context(a) => {
                    assert_eq!(a.mode, "hybrid");
                    assert_eq!(a.rerank.as_deref(), Some("haiku"));
                    assert_eq!(a.query.as_deref(), Some("deploy"));
                }
                other => panic!("expected Context, got {other:?}"),
            },
            other => panic!("expected Obs, got {other:?}"),
        }
    }

    #[test]
    fn obs_facts_verb_parses() {
        let cli = Cli::try_parse_from(["memlayer", "obs", "facts", "42"]).unwrap();
        match cli.command {
            Command::Obs(o) => match o.verb {
                ObsVerb::Facts(a) => assert_eq!(a.id, "42"),
                other => panic!("expected Facts, got {other:?}"),
            },
            other => panic!("expected Obs, got {other:?}"),
        }
    }

    #[test]
    fn obs_history_verb_parses() {
        let cli = Cli::try_parse_from(["memlayer", "obs", "history", "99"]).unwrap();
        match cli.command {
            Command::Obs(o) => match o.verb {
                ObsVerb::History(a) => assert_eq!(a.id, "99"),
                other => panic!("expected History, got {other:?}"),
            },
            other => panic!("expected Obs, got {other:?}"),
        }
    }

    #[test]
    fn daemon_restart_still_parses() {
        let cli = Cli::try_parse_from(["memlayer", "daemon", "restart"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Daemon(DaemonArgs { verb: DaemonVerb::Restart })
        ));
    }

    #[test]
    fn daemon_stop_still_parses() {
        let cli = Cli::try_parse_from(["memlayer", "daemon", "stop"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Daemon(DaemonArgs { verb: DaemonVerb::Stop })
        ));
    }

    #[test]
    fn daemon_status_still_parses() {
        let cli = Cli::try_parse_from(["memlayer", "daemon", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Daemon(DaemonArgs { verb: DaemonVerb::Status })
        ));
    }

    #[test]
    fn hook_pre_tool_grep_parses() {
        let cli = Cli::try_parse_from([
            "memlayer", "hook", "pre-tool", "--tool", "Grep", "--pattern", "foo",
        ])
        .unwrap();
        match cli.command {
            Command::Hook(h) => match h.verb {
                HookVerb::PreTool(a) => {
                    assert_eq!(a.tool, "Grep");
                    assert_eq!(a.pattern.as_deref(), Some("foo"));
                    assert!(a.path.is_none());
                }
                other => panic!("expected PreTool, got {other:?}"),
            },
            other => panic!("expected Hook, got {other:?}"),
        }
    }

    #[test]
    fn hook_pre_tool_read_parses() {
        let cli = Cli::try_parse_from([
            "memlayer", "hook", "pre-tool", "--tool", "Read", "--path", "/tmp/x.rs",
        ])
        .unwrap();
        match cli.command {
            Command::Hook(h) => match h.verb {
                HookVerb::PreTool(a) => {
                    assert_eq!(a.tool, "Read");
                    assert_eq!(a.path.as_deref(), Some("/tmp/x.rs"));
                    assert!(a.pattern.is_none());
                }
                other => panic!("expected PreTool, got {other:?}"),
            },
            other => panic!("expected Hook, got {other:?}"),
        }
    }

    #[test]
    fn hook_session_start_rejects_non_integer_limit() {
        let res = Cli::try_parse_from(["memlayer", "hook", "session-start", "--limit", "abc"]);
        assert!(res.is_err(), "non-integer limit must be rejected by clap");
    }
}
