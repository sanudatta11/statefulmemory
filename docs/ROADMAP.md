# statefulmemory Improvement Roadmap

> Forward-looking strategic plan. The *what is* lives in `PRD.md`; this is
> the *what's next*. See also [RETRIEVAL_ROADMAP.md](RETRIEVAL_ROADMAP.md)
> for the promotion plan covering `statefulmemory-embed`, `statefulmemory-extract`, and
> `statefulmemory-eval`.

## 1. Where statefulmemory is today

After the **agent-integration-depth** spec landed (PreToolUse hooks for
Grep/Read, progressive-disclosure SKILL.md, fail-silent JSONL audit log,
cross-project global mirror DB), the platform looks like this:

| Layer | State | Notes |
|---|---|---|
| Per-project storage | Production | SQLite + FTS5, V7 schema with `code_anchor` and `superseded_by_id`. |
| Cross-project mirror | Production | `~/.statefulmemory/global.sqlite` mirrors saves; powers `--all-projects`. |
| Retrieval | BM25 + Hybrid | BM25 default + BGE-small dense vector ANN top-30 fusion (RRF). |
| Supersession | Synchronous BM25 | V3 — top BM25 hit in same `type+scope` is soft-deleted on save. |
| Audit log | Production | `~/.statefulmemory/queries.log`, JSONL, fail-silent, opt-in full mode. |
| Agent integration | 4 hooks + skill | SessionStart, Stop, PreToolUse[Grep], PreToolUse[Read]. |
| MCP server | **Shipped** | `statefulmemory mcp` + seven `memory_*` tools; `statefulmemory install` registers Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code, ZCode, VS Code, Copilot CLI, Gemini CLI, Codex, Amazon Q, `.agents`. |
| Deployment | **Self-host shipped; Cloud SaaS offering** | Local UDS + team TCP+TLS self-host today; StatefulMemory Cloud managed SaaS is a stated product path (signup on statefulmemory.dev as it rolls out). |
| CI Scorecard & Eval | **Shipped** | `statefulmemory eval [--smoke] [--save-scorecard <file>]` and `.github/workflows/eval.yml`. |
| Code Anchors | **Shipped** | V7 `code_anchor` schema, CLI `--anchor` & Graphify call-graph bridge. |
| TUI & Doctor | **Shipped** | `statefulmemory tui` observation browser and `statefulmemory doctor [--repair]` auto-repair. |
| LLM judge | **Shipped** | Relation classifier (`observation_relations`). |
| Laya System-1 | **Shipped** | Opt-in Wave 4 sidecar for router / decide / conflict; `smem install` sets up venv + assets + local spawn (`--no-laya` to skip). Fine-tune export deferred. |

The agent-side surface is sound. The *retrieval substrate* is where the
gap to the published research benchmarks lives.

## 2. Code AST extraction & token reduction

A common question: "how well will statefulmemory reduce token usage on agentic
coding tasks via code-structure understanding?"

### 2.1 Current capability: 0%

statefulmemory has no AST extractors, no `code_nodes` / `code_edges` tables, no
tree-sitter, no graph traversal. The substrate (SQLite + FTS5) *could*
host such a layer, but nothing in the daemon, storage, or any spec scopes
it.

### 2.2 What graphify does (for comparison)

