# AGENTS.md — statefulmemory for OpenCode, Codex, and local / open-source LLMs

Cross-agent project instructions ([agents.md](https://agents.md/) convention).
Read by OpenCode, Codex, Gemini CLI, Cursor, and peers. Claude Code also has
[`CLAUDE.md`](CLAUDE.md) with the same architecture notes.

If both `AGENTS.md` and `CLAUDE.md` exist, OpenCode prefers this file.

## What this repo is

Local, per-project persistent memory for coding agents. Thin CLI → gRPC
daemon → SQLite (FTS5 + optional sqlite-vec). Self-host (UDS or team TCP) or
StatefulMemory Cloud (managed SaaS). Optional LLM steps (extract, conflict judge,
rerank, Decide) shell out to whichever agent CLI is on `PATH`. On self-host,
hybrid search/context does not require a third-party search API key.

**CLI:** primary binary `statefulmemory`; shorthand **`smem`** (same binary).
Data dir `~/.statefulmemory`; env prefix `STATEFULMEMORY_*`. Clean break from
legacy `memlayer`: `smem uninstall --purge` then `smem install`.

**Research skills (Wave 2 effective-context work):** agents implementing
HippoRAG PPR / LongMemEval indexing / eval should load
`firecrawl-research-papers`, `literature-search-arxiv`, and `paper-to-code`
from `~/.agents/skills/`. Those are agent tooling, not the product skill under
`skills/statefulmemory`.

## Architecture (one paragraph)

`statefulmemory` (`crates/statefulmemory-cli`) auto-spawns `statefulmemory-daemon` on first use.
Per-project SQLite + FTS5, cross-project BM25 mirror (`global.sqlite`), write
threads. Background pools: embed (BGE-small), extract, resolve, verify (code
anchors vs git). Default retrieval is hybrid BM25 + dense RRF. Protocol:
`proto/statefulmemory.proto`.

## Crate order (low → high)

```
statefulmemory-core → statefulmemory-proto → statefulmemory-storage → statefulmemory-embed
→ statefulmemory-extract → statefulmemory-retrieval → statefulmemory-daemon → statefulmemory-client
→ statefulmemory-cli / statefulmemory-mcp / statefulmemory-sync
statefulmemory-eval (benchmarks only) · statefulmemory-tests (needs live daemon)
```

## Conventions

- Migrations: `migrations/V{N}__name.sql`, head **V11** (`graph_briefing`).
  Prefer `IF NOT EXISTS` / additive `ALTER TABLE`.
- All DB writes go through the per-project write thread (`WriteRequest`).
  Never open a second write connection from workers/RPC handlers.
- Config: env > `~/.statefulmemory/projects/<name>.config.toml` >
  `~/.statefulmemory/config.toml` > code defaults. Re-resolve per task.
- Worker pools: `try_queue` drops on full; never block the save path.
- Git: shell out only (`statefulmemory-core::git`). No git2/gix.
- Do not name third-party memory products in product UI / CLI help / MCP
  blurbs. Comparison and eval docs (`why-statefulmemory`, LoCoMo, baselines) may
  name them with sources and unmatched-harness caveats. Never invent peer
  features.

## Build / test / debug

```bash
cargo build --tests --workspace
cargo test -p statefulmemory-storage --lib -- history_chain
cargo test -p statefulmemory-daemon --lib -- verify::
cargo test -p statefulmemory-cli --lib -- git_hooks::
cargo test -p statefulmemory-core --lib -- tokens::

RUST_LOG=statefulmemory=debug cargo run -p statefulmemory-cli --bin statefulmemory -- daemon start --foreground

make eval-locomo-smoke
make eval-staleness
LIMIT=50 make eval-locomo
```

Pinned LoCoMo measure (OpenCode deepseek-flash, concurrency 4):

```bash
export STATEFULMEMORY_LLM_PROVIDER=opencode STATEFULMEMORY_LLM_BIN=opencode
export STATEFULMEMORY_LLM_MODEL=opencode-go/deepseek-v4.1-flash
export STATEFULMEMORY_EVAL_CONCURRENCY=4
LIMIT=50 make eval-locomo
# Facts path is default (builds facts.db if missing). Ablation: EXTRACT=0
# Optional capable pin:
# EXTRACT=force LIMIT=50 make eval-locomo
```

Full LoCoMo on AWS (one-shot EC2, scorecards → S3): see
[`infra/eval/README.md`](infra/eval/README.md).

Integration tests under `crates/statefulmemory-tests/` need a live daemon socket.

## Config (defaults that matter)

```toml
[search]
mode = "hybrid"
rerank = true           # local CE by default ([rerank].backend)
router = "adaptive"     # Easy skips dense; Hard enables expand+graph
decay_lambda = 0.0      # off
evidence_window = 0     # off
max_per_type = 0        # unlimited

[rerank]
backend = "local"       # "local" | "llm" | "off"
top_n = 16
timeout_secs = 5

[verify]
serve_stale = false     # withdraw bad anchors from context

[conflict]
enabled = true          # LLM supersession judge

[extract]
enabled = false         # keep opt-in (LLM on every save otherwise)

[embed]
quantize = false

[graph]
enabled = false
ranker = "ppr"          # when graph.enabled
```

`statefulmemory install` writes a bootstrap `~/.statefulmemory/config.toml` (hybrid +
conflict + extract on) without clobbering existing keys. Git hooks
(`post-commit` / `post-merge` / `post-checkout` → `statefulmemory verify --quiet`)
install when cwd is a git repo; skip with `--no-git-hooks`.

## Local / open-source LLM wiring

statefulmemory does **not** embed an OpenAI/Anthropic SDK for these steps. It detects
an agent CLI and passes prompts on stdin / argv.

| Preference | How |
|---|---|
| Force binary | `STATEFULMEMORY_LLM_BIN=/path/to/opencode` (or `cursor-agent`, `gemini`, `codex`, `kilo`, …) |
| Force provider id | `STATEFULMEMORY_LLM_PROVIDER=opencode` |
| Pin model | `STATEFULMEMORY_LLM_MODEL=qwen` or `opencode/glm-5.3` |
| Inherit session model | leave roles as `fast` / `capable`; set host env if available |

Host model hints checked: `OPENCODE_MODEL`, `CURSOR_MODEL`, `GEMINI_MODEL`,
`ANTHROPIC_MODEL`, `KILO_MODEL`, …

OpenCode / Kilo Zen catalog nicknames (`qwen`, `glm`, `deepseek`, …) resolve
in `crates/statefulmemory-extract/src/opencode_models.rs`.

Examples:

```bash
# OpenCode + a concrete Zen model for extract/judge
export STATEFULMEMORY_LLM_PROVIDER=opencode
export STATEFULMEMORY_LLM_MODEL=opencode/qwen3.7-plus

# Gemini CLI as the only LLM backend
export STATEFULMEMORY_LLM_BIN=gemini
export STATEFULMEMORY_LLM_PROVIDER=gemini

# Disable LLM-backed features entirely (local retrieval only)
statefulmemory config set extract.enabled false
statefulmemory config set conflict.enabled false
statefulmemory config set search.rerank false
```

Hybrid **search** stays local (BGE-small + BM25) even when no agent CLI is
installed. Only extract / conflict / rerank / Decide need a CLI.

## Install targets (`statefulmemory install`)

Auto-detect or pass `--agent <id>` / `--all`:

`claude-code`, `cursor`, `windsurf`, `antigravity`, `opencode`, `kimi-code`,
`zcode`, `agents`, `vscode`, `copilot-cli`, `copilot`, `gemini`, `codex`,
`amazon-q`.

OpenCode config: `~/.config/opencode/opencode.json` (MCP). Shared skills also
land under `.agents/` when that target is selected.

## Commands useful while coding in this repo

```bash
statefulmemory obs save --type decision --title "…" --content "…" --session "$SID"
statefulmemory obs save --anchor src/foo.rs::bar --title "…" --content "…"
statefulmemory obs search "…" --mode hybrid
statefulmemory obs context --query "…" --max-tokens 500
statefulmemory verify
statefulmemory decide "Should we …?"
smem context compile --query "…" --window 32k
smem ingest repo --dry-run
statefulmemory mem export --out backup.mem
statefulmemory graph query <entity> --hops 2
statefulmemory graph rebuild         # backfill entities/edges from anchors
statefulmemory graph stats
statefulmemory dream run --review    # consolidation scan (review-only, no LLM)
```

MCP tools (stdio via `statefulmemory mcp`): `memory_search`, `memory_recent`,
`memory_context`, `memory_add`, `memory_facts`, `memory_health`,
`memory_decide`, `memory_graph_query`.

## RPC / schema reminders

- Search/context accept `max_tokens`; responses include estimated `tokens_used`
  (`statefulmemory-core::tokens::estimate_tokens`, not tiktoken).
- Context withdraws `stale` / `invalidated` / `unprovable` unless
  `verify.serve_stale` or `--include-stale`. Never hides `unanchored`.
- Save accepts repeated `--anchor` / proto `anchors`; stamps commit + digest
  when cwd (or project repo) is a git work tree.

## Entity graph (spec: .catalyst/specs/graph-briefing/spec.md)

- Schema head **V11** (`graph_briefing`): `entities`, `entity_mentions`,
  `entity_edges`. Migrations still under `migrations/`, refinery-embedded.
- Config `[graph]` (default **off** until CI multi-hop gate +5 pts passes):
  `enabled`, `hops` (≤2), `boost`, `edge_types`, `budget_pct`, `degree_cap`,
  `max_query_entities`, `ranker` (`bfs` | `ppr`), `ppr_damping`, `ppr_iters`.
  Env: `STATEFULMEMORY_GRAPH_ENABLED/HOPS/BOOST/RANKER`.
- Entity extraction is **heuristics-only** (`statefulmemory-extract::entity_resolve`),
  no LLM on save or retrieval. `obs context` graph expansion is budget-capped
  (`budget_pct` × `max_tokens`) and never displaces primary hits. With
  `graph.ranker = ppr`, expansion uses HippoRAG-style Personalized PageRank
  over `entity_edges` (node specificity = 1/mention_count).
- Wave 2 retrieve: fact-augmented query merge + heuristic time prune
  (LongMemEval indexing tips); `smem context compile` / `smem ingest repo`.
- Writes ride `WriteRequest::IndexGraph` on the per-project write thread — never
  a second write connection.
- RPCs (additive): `ListEntities`, `GetEntity`, `GraphQuery`.

## Roadmap status (short)

Shipped: hybrid default, MCP, Decide, `.mem` archives (**v2 now carries the
entity graph**), in-force `supersedes_ids`, staleness eval, anchors + verify +
git hooks, optional decay / evidence window / token budget, **entity graph (V11)
+ `graph` CLI + MCP tool + CI gates (graph-smoke / mem-roundtrip / eval
regression)**, **Dream-lite review scan (`dream run --review`, heuristic + no
LLM, applies nothing)**, **effective-context compile + repo ingest + opt-in PPR**.

Still open: graphify bridge / auto-anchor, multi-relation `obs judge`, eval CI
scorecard promotion, graph default-on after multi-hop gate. Details: `docs/ROADMAP.md`.
