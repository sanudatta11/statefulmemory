# memlayer Improvement Roadmap

> Forward-looking strategic plan. The *what is* lives in `PRD.md`; this is
> the *what's next*. See also [RETRIEVAL_ROADMAP.md](RETRIEVAL_ROADMAP.md)
> for the promotion plan covering `memlayer-embed`, `memlayer-extract`, and
> `memlayer-eval`.

## 1. Where memlayer is today

After the **agent-integration-depth** spec landed (PreToolUse hooks for
Grep/Read, progressive-disclosure SKILL.md, fail-silent JSONL audit log,
cross-project global mirror DB), the platform looks like this:

| Layer | State | Notes |
|---|---|---|
| Per-project storage | Production | SQLite + FTS5, V3 schema with `superseded_by_id`. |
| Cross-project mirror | Production | `~/.memlayer/global.sqlite` mirrors saves; powers `--all-projects`. |
| Retrieval | BM25 only | FTS5 against the project DB. No embeddings live in production. |
| Supersession | Synchronous BM25 | V3 — top BM25 hit in same `type+scope` is soft-deleted on save. |
| Audit log | Production | `~/.memlayer/queries.log`, JSONL, fail-silent, opt-in full mode. |
| Agent integration | 4 hooks + skill | SessionStart, Stop, PreToolUse[Grep], PreToolUse[Read]. |
| MCP server | **Not built** | Roadmap item; spec not written. |
| Hybrid retrieval | **Eval-only** | `memlayer-embed`, `memlayer-extract`, `memlayer-eval` all scaffolded. Daemon does not use them. |
| LLM judge | **Deferred** | Engram-style relation classifier; no code yet. |
| Code AST / graph | **None** | Zero AST extraction, zero graph tables, zero tree-sitter. |
| TUI / doctor | **Spec written, not built** | `tui-doctor-setup` spec exists. |

The agent-side surface is sound. The *retrieval substrate* is where the
gap to the published research benchmarks lives.

## 2. Code AST extraction & token reduction

A common question: "how well will memlayer reduce token usage on agentic
coding tasks via code-structure understanding?"

### 2.1 Current capability: 0%

memlayer has no AST extractors, no `code_nodes` / `code_edges` tables, no
tree-sitter, no graph traversal. The substrate (SQLite + FTS5) *could*
host such a layer, but nothing in the daemon, storage, or any spec scopes
it.

### 2.2 What graphify does (for comparison)

