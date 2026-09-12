# Implementation handover: code-anchored, verifiable memory

Status: ready to implement. Written for an implementing agent with no prior
context on this branch.

Branch: `cursor/mem-archive-decide-locomo-plan-8379` (PR #6). Land each
workstream as its own commit, in the order given.

---

## 0. Why this work exists

memlayer already stores observations, supersedes stale ones deterministically,
and retrieves them locally with no network call. Published conversational
memory systems are add-only: they never remove a superseded fact and rely on
the answering model to adjudicate at read time. The 2026 literature measured
that failure mode on real software history — a retrieval-only pipeline serves
the *superseded* value 36–38% of the time, and an LLM reranker does not fix it,
while deterministic supersession drives it to ~0.

memlayer's supersession path already does the right thing (superseded rows are
soft-deleted, so they never reach retrieval), but three things are missing:

1. We never **measure** it, so the advantage is invisible.
2. We never **say** which value won, so the agent cannot tell that adjudication
   happened.
3. `code_anchor` is a stored string that nothing ever **verifies** against the
   repository, so a memory cannot know when the code moved underneath it.

This spec closes all three, plus the metric and retrieval gaps that cap our
benchmark ceiling.

### Naming constraint (hard requirement)

Do **not** name third-party memory products in code, comments, commit
messages, docs, CLI help, or test names. Refer to "published conversational
memory systems", "vendor LLM-judge leaderboards", or "add-only memory
designs". This applies to every file you touch.

---

## 1. Invariants you must not break

Read `CLAUDE.md` first. These are the ones this spec stresses:

- **All DB mutations go through the per-project write thread.** Never write
  from an RPC handler or a worker. Use an existing `WriteRequest` variant, or
  `WriteRequest::Custom` for one-off operations
  (`crates/memlayer-storage/src/write.rs:36`).
- **Save stays off the LLM hot path.** Worker pools use `try_queue` and drop on
  full; they never block a save. Follow `embed_worker.rs` / `extract_worker.rs`
  / `resolve_worker.rs`.
- **Retrieval must stay local and network-free** unless the caller explicitly
  asks for rerank. Do not add an LLM call to the default read path.
- **Migrations are idempotent** and embedded at compile time via
  `refinery::embed_migrations!`. Use `IF NOT EXISTS` and
  `ALTER TABLE ... ADD COLUMN`. Current head is `migrations/V8__observation_relations.sql`,
  so new files start at `V9`.
- **Config re-resolves per task.** Never cache `MemlayerConfig` at daemon
  startup. New flags need a code default, a TOML key, and a
  `MEMLAYER_*` env override, matching `crates/memlayer-core/src/config.rs`.
- **No new build-time-downloading dependencies.** The workspace deliberately
  avoids crates that fetch at build time (see the candle comment in
  `Cargo.toml`). For git, shell out to the `git` binary — do not add `git2` or
  `gix`.

Test commands (per-crate, avoids full rebuilds):

```bash
cargo build --tests --workspace
cargo test -p memlayer-eval --lib
cargo test -p memlayer-storage --lib
cargo test -p memlayer-daemon --lib
cargo test -p memlayer-cli --lib
cargo test -p memlayer-retrieval --lib
cargo test -p memlayer-mcp --lib
cargo clippy --workspace -- -D warnings
python3 tools/compare_locomo.py --self-check
```

Integration tests in `crates/memlayer-tests/` need a live daemon socket and
fail in sandboxed environments. Do not treat those failures as your
regression.

---

## 2. Workstream 1 — Make the eval numbers real

**Problem.** `crates/memlayer-eval/src/scorecard.rs:34-41` sets
`precision = recall = accuracy/100` and derives `f1` from that, so `f1_score`
is an alias of accuracy and carries no information. There is no per-category
breakdown (LoCoMo category is passed to the judge via `judge_context` at
`datasets/locomo.rs:119` but never aggregated), no recall@k, no MRR, and no
token totals. You cannot diagnose a regression with this.

### 1.1 Add a retrieval-side gold rank

In `crates/memlayer-eval/src/runner.rs`, replace `gold_in_hits`
(line 502) with a rank-returning function and derive the boolean from it:

```rust
/// 0-based rank of the first hit containing the gold answer, if any.
fn gold_rank(gold: &str, hits: &[String]) -> Option<usize> {
    let g = gold.trim().to_ascii_lowercase();
    if g.is_empty() {
        return None;
    }
    hits.iter().position(|h| h.to_ascii_lowercase().contains(&g))
}

fn gold_in_hits(gold: &str, hits: &[String]) -> bool {
    gold_rank(gold, hits).is_some()
}
```

Compute `gold_rank` on **every** run, not just the lexical-judge path. It is a
retrieval measurement independent of the judge, which is exactly what we lack
today.

### 1.2 Carry category through the dataset layer

`crates/memlayer-eval/src/datasets/mod.rs` — add to `EvalQuery`:

```rust
    /// Benchmark-provided category label (e.g. LoCoMo single-hop / multi-hop /
    /// temporal / open-domain / adversarial). Used for per-category reporting.
    #[serde(default)]
    pub category: Option<String>,
```

Populate it in `datasets/locomo.rs` from the existing `LoCoMoQA::category`
field. Keep `judge_context` behaviour unchanged. Set `category: None` in
`longmemeval.rs`, `beam.rs`, and the two smoke fixtures in
`crates/memlayer-cli/src/cmd_eval.rs` (`smoke_memories` / `smoke_queries`).

### 1.3 Extend `QueryResult` and `RunReport`

`runner.rs` — add to `QueryResult`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// 0-based rank of the first hit containing the gold answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_rank: Option<usize>,
    pub hits_count: usize,
```

Add to `RunReport`:

```rust
    /// Per-category totals, sorted by category name.
    #[serde(default)]
    pub by_category: std::collections::BTreeMap<String, CategoryStats>,
    /// Fraction of queries whose gold answer appeared anywhere in the
    /// retrieved set. This is a retrieval metric and is independent of the
    /// judge verdict.
    pub recall_at_k: f64,
    /// Mean reciprocal rank of the gold answer within the retrieved set.
    pub mrr: f64,
    pub total_prompt_tokens: usize,
```

with

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CategoryStats {
    pub total: usize,
    pub correct: usize,
    pub accuracy_pct: f64,
}
```

Aggregate after the query loop. `recall_at_k = count(gold_rank.is_some()) / total`.
`mrr = mean(1/(rank+1))`, counting misses as 0.

Extend `RunReport::to_markdown` with a per-category table and the three new
summary rows.

### 1.4 Fix the scorecard

`crates/memlayer-eval/src/scorecard.rs`:

- Bump `scorecard_version` to `"2.0"`.
- Keep `accuracy_pct` as-is (judge or lexical pass rate).
- **Remove the fake aliases.** Replace `precision` / `recall` / `f1_score`
  with:

```rust
    /// Retrieval recall@k: gold answer present anywhere in the retrieved set.
    pub recall_at_k: f64,
    pub mrr: f64,
    /// Judge/lexical pass rate per category.
    pub by_category: BTreeMap<String, CategoryStats>,
    pub total_prompt_tokens: usize,
    pub mean_prompt_tokens: f64,
```

  Do not emit a field named `f1_score` at all. We do not compute token-overlap
  F1, and a field with that name invites an invalid comparison against
  published token-F1 results. If you want a placeholder, use
  `token_f1: Option<f64>` set to `None` with a doc comment explaining it is
  not implemented.
- Update `render_text` to print recall@k, MRR, and the per-category table.
- Update the round-trip test `scorecard_from_report_and_save_round_trip` for
  the new shape, and add a test asserting `by_category` survives serialization.

### 1.5 Update the comparison tooling

`tools/compare_locomo.py` currently reads `f1_score`. Make it:

- tolerate a missing / null `f1_score` and `token_f1`,
- print `recall_at_k`, `mrr`, and the per-category table when present,
- keep the existing protocol warning logic and the `--self-check` fixture
  (extend the fixtures to the v2 shape so `--self-check` still passes).

Also update `crates/memlayer-eval/baselines/locomo.json`: the
`memlayer_scorecard` block should describe `recall_at_k` and state that
token-F1 is not emitted.