[graphify](https://github.com/...) is a separate tool with deep code
understanding:

| Capability | Graphify | statefulmemory |
|---|---|---|
| AST extraction | Tree-sitter, ~36 languages, 12.8k LOC `extract.py` | ✗ |
| Cross-file call resolution | `symbol_resolution.py` (~150 LOC) | ✗ |
| Graph build | NetworkX DiGraph, ghost-node merge | ✗ |
| Clustering | Leiden + Louvain fallback | ✗ |
| God-node / surprising-connection analytics | `analyze.py` | ✗ |
| Token-reduction benchmark | `benchmark.py`: BFS-subgraph vs. full-corpus tokens | ✗ |
| MCP query tools | `query_graph`, `get_node`, `get_neighbors`, `god_nodes` | ✗ |

graphify is ~36k Python LOC. Reimplementing in Rust + tree-sitter to
feature parity is realistically 6-9 months of dedicated work.

### 2.3 The strategic answer — integrate, don't reimplement

- **statefulmemory's lane** — *why*: decisions, patterns, fixes, feedback,
  conventions. Free-text observations with topic-key supersession.
- **graphify's lane** — *what*: call graph, imports, communities, blast
  radius. Structural code understanding.

These are complementary, not competing. The token-reduction story for an
agent is **strongest when both run together**:

| Scenario | Tokens dumped to LLM | Notes |
|---|---|---|
| Agent without either tool | ~30-50k | Greps return raw file content. |
| Agent with statefulmemory only | ~15-25k | Decisions surface but the agent still greps full files for code context. |
| Agent with graphify only | ~3-8k | BFS subgraph for the symbol, no decisions / why. |
| Agent with both | ~3-10k | Subgraph + decisions about that subgraph. **5-10× reduction.** |

### 2.4 Concrete integration design — the "code anchor" approach

Add an optional `code_anchor` field to observations:

```sql
ALTER TABLE observations
  ADD COLUMN code_anchor TEXT;  -- "<file>::<symbol>::<line>" or null
```

Plus a thin bridge command:

```bash
statefulmemory ctx <file>::<symbol>
```

Behavior:

1. Look up observations whose `code_anchor` matches `<file>::<symbol>` or
   whose ancestor symbol does (textual prefix match — no graph required
   in statefulmemory).
2. If `graphify` is installed (`which graphify`), shell out to it for the
   BFS subgraph around `<symbol>` and merge.
3. Output a single markdown block: prior decisions + structural
   neighborhood.

**Cost:** ~500 LOC (one column, one CLI verb, one optional shell-out).
No dependency on tree-sitter in the statefulmemory crates.

**Alternative considered (rejected):** build AST in statefulmemory. Would
duplicate ~30k LOC of graphify and divert the team from the higher-ROI
promotions below.

### 2.5 Honest token-reduction estimate for statefulmemory alone

Without AST, only the *why* axis is covered. Real-session token reduction
for statefulmemory alone:

| Task class | Reduction vs. no statefulmemory |
|---|---|
| "What did we decide about X?" | ~5-10× (single observation vs. re-derivation) |
| "Add a feature using existing patterns" | ~2-3× (pattern observations surface relevant prior choices) |
| "Why is this code shaped this way?" | ~3-5× when a fix/decision exists; 1× otherwise |
| "Refactor across a module" | ~1× — statefulmemory can't help; graphify can. |
| "Greenfield code with no prior history" | ~1× — empty briefing |

Aggregate across mixed tasks: **~2-3× average token reduction**, which is
real but not eye-popping. With graphify integration, the same workload
becomes 5-10×.

## 3. Improvement roadmap

Five proposed specs, sequenced. Each is independent enough to ship in its
own PR.

### Spec 1 — Promote hybrid retrieval to daemon  *(highest ROI)*

**Status:** ✅ **Implemented in `.catalyst/specs/retrieval-promotion/`**
(merged 2026-06-18, 14 tasks, 70 unit tests). Section retained as
historical context for the design rationale.

**What landed:** `BgeSmallEmbedder` async-wired into the daemon save path,
V4 migration adds `observations_vec` (vec0 vtable) + `observation_embedding_meta`,
`obs search --mode hybrid` and `obs context --mode hybrid` route through
BM25 + dense + RRF, optional `--rerank fast|capable` with 5s timeout +
graceful fallback, V5 migration adds atomic-fact storage + `obs facts <id>`
verb, config-gated extract worker (Haiku/Sonnet) writes facts off the hot
path, new `statefulmemory config show/get/set` CLI for managing the
`~/.statefulmemory/config.toml` and per-project overlays, audit log records
`embed_queued`/`extract_queued`/`extract_model` per save (SC-12).

**What's deferred to a follow-up spec:** quantization (int8 — `quantize`
module stubbed); cross-project hybrid (global DB stays BM25 — daemon-side
hybrid is per-project only); bulk re-extraction across historical
observations (`statefulmemory obs reextract` is a stub today); `statefulmemory-eval`
CI scorecard wiring (Section 3 of `RETRIEVAL_ROADMAP.md`).

**Original scope (for reference):**

1. Wire `BgeSmallEmbedder` into the daemon's `save_observation` path —
   embed title+content, store in a new `observation_vectors` table (or
   sqlite-vec virtual table).
2. Add `--mode hybrid` to `obs search` / `obs context` (default still
   BM25 for back-compat).
3. RRF fusion of BM25 + dense top-30 (already implemented in
   `statefulmemory-eval/retrieve_hybrid.rs`).
4. Optional `--rerank` Haiku LLM rerank (already implemented in
   `statefulmemory-eval/rerank.rs`).
5. Migration: V4 adds the vector table; backfill existing observations on
   first daemon startup post-upgrade.

**Why first:** the eval crates are scaffolded and validated; "promote to
daemon" is mostly plumbing. Largest LoCoMo / LongMemEval jump per LOC.

**Estimated lift:** +10-15 pts on LoCoMo (55-62% → 70-77%), driven by the
open-domain / paraphrased categories that BM25 can't cover.

**Out of scope:** quantization (int8 — `quantize` module stubbed for
spec-task-24, defer); cross-project hybrid (global DB stays BM25 in this
spec).

See [RETRIEVAL_ROADMAP.md](RETRIEVAL_ROADMAP.md) §6.1 for full design.

### Spec 2 — MCP server  *(largest reach jump)*

**Status today:** shipped (stdio server + install-time multi-agent registration).

**Scope (delivered):**

1. Crate `statefulmemory-mcp` exposing stdio MCP via `rmcp`.
2. Tools: `memory_search`, `memory_add`, `memory_context`, `memory_recent`,
   `memory_facts`, `memory_health`.
