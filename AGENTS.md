# AGENTS.md — memlayer for OpenCode, Codex, and local / open-source LLMs

Cross-agent project instructions ([agents.md](https://agents.md/) convention).
Read by OpenCode, Codex, Gemini CLI, Cursor, and peers. Claude Code also has
[`CLAUDE.md`](CLAUDE.md) with the same architecture notes.

If both `AGENTS.md` and `CLAUDE.md` exist, OpenCode prefers this file.

## What this repo is

Local, per-project persistent memory for coding agents. Thin CLI → user-local
gRPC daemon → SQLite (FTS5 + optional sqlite-vec). No cloud required for
search/context. Optional LLM steps (extract, conflict judge, rerank, Decide)
shell out to whichever agent CLI is on `PATH`.

## Architecture (one paragraph)

`memlayer` (`crates/memlayer-cli`) auto-spawns `memlayer-daemon` on first use.
Per-project SQLite + FTS5, cross-project BM25 mirror (`global.sqlite`), write
threads. Background pools: embed (BGE-small), extract, resolve, verify (code
anchors vs git). Default retrieval is hybrid BM25 + dense RRF. Protocol:
`proto/memlayer.proto`.

## Crate order (low → high)

```
memlayer-core → memlayer-proto → memlayer-storage → memlayer-embed
→ memlayer-extract → memlayer-retrieval → memlayer-daemon → memlayer-client
→ memlayer-cli / memlayer-mcp / memlayer-sync
memlayer-eval (benchmarks only) · memlayer-tests (needs live daemon)
```

## Conventions

- Migrations: `migrations/V{N}__name.sql`, head **V10** (`verify_state`).
  Prefer `IF NOT EXISTS` / additive `ALTER TABLE`.
- All DB writes go through the per-project write thread (`WriteRequest`).
  Never open a second write connection from workers/RPC handlers.
- Config: env > `~/.memlayer/projects/<name>.config.toml` >
  `~/.memlayer/config.toml` > code defaults. Re-resolve per task.
- Worker pools: `try_queue` drops on full; never block the save path.
- Git: shell out only (`memlayer-core::git`). No git2/gix.
- Do not invent third-party memory product names in user-facing docs/UI.

## Build / test / debug

```bash
cargo build --tests --workspace
cargo test -p memlayer-storage --lib -- history_chain
cargo test -p memlayer-daemon --lib -- verify::
cargo test -p memlayer-cli --lib -- git_hooks::
cargo test -p memlayer-core --lib -- tokens::

RUST_LOG=memlayer=debug cargo run -p memlayer-cli -- daemon start --foreground

make eval-locomo-smoke
make eval-staleness
LIMIT=50 make eval-locomo
```

Full LoCoMo on AWS (one-shot EC2, scorecards → S3): see
[`infra/eval/README.md`](infra/eval/README.md).

Integration tests under `crates/memlayer-tests/` need a live daemon socket.

## Config (defaults that matter)

```toml
[search]
mode = "hybrid"
rerank = false
decay_lambda = 0.0      # off
evidence_window = 0     # off
max_per_type = 0        # unlimited

[verify]
serve_stale = false     # withdraw bad anchors from context

[conflict]
enabled = true          # LLM supersession judge

[extract]
enabled = false         # keep opt-in (LLM on every save otherwise)

[embed]
quantize = false
```

`memlayer install` writes a bootstrap `~/.memlayer/config.toml` (hybrid +
conflict + extract on) without clobbering existing keys. Git hooks
(`post-commit` / `post-merge` / `post-checkout` → `memlayer verify --quiet`)
install when cwd is a git repo; skip with `--no-git-hooks`.

## Local / open-source LLM wiring

memlayer does **not** embed an OpenAI/Anthropic SDK for these steps. It detects
an agent CLI and passes prompts on stdin / argv.

| Preference | How |
|---|---|
| Force binary | `MEMLAYER_LLM_BIN=/path/to/opencode` (or `cursor-agent`, `gemini`, `codex`, `kilo`, …) |
| Force provider id | `MEMLAYER_LLM_PROVIDER=opencode` |
| Pin model | `MEMLAYER_LLM_MODEL=qwen` or `opencode/glm-5.3` |
| Inherit session model | leave roles as `fast` / `capable`; set host env if available |

Host model hints checked: `OPENCODE_MODEL`, `CURSOR_MODEL`, `GEMINI_MODEL`,
`ANTHROPIC_MODEL`, `KILO_MODEL`, …

OpenCode / Kilo Zen catalog nicknames (`qwen`, `glm`, `deepseek`, …) resolve
in `crates/memlayer-extract/src/opencode_models.rs`.

Examples:

```bash
# OpenCode + a concrete Zen model for extract/judge
export MEMLAYER_LLM_PROVIDER=opencode
export MEMLAYER_LLM_MODEL=opencode/qwen3.7-plus

# Gemini CLI as the only LLM backend
export MEMLAYER_LLM_BIN=gemini
export MEMLAYER_LLM_PROVIDER=gemini

# Disable LLM-backed features entirely (local retrieval only)
memlayer config set extract.enabled false
memlayer config set conflict.enabled false
memlayer config set search.rerank false
```

Hybrid **search** stays local (BGE-small + BM25) even when no agent CLI is
installed. Only extract / conflict / rerank / Decide need a CLI.

## Install targets (`memlayer install`)

Auto-detect or pass `--agent <id>` / `--all`:

`claude-code`, `cursor`, `windsurf`, `antigravity`, `opencode`, `kimi-code`,
`zcode`, `agents`, `vscode`, `copilot-cli`, `copilot`, `gemini`, `codex`,
`amazon-q`.

OpenCode config: `~/.config/opencode/opencode.json` (MCP). Shared skills also
land under `.agents/` when that target is selected.

## Commands useful while coding in this repo

```bash
memlayer obs save --type decision --title "…" --content "…" --session "$SID"
memlayer obs save --anchor src/foo.rs::bar --title "…" --content "…"
memlayer obs search "…" --mode hybrid
memlayer obs context --query "…" --max-tokens 500
memlayer verify
memlayer decide "Should we …?"
memlayer mem export --out backup.mem
```

MCP tools (stdio via `memlayer mcp`): `memory_search`, `memory_recent`,
`memory_context`, `memory_add`, `memory_facts`, `memory_health`,
`memory_decide`.

## RPC / schema reminders

- Search/context accept `max_tokens`; responses include estimated `tokens_used`
  (`memlayer-core::tokens::estimate_tokens`, not tiktoken).
- Context withdraws `stale` / `invalidated` / `unprovable` unless
  `verify.serve_stale` or `--include-stale`. Never hides `unanchored`.
- Save accepts repeated `--anchor` / proto `anchors`; stamps commit + digest
  when cwd (or project repo) is a git work tree.

## Roadmap status (short)

Shipped: hybrid default, MCP, Decide, `.mem` archives, in-force
`supersedes_ids`, staleness eval, anchors + verify + git hooks, optional decay /
evidence window / token budget.

Still open: graphify bridge / auto-anchor, multi-relation `obs judge`, eval CI
scorecard promotion. Details: `docs/ROADMAP.md`.
