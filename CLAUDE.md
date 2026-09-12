# memlayer — contributor notes for AI coding agents

Companion file for Claude Code / Cursor. Open-source and local-LLM agents
(OpenCode, Codex, Gemini CLI, …) should prefer root [`AGENTS.md`](AGENTS.md),
which carries the same build/architecture rules plus local-model wiring.

## Architecture in one paragraph

The `memlayer` binary is a thin CLI (`crates/memlayer-cli`) that auto-spawns a
per-user gRPC daemon (`crates/memlayer-daemon`) on first call. The daemon owns
one SQLite+FTS5 per-project DB (`crates/memlayer-storage`), a cross-project
BM25 mirror (`global.sqlite`), and per-project write threads. Off the save hot
path: embed worker (BGE-small, candle-rs), extract worker (detected agent CLI),
resolve worker (conflict pairs), and verify worker (code anchors vs git).
Retrieval uses BM25 only or BM25 + dense ANN (sqlite-vec) fused via RRF
(`crates/memlayer-retrieval`). Optional time decay, evidence-window expansion,
and per-type quotas are config-gated (default off / unlimited). The wire
protocol is proto3 gRPC (`proto/memlayer.proto`).

## Crate dependency order (low → high)

```
memlayer-core              (config, paths, error, git, tokens)
memlayer-proto             (tonic-generated stubs)
memlayer-storage           (rusqlite, facts, anchors, conflict_judge trait)
memlayer-embed             (BgeSmallEmbedder, quantize)
memlayer-extract           (agent_cli, ClaudeClient, ClaudeCliExtractor)
memlayer-retrieval         (rrf_fuse / rrf_fuse_scored, ClaudeReranker, HybridMode)
memlayer-daemon            (service, verify, verify_worker, context_filter, token_budget, …)
memlayer-client            (channel builders: UDS + TCP)
memlayer-cli               (memlayer binary; install / git_hooks / mcp)
memlayer-sync              (export/import, .mem archives)
memlayer-mcp               (stdio MCP server)
memlayer-eval              (benchmark harness — not imported by daemon)
memlayer-tests             (integration tests)
```

## Key conventions

- **Migrations** live in `migrations/V{N}__name.sql`, embedded at compile time
  via `refinery::embed_migrations!`. Current head: V10 (verify_state).
  Always use `IF NOT EXISTS` / `ALTER TABLE ... ADD COLUMN` for idempotency.
- **Write thread** per project: `storage::write::spawn_write_thread`. All DB
  mutations go through `WriteRequest` variants on a `crossbeam` bounded channel.
  Use `WriteRequest::Custom` for one-off operations (including verify / anchor
  stamp).
- **Config** (`MemlayerConfig`) uses 3-level TOML merge: env vars >
  `~/.memlayer/projects/<name>.config.toml` > `~/.memlayer/config.toml` >
  code defaults. Re-resolve per task — never cache at daemon startup.
- **Worker pools** (embed, extract, resolve, verify) pattern: `try_queue` drops
  on full, never blocks. Workers re-resolve config per task.
- **ConflictClassifier** trait is defined in `storage::conflict_judge` (no
  external deps); implemented in `daemon::conflict_judge`.
- **sqlite-vec** must be registered before opening any DB that uses vec0:
  `pragmas::ensure_sqlite_vec_extension()`. Called in `db::open_write/read`.
- **Git** helpers shell out to `git` (`memlayer-core::git`); no git2/gix.
- **Tests**: integration tests in `crates/memlayer-tests/` require a live
  daemon socket; they fail under sandbox-restricted envs. Unit tests in each
  crate's `#[cfg(test)] mod tests` run without a daemon.

## How to run

```bash
# Build all (including tests — catches compile errors in test code too)
cargo build --tests --workspace

# Unit tests for a specific module (avoids rebuilding rusqlite from scratch
# when backup.rs is sandbox-blocked — use the cached binary):
BIN=$(find target/debug/deps -maxdepth 1 -name 'memlayer_storage-*' -perm -u+x | tail -1)
$BIN history_chain --test-threads=1

# All unit tests for a crate (via cargo, requires rusqlite compile):
cargo test -p memlayer-storage --lib -- history_chain
cargo test -p memlayer-daemon --lib -- verify::
cargo test -p memlayer-cli --lib -- git_hooks::

# Run daemon in foreground (for debugging):
RUST_LOG=memlayer=debug cargo run -p memlayer-cli -- daemon start --foreground

# LoCoMo / staleness local analysis (scorecards in ./eval/, gitignored)
make eval-locomo-smoke              # fixture, no dataset / LLM
make eval-locomo                    # full locomo10 (needs BGE + agent CLI)
LIMIT=50 make eval-locomo           # cheaper stratified slice (cats 1–4)
# Pin OpenCode flash + concurrency:
#   MEMLAYER_LLM_PROVIDER=opencode MEMLAYER_LLM_BIN=opencode \
#   MEMLAYER_LLM_MODEL=opencode-go/deepseek-v4.1-flash \
#   MEMLAYER_EVAL_CONCURRENCY=4 LIMIT=50 make eval-locomo
# Facts path is default (builds facts.db if missing). Ablation: EXTRACT=0
make eval-locomo-e2e                # smoke then full
make eval-locomo-compare SCORECARD=eval/locomo-full.json
make eval-staleness                 # supersession vs --no-supersede baseline
```