[graphify](https://github.com/...) is a separate tool with deep code
understanding:

| Capability | Graphify | memlayer |
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

- **memlayer's lane** — *why*: decisions, patterns, fixes, feedback,
  conventions. Free-text observations with topic-key supersession.
- **graphify's lane** — *what*: call graph, imports, communities, blast
  radius. Structural code understanding.

These are complementary, not competing. The token-reduction story for an
agent is **strongest when both run together**:

| Scenario | Tokens dumped to LLM | Notes |
|---|---|---|
| Agent without either tool | ~30-50k | Greps return raw file content. |
| Agent with memlayer only | ~15-25k | Decisions surface but the agent still greps full files for code context. |
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
memlayer ctx <file>::<symbol>
```

Behavior:

1. Look up observations whose `code_anchor` matches `<file>::<symbol>` or
   whose ancestor symbol does (textual prefix match — no graph required
   in memlayer).
2. If `graphify` is installed (`which graphify`), shell out to it for the
   BFS subgraph around `<symbol>` and merge.
3. Output a single markdown block: prior decisions + structural
   neighborhood.

**Cost:** ~500 LOC (one column, one CLI verb, one optional shell-out).
No dependency on tree-sitter in the memlayer crates.

**Alternative considered (rejected):** build AST in memlayer. Would
duplicate ~30k LOC of graphify and divert the team from the higher-ROI
promotions below.

### 2.5 Honest token-reduction estimate for memlayer alone

Without AST, only the *why* axis is covered. Real-session token reduction
for memlayer alone:

| Task class | Reduction vs. no memlayer |
|---|---|
| "What did we decide about X?" | ~5-10× (single observation vs. re-derivation) |
| "Add a feature using existing patterns" | ~2-3× (pattern observations surface relevant prior choices) |
| "Why is this code shaped this way?" | ~3-5× when a fix/decision exists; 1× otherwise |
| "Refactor across a module" | ~1× — memlayer can't help; graphify can. |
| "Greenfield code with no prior history" | ~1× — empty briefing |

Aggregate across mixed tasks: **~2-3× average token reduction**, which is
real but not eye-popping. With graphify integration, the same workload
becomes 5-10×.

## 3. Improvement roadmap

Five proposed specs, sequenced. Each is independent enough to ship in its
own PR.

### Spec 1 — Promote hybrid retrieval to daemon  *(highest ROI)*

**Status today:** `memlayer-embed` (BGE-small, candle-rs, 384-dim),
`memlayer-extract` (Haiku-extracted facts), `memlayer-eval`
(LoCoMo/LongMemEval/BEAM scaffolds with BM25 + dense + RRF + Haiku
rerank) all working in eval. Daemon doesn't use any of them.

**Scope:**

1. Wire `BgeSmallEmbedder` into the daemon's `save_observation` path —
   embed title+content, store in a new `observation_vectors` table (or
   sqlite-vec virtual table).
2. Add `--mode hybrid` to `obs search` / `obs context` (default still
   BM25 for back-compat).
3. RRF fusion of BM25 + dense top-30 (already implemented in
   `memlayer-eval/retrieve_hybrid.rs`).
4. Optional `--rerank` Haiku LLM rerank (already implemented in
   `memlayer-eval/rerank.rs`).
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

**Status today:** roadmap item, no spec.

**Scope:**

1. New crate `memlayer-mcp` exposing stdio JSON-RPC 2.0.
2. Tools: `memory/search`, `memory/save`, `memory/context`,
   `memory/recent`, `memory/get`, `memory/delete`.
3. Resources: `memlayer://briefing`, `memlayer://recent`,
   `memlayer://stats`.
4. Optional Streamable HTTP via axum + tower for shared team servers.
5. Bearer-token auth on TCP, none on stdio (filesystem-trusted).
6. `memlayer install --mcp` adds an `mcp` server entry to
   `~/.claude/mcp.json` and Cursor's `mcp_servers.json`.

**Why second:** unblocks MCP-native agents (Cursor, Claude Code) to use
memlayer as first-class tools instead of shell-out hooks. The hook
approach we just shipped works, but MCP is the cleaner future. Doesn't
conflict with the hooks — hooks remain for non-MCP fallback.

**Estimated lift:** doesn't change LoCoMo, but ~3× adoption ceiling
because Cursor / Windsurf MCP support is the standard integration path
now.

### Spec 3 — Code anchors + graphify bridge  *(answers the AST question)*

**Status today:** unscoped.

**Scope:**

1. Migration V5: add `code_anchor TEXT` to `observations`; index for
   prefix matching.
2. CLI: `memlayer obs save --anchor "src/auth/middleware.rs::validate_token"`.
3. Auto-anchor heuristic: when invoked from a Claude Code Edit/Write
   hook, infer anchor from `$CLAUDE_TOOL_INPUT_file_path` + nearest
   enclosing fn name (parsed by ripgrep + simple regex, not tree-sitter).
4. New CLI verb: `memlayer ctx <file>::<symbol>` returns prior
   observations matching that anchor + (if graphify on PATH) shells out
   to `graphify query "<symbol>"` and merges.
5. README section explaining the pairing.

**Why third:** answers the AST question directly. ~500 LOC vs. 36k for a
from-scratch AST layer. Doesn't depend on graphify being installed —
graceful degradation.

**Estimated lift:** 5-10× token reduction on tasks where graphify is
co-installed, vs. ~2-3× memlayer-alone today.

### Spec 4 — LLM judge for relation classification  *(deferred Part C)*

**Status today:** scoped in prior plan as "Part C — Judge upgrade
(DEFERRED)". Not implemented.

**Scope:** Engram-style locked-vocabulary classifier
(`conflicts_with | supersedes | scoped | related | compatible | not_conflict`),
new `observation_relations` table, opt-in `memlayer obs judge` verb. New
table is a strict superset of the current `superseded_by_id` FK — that
column becomes a denormalized cache of relations where
`relation = supersedes`.

**Why fourth:** lifts multi-hop and adversarial categories on LoCoMo.
Smaller absolute lift than Spec 1, and only worth doing once Spec 1 is in
place (so the judge has good candidates to classify).

**Estimated lift:** +2-3 pts on LoCoMo on top of Spec 1, mostly
multi-hop.

### Spec 5 — `memlayer obs history` + minor polish

**Status today:** `obs history` was scoped in a prior plan, only the
skill-text portion shipped. The actual command is unimplemented.

**Scope:** small CLI verb walking the `superseded_by_id` chain. ~30 LOC.
Plus:

- Auto-anchor heuristic refinement (from Spec 3).
- Quantization promotion (the `quantize` int8 module from `memlayer-embed`
  activated in production for 10M+ scale).

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

memlayer's published targets in the `retrieval-upgrade-v1` spec are
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
- `docs/RETRIEVAL_ROADMAP.md` — promotion plan for `memlayer-embed`,
  `memlayer-extract`, `memlayer-eval`
- `.catalyst/specs/retrieval-upgrade-v1/spec.md` — eval-side hybrid
  retrieval design (already approved, partially implemented)
- `.catalyst/specs/agent-integration-depth/spec.md` — the
  just-completed agent-integration work