### Acceptance criteria

- `cargo test -p memlayer-eval --lib` passes.
- `make eval-locomo-smoke` produces a scorecard containing `recall_at_k`,
  `mrr`, and `by_category`, and `tools/compare_locomo.py` renders it.
- No field named `f1_score` holds a value derived from accuracy.

**Commit:** `feat(eval): report real recall@k, MRR, and per-category accuracy`

---

## 3. Workstream 2 — Say which value wins

**Problem.** Supersession is silent. `read.rs` soft-deletes the superseded row,
so search correctly stops serving the stale value, but the wire `Observation`
message has no supersession field and the CLI never tells the agent that an
adjudication happened. The published stale-context work found that the only
mitigation that held across every model was *delivering the in-force decision
into context and stating which value wins*; a summary that merely asserts a
value is the barrier, because the agent never goes looking for a newer one.

### 2.1 Proto

`proto/memlayer.proto`, `message Observation` (field 20 is currently last):

```proto
  // Set when this observation replaced an earlier one. Lets a client render
  // "current value, supersedes #N" so the agent knows adjudication happened.
  repeated int64 supersedes_ids = 21;
  // Number of observations this one has replaced (denormalized counter).
  int32 superseded_count = 22;
```

`superseded_count` already exists as a column (`migrations/V3__conflict.sql`).

### 2.2 Storage

`crates/memlayer-storage/src/read.rs`:

- Add `superseded_count` to `SELECT_COLS` (line 24). **Careful:**
  `history_chain` hard-codes `row.get(18)` for `superseded_by_id` with a
  comment that `SELECT_COLS` has 18 columns. Adding a column shifts that
  index. Fix it by selecting by name instead of position, or update both the
  index and the comment. Missing this silently corrupts the history chain.
- Add a helper that, for a set of observation ids, returns the ids each one
  superseded (query `observations WHERE superseded_by_id IN (...)`), so the
  service can populate `supersedes_ids` for search/context results in one
  round trip rather than N.

`crates/memlayer-storage/src/models.rs`: add `superseded_count: i32` to
`Observation` and to `from_row`.

### 2.3 Service

`crates/memlayer-daemon/src/service.rs` — `obs_to_proto` (around line 384)
gains the two fields. For `search`, `context`, and `recent`, batch-fetch the
superseded ids and fill them in. Keep it to one extra query per RPC.

### 2.4 CLI rendering

`crates/memlayer-cli/src/render.rs`: when `supersedes_ids` is non-empty,
render a line under the observation, e.g.

```
  current — supersedes #12 (use `memlayer obs history 47` for the chain)
```

Text output only; JSON/YAML consumers get the raw fields.

### 2.5 MCP

`crates/memlayer-mcp/src/server.rs`: include `supersedes_ids` in the
`memory_search` and `memory_context` result payloads, and mention in the tool
descriptions that a result carrying `supersedes_ids` is the in-force value and
the superseded values are intentionally withheld. That sentence is the thing
that stops a model from going looking for an older value.

### Acceptance criteria

- Save two conflicting observations; `memlayer obs search` on the survivor
  shows the supersedes line, and the superseded one is absent from results.
- `history_chain` tests still pass (this is the regression risk).

**Commit:** `feat: surface the in-force value and what it superseded`

---

## 4. Workstream 3 — Staleness benchmark (prove the moat early)

Do this before the anchor work so every later change has a number to move.

**Goal.** Measure how often a superseded value is served, with a baseline arm
that has supersession disabled. This is the metric we win on architecture, and
it is the one our actual user feels.

### 3.1 Dataset

New `crates/memlayer-eval/src/datasets/staleness.rs`. Input JSONL at
`data/staleness/transitions.jsonl`, one clean atomic transition per line:

```json
{
  "id": "t001",
  "subject": "auth middleware",
  "predicate": "token header",
  "stale_value": "X-Auth-Token",
  "current_value": "Authorization: Bearer",
  "stale_statement": "The auth middleware reads the X-Auth-Token header.",
  "current_statement": "The auth middleware reads the Authorization: Bearer header.",
  "question": "Which header does the auth middleware read?",
  "gold_answer": "Authorization: Bearer",
  "anchor_path": "src/auth/middleware.rs",
  "topic_key": "auth/token-header"
}
```