3. `statefulmemory mcp` CLI subcommand (best-effort daemon autospawn).
4. `statefulmemory install` registers the stdio server with Claude Code, Cursor,
   Windsurf, Antigravity, OpenCode, Kimi Code, ZCode, VS Code / Copilot,
   Copilot CLI, Gemini CLI, Codex, Amazon Q, and the shared `.agents`
   convention (no separate `--mcp` flag).

**Deferred / follow-ups:** Streamable HTTP for shared team servers; bearer
auth on TCP; resources (`statefulmemory://briefing`, …); live integration tests.

**Why it matters:** unblocks MCP-native agents to use statefulmemory as first-class
tools instead of shell-out hooks. Hooks remain for non-MCP fallback.

### Spec 3 — Code anchors + graphify bridge  *(answers the AST question)*

**Status today:** partial. Anchors + verify shipped (`observation_anchors`,
`verify_state`, `statefulmemory verify`, install git hooks, stale withdrawal from
context). The graphify / AST bridge did **not** ship.

**Shipped:**

1. Migrations V9/V10: multi-anchor table + `verify_state` /
   `verified_commit` / `verified_at` (legacy `code_anchor` column remains
   for prefix search).
2. CLI: `statefulmemory obs save --anchor …` (repeatable), `statefulmemory verify`,
   `obs context --include-stale`.
3. Install writes post-commit / post-merge / post-checkout hooks that
   background `statefulmemory verify --quiet`.

**Still open:**

1. Auto-anchor heuristic from Edit/Write hooks.
2. `statefulmemory ctx <file>::<symbol>` + optional `graphify query` merge.
3. README section on the graphify pairing.

**Why third:** answers the AST question directly. ~500 LOC vs. 36k for a
from-scratch AST layer. Doesn't depend on graphify being installed —
graceful degradation.

**Estimated lift:** 5-10× token reduction on tasks where graphify is
co-installed, vs. ~2-3× statefulmemory-alone today.

### Spec 4 — LLM judge for relation classification  *(deferred Part C)*

**Status today:** supersession judge shipped (`conflict.enabled` +
`ConflictsWith` / resolve worker). Locked-vocabulary multi-relation
classifier and `obs judge` verb remain deferred.

**Scope (remaining):** locked-vocabulary relation classifier
(`conflicts_with | supersedes | scoped | related | compatible | not_conflict`),
opt-in `statefulmemory obs judge` verb. `observation_relations` already exists;
`superseded_by_id` stays a denormalized cache of relations where
`relation = supersedes`.

**Why fourth:** lifts multi-hop and adversarial categories on LoCoMo.
Smaller absolute lift than Spec 1, and only worth doing once Spec 1 is in
place (so the judge has good candidates to classify).

**Estimated lift:** +2-3 pts on LoCoMo on top of Spec 1, mostly
multi-hop.

### Spec 5 — `statefulmemory obs history` + minor polish

**Status today:** shipped (`obs history` walks `superseded_by_id`; int8
quantize via `embed.quantize` + `statefulmemory reindex`).

**Remaining polish:**

- Auto-anchor heuristic refinement (from Spec 3).
- Graphify bridge (`statefulmemory ctx` + optional `graphify query`).

**Why last:** small, polish-tier. Nice to ship together once the big
specs are in.

## 4. Estimated LoCoMo trajectory

| Milestone | LoCoMo | LongMemEval | Production tokens (mixed agentic) |
|---|---|---|---|
| Today (post agent-integration-depth) | 55-62% | 60-65% | ~2-3× reduction |
| + Spec 1 (hybrid retrieval) | 70-77% | 80-85% | ~2-3× (no change — retrieval quality not main token driver) |
| + Spec 4 (LLM judge) | 72-80% | 82-87% | ~2-3× |
| + Spec 3 (code anchors + graphify) | 72-80% (no change) | 82-87% (no change) | **5-10×** |
| + Spec 2 (MCP) | 72-80% (no change) | 82-87% (no change) | 5-10× — adoption ceiling lifts |

statefulmemory's published targets in the `retrieval-upgrade-v1` spec are
LoCoMo 91.6% / LongMemEval 94.8% — these reflect the *tuned* eval-harness
numbers with full re-rank. The trajectory above is for the *production
daemon* path which doesn't reach the eval ceiling because:

- Production must answer in <50ms p95 (eval can take seconds).
- Production cannot rerank with Haiku synchronously by default.
- Production cannot fact-extract every observation eagerly.

The gap between 77% (production) and 91.6% (eval) is acceptable — it's
the cost of being a daemon, not a benchmark harness.

## 5. References

- `docs/PRD.md` — current product scope and non-goals
- `docs/RETRIEVAL_ROADMAP.md` — promotion plan for `statefulmemory-embed`,
  `statefulmemory-extract`, `statefulmemory-eval`
- `.catalyst/specs/retrieval-upgrade-v1/spec.md` — eval-side hybrid
  retrieval design (already approved, partially implemented)
- `.catalyst/specs/agent-integration-depth/spec.md` — the
  just-completed agent-integration work