## Config quick-reference (MemlayerConfig)

```toml
# ~/.memlayer/config.toml (global)
# ~/.memlayer/projects/<name>.config.toml (per-project overlay)

[extract]
enabled = false         # opt-in: fact decomposition on every save (uses agent CLI)
model = "fast"          # "fast" | "capable" (aliases: haiku | sonnet)
timeout_secs = 30
workers = 1

[rerank]
model = "fast"          # role used when --rerank is passed / search.rerank = true
timeout_secs = 5

[embed]
workers = 2
quantize = false        # store int8 BLOBs (384 B/obs) instead of float32 (1.5 KB/obs)

[conflict]
enabled = true          # LLM judge for supersession (FTS5 heuristic on judge error)
model = "fast"
timeout_secs = 5

[verify]
serve_stale = false     # context withdraws stale/invalidated/unprovable; search still shows them

[search]
mode = "hybrid"         # "hybrid" | "bm25"
rerank = false          # when true, default rerank model if request omits one
decay_lambda = 0.0      # time decay; 0 = off (eval uses 0.005)
evidence_window = 0     # ±N same-session neighbors in context; 0 = off
max_per_type = 0        # per-type quota; 0 = unlimited
```

**Env overrides** (highest precedence, any session):
`MEMLAYER_EXTRACT_ENABLED`, `MEMLAYER_EXTRACT_MODEL`, `MEMLAYER_EXTRACT_TIMEOUT_SECS`,
`MEMLAYER_EXTRACT_WORKERS`, `MEMLAYER_RERANK_MODEL`, `MEMLAYER_RERANK_TIMEOUT_SECS`,
`MEMLAYER_EMBED_WORKERS`, `MEMLAYER_EMBED_QUANTIZE`,
`MEMLAYER_CONFLICT_ENABLED`, `MEMLAYER_CONFLICT_MODEL`, `MEMLAYER_CONFLICT_TIMEOUT_SECS`,
`MEMLAYER_VERIFY_SERVE_STALE`, `MEMLAYER_SEARCH_MODE`,
`MEMLAYER_SEARCH_DECAY_LAMBDA`, `MEMLAYER_SEARCH_EVIDENCE_WINDOW`,
`MEMLAYER_SEARCH_MAX_PER_TYPE`,
`MEMLAYER_LLM_BIN`, `MEMLAYER_LLM_PROVIDER`, `MEMLAYER_LLM_MODEL`.
Roles (`fast`/`capable`) inherit the invoking agent's current model. Pin with
`MEMLAYER_LLM_MODEL` or a concrete id (`qwen`, `opencode/glm-5.3`). Host hints:
`CURSOR_MODEL`, `OPENCODE_MODEL`, `ANTHROPIC_MODEL`, `GEMINI_MODEL`, `KILO_MODEL`.

## Proto + gRPC surface (current RPCs)

Observation lifecycle: `SaveObservation` (repeated `anchors`), `GetObservation`,
`UpdateObservation`, `DeleteObservation`, `SearchObservations` (`mode`, `rerank`,
`max_tokens` → `tokens_used`), `ListObservations`, `RecentObservations`.

Context / retrieval: `Context` (`mode`, `rerank`, `query`, `include_stale`,
`max_tokens` → `tokens_used`), `Timeline`, `SuggestTopicKey`, `CapturePassive`,
`VerifyAnchors`, `Decide`.

Atomic facts: `GetFacts`, `GetObservationHistory`.

Bulk background ops: `ReextractObservations`, `ReindexObservations`.

Sessions: `StartSession`, `EndSession`, `SaveSessionSummary`, `GetSession`,
`ListSessions`, `DeleteSession`.

Archives: `ExportMem` / `ImportMem` (`.mem`). Sync / Team / Admin / Health /
Doctor RPCs — see `proto/memlayer.proto`.

## Product commands agents should know

```bash
memlayer install                 # skills + MCP; git hooks when cwd is a repo
memlayer install --no-git-hooks  # skip post-commit verify hooks
memlayer obs save --anchor path::symbol --title "…" --content "…"
memlayer verify [--quiet]        # re-check anchors vs HEAD
memlayer obs context --query "…" --max-tokens 500
memlayer decide "…"
memlayer mem export --out backup.mem
```

## Pending roadmap (see `docs/ROADMAP.md`)

- Spec 2 (shipped): MCP — `memlayer mcp` + seven `memory_*` tools; install
  targets Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code,
  ZCode, VS Code / Copilot, Codex, Gemini CLI, Amazon Q, `.agents`
- Spec 3 (partial): anchors + `memlayer verify` + stale withdrawal + git hooks
  shipped; graphify / auto-anchor heuristic still open
- Spec 4 (shipped for supersession): `conflict.enabled`; multi-relation
  `obs judge` still deferred
- Spec 5 (shipped): `obs history <id>`
- Spec 1 deferred items (shipped): int8 quantize, cross-project hybrid,
  `obs reextract`, `memlayer reindex`
- `memlayer-eval` CI scorecard wiring (separate `eval-promotion` spec)