Loader contract: emit **two** `EvalMemory` items per transition — the stale
statement first, then the current one, sharing a `topic_key` so the
supersession path fires — and one `EvalQuery`. The stale and current
statements must differ only in the value, with no recency markers in the text,
or the answering model can cheat.

Ship a small committed fixture (10–20 transitions) at
`crates/memlayer-eval/fixtures/staleness-sample.jsonl` so the benchmark runs
with no download. Keep `data/` gitignored for anything larger.

### 3.2 New metrics

Add `BenchmarkKind::Staleness` in `runner.rs` (and to the clap `ValueEnum`,
the `dataset_project_prefix` match, `default_profile` in `config.rs`, and
`cmd_eval.rs`'s benchmark string match — the compiler will find these).

`EvalQuery` needs the stale value for scoring. Rather than widen `EvalQuery`
for one benchmark, add:

```rust
    /// Value that must NOT be served (superseded). Staleness benchmark only.
    #[serde(default)]
    pub anti_answer: Option<String>,
```

Report, in `RunReport`:

```rust
    /// Fraction of queries where the superseded value was served — either in
    /// the retrieved set or in the model answer. Lower is better.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_served_pct: Option<f64>,
```

Scoring per query:

- `stale_served` = `anti_answer` appears in the retrieved hits, or (in LLM mode)
  in `model_answer`, while `gold_answer` does not.
- `correct` keeps its existing meaning.

### 3.3 Baseline arm

To produce the contrast, the harness needs a mode where supersession is off.
Add to `RunConfig`:

```rust
    /// When true, ingest without supersession so both the stale and current
    /// statements remain retrievable. Baseline arm for the staleness
    /// benchmark; mirrors an add-only memory design.
    pub no_supersede: bool,
```

Wire it in `crates/memlayer-eval/src/ingest.rs` (skip the conflict/supersede
step) and expose it as `memlayer eval --no-supersede`. Both arms must be
otherwise identical.

### 3.4 Makefile

```make
eval-staleness: release
	@mkdir -p "$(EVAL_OUT)"
	MEMLAYER_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --benchmark staleness \
	  --save-scorecard "$(EVAL_OUT)/staleness.json"
	MEMLAYER_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --benchmark staleness \
	  --no-supersede --save-scorecard "$(EVAL_OUT)/staleness-baseline.json"
	@$(COMPARE_STALENESS) --scorecard "$(EVAL_OUT)/staleness.json" \
	  --baseline "$(EVAL_OUT)/staleness-baseline.json"
```

Add `tools/compare_staleness.py` printing both arms side by side with the
delta on `superseded_served_pct`, plus a `--self-check` fixture mode like
`compare_locomo.py`. Register the new targets in `.PHONY` and `make help`, and
document them in `crates/memlayer-eval/README.md` and
`website/src/content/docs/docs/locomo.md` (rename that page to cover both
benchmarks, or add a sibling page and link it from `index.mdx`).

### Acceptance criteria

- `make eval-staleness` runs on the committed fixture with no network.
- The baseline arm shows a materially higher `superseded_served_pct` than the
  default arm. Report whatever you measure; do not hard-code an expected
  number anywhere.

**Commit:** `feat(eval): staleness benchmark measuring superseded-value serving`

---

## 5. Workstream 4 — Anchor verification (the moat)

**Problem.** `migrations/V7__code_anchor.sql` adds `observations.code_anchor`
and an index. `read.rs:689` has `search_by_anchor`. Nothing ever checks an
anchor against the repository. A memory cannot know that the code moved.

**Design principles** (from the artifact-anchored verification literature):

- Freshness is an axis **independent** of confidence.
- A stale claim is **withdrawn**, not returned for the agent to weigh.
- When the content needed to re-check is gone, record `unprovable` rather than
  guessing.
- Untouched paths must re-verify **for free** — that property is what makes
  verification cheap enough to run on every commit.

### 4.1 Schema

`migrations/V9__observation_anchors.sql`:

```sql
-- V9__observation_anchors.sql — multi-anchor table binding observations to
-- repository artifacts, with the commit and content digest used at write time.
CREATE TABLE IF NOT EXISTS observation_anchors (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    observation_id  INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
    path            TEXT NOT NULL,          -- repo-relative, forward slashes
    symbol          TEXT,                   -- optional symbol name
    line_start      INTEGER,
    line_end        INTEGER,
    anchor_commit   TEXT,                   -- 40-hex sha at write time
    content_digest  TEXT,                   -- sha256 of the anchored slice
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(observation_id, path, symbol)
);

CREATE INDEX IF NOT EXISTS idx_obs_anchors_obs  ON observation_anchors(observation_id);
CREATE INDEX IF NOT EXISTS idx_obs_anchors_path ON observation_anchors(path);
```

`migrations/V10__verify_state.sql`:

```sql
-- V10__verify_state.sql — freshness state, tracked independently of confidence.
ALTER TABLE observations ADD COLUMN verify_state TEXT NOT NULL DEFAULT 'unanchored';
ALTER TABLE observations ADD COLUMN verified_commit TEXT;
ALTER TABLE observations ADD COLUMN verified_at TEXT;

CREATE INDEX IF NOT EXISTS idx_observations_verify_state
    ON observations(verify_state);
```

Keep `observations.code_anchor` populated with the first anchor's canonical
string for backward compatibility with `search_by_anchor` and existing tests.

States: `unanchored` (no anchors — the default, and not a failure),
`verified`, `stale`, `invalidated`, `unprovable`.

### 4.2 Git helpers

New `crates/memlayer-core/src/git.rs`, shelling out to `git` (no new
dependency). Every function returns `Result` and must tolerate "not a repo".

```rust
pub fn is_repo(dir: &Path) -> bool;
pub fn head_sha(dir: &Path) -> Result<String>;
/// `git diff --name-only <from>..<to>` — repo-relative paths.
pub fn changed_paths(dir: &Path, from: &str, to: &str) -> Result<Vec<String>>;
/// `git show <commit>:<path>` — None when the path does not exist there.
pub fn file_at_commit(dir: &Path, commit: &str, path: &str) -> Result<Option<String>>;
```

Strip proxy env vars and set a timeout the same way `agent_cli.rs` does for
its shell-outs. Never let a git failure propagate as a save failure.

### 4.3 Anchor model

New `crates/memlayer-storage/src/anchor.rs`:

```rust
pub struct Anchor {
    pub path: String,
    pub symbol: Option<String>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub anchor_commit: Option<String>,
    pub content_digest: Option<String>,
}

pub enum VerifyState { Unanchored, Verified, Stale, Invalidated, Unprovable }
```

- `Anchor::parse(s: &str)` accepts the existing `path::symbol::line` form plus
  `path`, `path::symbol`, and `path:12-40`. Round-trip with `Anchor::to_string`.
- `digest_slice(file_text: &str, anchor: &Anchor) -> String` — sha256 (the
  `sha2` crate is already a workspace dependency) over the anchored lines when
  a line range is known, else over the located symbol window, else the whole
  file. Normalize line endings before hashing so CRLF churn is not a change.
- `locate_symbol(file_text: &str, symbol: &str) -> Option<(u32, u32)>` — a
  deliberately dumb, deterministic line scan: first line containing the symbol
  as a whole word, extended to the end of its indentation block. **No AST
  parsing.** A real code graph is explicitly out of scope (`docs/ROADMAP.md`
  §2.1); do not start one here.
- DB helpers: `insert_anchors`, `anchors_for`, `set_verify_state`, and
  `list_anchored(project_conn, limit)`.

Unit-test `parse`/`to_string` round-trips, `digest_slice` stability across
line-ending changes, and `locate_symbol` on a small Rust and a small Python
fixture string.

### 4.4 Verification engine

New `crates/memlayer-daemon/src/verify.rs`. Pure function, no I/O beyond git,
so it is unit-testable:

```rust
pub struct AnchorVerdict {
    pub observation_id: i64,
    pub state: VerifyState,
    pub reason: &'static str,
}

pub fn verify_anchors(
    repo: &Path,
    head: &str,
    anchors: &[(i64, Anchor)],
) -> Vec<AnchorVerdict>;
```

Algorithm, per anchor:

1. `anchor_commit` is `None` → `Unanchored`.
2. `anchor_commit == head` → `Verified` (no git work).
3. Compute `changed_paths(anchor_commit, head)` **once per distinct
   `anchor_commit`** and cache it for the call. This is the free-re-verify
   property; do not call `git diff` per anchor.
4. `anchor.path` not in the changed set → `Verified`, update `verified_commit`.
5. Path changed: read the file at `head`.
   - File absent → `Unprovable` (terminal; the content needed to re-check is
     gone). Do not downgrade to `Stale`.
   - Symbol anchor whose symbol is no longer present → `Invalidated`.
   - Recomputed digest equals `content_digest` → `Verified` (the file changed
     but the anchored slice did not).
   - Otherwise → `Stale`.

An observation with several anchors takes the worst state, ordered
`Verified < Stale < Invalidated < Unprovable`.

### 4.5 Worker + RPC + CLI

- `crates/memlayer-daemon/src/verify_worker.rs`, copying `resolve_worker.rs`:
  bounded channel (256), `try_queue` drops on full, applies verdicts through
  `WriteRequest::Custom` in one batch.
- Proto: `VerifyAnchors(VerifyAnchorsRequest) returns (VerifyAnchorsResponse)`
  with `project_name`, optional `observation_id`, and a response carrying
  counts per state plus the changed observations. Add
  `optional string verify_state = 23;` to `Observation`.
- CLI `memlayer verify [--project P] [--json]` in a new
  `crates/memlayer-cli/src/cmd_verify.rs`; register `Verify(VerifyArgs)` in
  `cli.rs`'s `Command` enum and dispatch in `main.rs` using the existing
  `open_client` pattern. Print a per-state summary and list stale/invalidated
  titles with their anchors.
- `memlayer obs save --anchor` already exists; extend it to accept repeated
  `--anchor` flags, stamp `anchor_commit` from `head_sha`, and compute
  `content_digest` at save time. Resolve the repo from
  `ProjectConfig.repo_path` (`crates/memlayer-storage/src/registry.rs:49`),
  falling back to the detected cwd. If the directory is not a git repo, save
  normally with `verify_state = 'unanchored'` and emit a warning through the
  existing save-warnings channel — never fail the save.

### 4.6 Retrieval integration (the payoff)

This is what turns verification into a product feature.

- Add config `[verify] serve_stale = false` (code default `false`, TOML key,
  `MEMLAYER_VERIFY_SERVE_STALE` override) in `crates/memlayer-core/src/config.rs`.
- When `serve_stale` is false, `context` (the auto-injected briefing) excludes
  `stale`, `invalidated`, and `unprovable` observations. A withdrawn claim must
  not reach the model.
- `obs search` still returns them, flagged in the rendered output, so a human
  can find and fix them. Add `--include-stale` to override on `context`.
- Never let this filter hide `unanchored` observations. That is the default
  state for everything saved today, and hiding it would empty every result set.

Add a daemon unit test asserting an `invalidated` observation is absent from
`context` and present in `search`.

### Acceptance criteria

- Fresh DB migrates cleanly to V10; re-running migrations is a no-op.
- In a scratch git repo: save an anchored observation, modify the anchored
  lines, commit, run `memlayer verify` → the observation becomes `stale` and
  disappears from `context` while remaining in `search`.
- Touch an unrelated file and commit → the observation stays `verified` and
  `git diff` is called once, not once per anchor.
- Delete the anchored file → `unprovable`, not `stale`.

**Commits (split these):**
1. `feat(storage): anchor table, verify state, and anchor parsing`
2. `feat(daemon): anchor verification engine and worker`
3. `feat: memlayer verify command and stale withdrawal from context`

---

## 6. Workstream 5 — Git-hook invalidation

Makes verification automatic instead of a chore.

- Extend `memlayer install` (`crates/memlayer-cli/src/cmd_skill.rs`) with a
  `--git-hooks` flag, and include it in the default install when the cwd is a
  git repo.
- Write `.git/hooks/post-commit`, `post-merge`, and `post-checkout`. If a hook
  already exists, append a delimited block; follow the idempotent marker
  approach already used by `ensure_hook` for agent settings
  (`cmd_skill.rs:527`). Re-running install must not duplicate the block.
- Hook body must be non-blocking and must never fail the git operation:

```sh
# >>> memlayer >>>
command -v memlayer >/dev/null 2>&1 && \
  (memlayer verify --quiet >/dev/null 2>&1 &)
exit 0
# <<< memlayer <<<
```

- Add `--quiet` to `memlayer verify`. Ensure `chmod 755` on any hook file you
  create.
- `memlayer uninstall` must remove the marker block and leave any surrounding
  user content intact.

### Acceptance criteria

- Install twice → one marker block. Uninstall → block gone, pre-existing hook
  content preserved.
- A commit that touches an anchored file flips that observation to `stale`
  without the user running anything.

**Commit:** `feat(install): git hooks that re-verify anchors after commit`

---

## 7. Workstream 6 — Promote eval-only retrieval into the daemon

**Problem.** Time decay, evidence-window expansion, entity boost, and the 2-hop
entity walk exist only in `crates/memlayer-eval` (`scoring.rs`,
`retrieve.rs:179`, `retrieve_facts.rs`, `entity_walk.rs`). The production
daemon has none of them. We benchmark a system we do not ship.

Land these **config-gated and defaulted off**, so the change is opt-in and no
existing behaviour shifts silently.

### 6.1 Time decay

`crates/memlayer-retrieval/src/rrf.rs` currently returns ordered ids. Add a
scored variant:

```rust
pub fn rrf_fuse_scored(lists: &[Vec<u64>], k_const: u32) -> Vec<(u64, f64)>;
```

Keep `rrf_fuse` as a thin wrapper over it so existing callers and tests are
untouched. In `service.rs::hybrid_search`, when
`search.decay_lambda > 0.0`, multiply each score by
`exp(-lambda * age_days)` using `created_at`, then re-sort. Reuse the constant
and formula from `crates/memlayer-eval/src/scoring.rs:32` so the two agree.

Config: `[search] decay_lambda = 0.0` (off), `MEMLAYER_SEARCH_DECAY_LAMBDA`.

### 6.2 Evidence window

Port `expand_evidence` (`crates/memlayer-eval/src/retrieve.rs:179`) into
`crates/memlayer-storage/src/read.rs` as `neighbors_in_session(conn, obs_id, window)`
and call it from `context` when `[search] evidence_window > 0` (default `0`).
Deduplicate against hits already present and never let expansion push the
result past the caller's `limit`.

### 6.3 Wire the dead rerank flag

`SearchConfig.rerank` (`config.rs:324`) is defined and never read. Either honor
it in `service.rs` as the default when a request omits `rerank`, or delete it.
Do not leave a config key that does nothing.

### 6.4 Entity boost (optional, last)

Production has no entity tables. If you take this on: add
`migrations/V11__entities.sql` mirroring
`crates/memlayer-eval/migrations_eval/V2__entities.sql`, and populate it with a
**zero-LLM** extractor (capitalized tokens, quoted strings, and code-shaped
identifiers) so the default read path stays local and free. Do not make entity
extraction depend on `extract.enabled`. Skip this section entirely rather than
introducing an LLM call into save.

### Acceptance criteria

- All defaults unchanged: with a default config, search results are
  byte-identical to before this workstream.
- Turning on `decay_lambda` and `evidence_window` measurably moves
  `make eval-locomo` (record the before/after in the commit body).

**Commit:** `feat(retrieval): optional time decay and evidence-window expansion`

---

## 8. Workstream 7 — Token budget as a contract

Competing systems sell token savings. We do not measure tokens in the daemon at
all — only in the eval harness.

- Add a cheap deterministic estimator in `crates/memlayer-core`:
  `pub fn estimate_tokens(s: &str) -> usize`. Do **not** pull `tiktoken_rs`
  into the daemon; document that this is an estimate with a stated margin and
  that the eval harness uses the real tokenizer.
- Add `--max-tokens N` to `obs context` and `obs search`. Pack hits in ranked
  order until the next one would exceed the budget; never emit a partial
  observation. Report the actual count in text output and as a
  `tokens_used` field in the response.
- Per-type quotas: add `[search] max_per_type = 0` (0 = unlimited). When set,
  group fused hits by `observations.type` and round-robin across types before
  truncating, so one noisy type cannot crowd out the others. This is the
  useful half of typed-memory designs and we already have the `type` column —
  no schema change needed.
- Surface both in `memory_search` / `memory_context` MCP schemas.

### Acceptance criteria

- `obs context --max-tokens 500` never exceeds the budget and reports a count.
- With `max_per_type = 2` and a result set dominated by one type, other types
  appear in the output.

**Commit:** `feat: context token budget and per-type retrieval quotas`

---

## 9. Documentation pass (fold into the last commit)

- `docs/PRD.md` still claims memlayer has no MCP server and no vectors. Both
  shipped. Fix or delete those lines.
- `docs/ROADMAP.md` says `obs history` is unimplemented; it is implemented
  (`read.rs:591`). Mark Spec 3 as partial (the `code_anchor` column shipped in
  V7; the code graph did not).
- Stale comments to correct while you are in these files:
  `service.rs:1046` calls `get_facts` a stub, `cmd_obs.rs:656` and
  `cli.rs:486` call `obs reextract` a stub, and
  `docs/RETRIEVAL_ROADMAP.md:39` says quantize is stubbed. All three are
  implemented.
- `crates/memlayer-mcp/src/tools/mod.rs:20` says the search default is bm25;
  the runtime default resolves to hybrid from config.
- Update `CLAUDE.md`: new migration head, the `[verify]` and new `[search]`
  config keys with their env overrides, `memlayer verify` in the RPC list, and
  the new make targets.
- Update `website/src/content/docs/docs/` for `memlayer verify`, stale
  withdrawal, `--max-tokens`, and the staleness benchmark. Add any new page to
  the guide list in `index.mdx`.

---

## 10. Order, and what to do if you run short

Dependency order. Each is independently shippable and leaves the tree green:

1. **W1 eval metrics** — no schema, unblocks measurement of everything else.
2. **W2 in-force surfacing** — small, high visibility. Watch the
   `history_chain` column-index trap.
3. **W3 staleness benchmark** — produces the headline number.
4. **W4 anchor verification** — the moat. Largest workstream; split into three
   commits as noted.
5. **W5 git hooks** — depends on W4.
6. **W6 retrieval promotion** — independent; default-off.
7. **W7 token budget** — independent.

If you run short, stop after W4. W1–W4 is a coherent, defensible story on its
own: a memory layer that knows when the code moved underneath it, withdraws
claims it can no longer prove, says which value is in force, and has a
benchmark showing it does not serve superseded values.

Do not enable `extract.enabled` by default as a shortcut to better retrieval
numbers. It puts an LLM call on every save, which contradicts the zero-egress
read-path guarantee that is part of the positioning. If facts coverage needs to
improve, propose a local zero-LLM extractor as separate work.

---

## 11. Risks

| Risk | Mitigation |
|---|---|
| `SELECT_COLS` change breaks `history_chain`'s positional `row.get(18)` | Switch to lookup by name; the existing `history_chain` tests must pass |
| Git shell-out cost on large repos | One `git diff` per distinct anchor commit per run, cached; untouched paths re-verify with no git call |
| `locate_symbol` heuristic misfires | Digest comparison is the real check; symbol location only narrows the slice. Prefer explicit line ranges |
| Stale filtering empties result sets | `unanchored` is never filtered; only explicitly verified-bad states are withheld |
| Scorecard shape change breaks tooling | Bump `scorecard_version` to 2.0 and update `tools/compare_locomo.py` plus its `--self-check` in the same commit |
| New config keys silently change behaviour | Every new key defaults to today's behaviour (`decay_lambda = 0.0`, `evidence_window = 0`, `max_per_type = 0`); only `verify.serve_stale = false` is intentionally opinionated |
