# Spec: Graph Briefing Layer (Agent-Optimized Entity Graph)

Status: in-progress · Target: memlayer v2 Phase 1 · Schema head: V10 → V11 · CI gate: `eval.yml` multi-hop threshold

Design basis: cue-tag-content heterogeneous graph (MRAgent, ICML 2026), typed causal
edges, budgeted subgraph assembly inside `obs context` (token-budget share), provenance
lines in output. No LLM at save or retrieval for the graph substrate. Per-project
SQLite only (global mirror unchanged, stays BM25 text).

## Goal

Improve multi-hop retrieval quality and default briefing composition so a coding
agent gets the right memory neighborhood on its first `obs context` call — no extra
MCP tool calls, no LLM dependency, no external graph database.

**Hard ship gate** (enforced in CI): stratified LoCoMo multi-hop slice +5 pts AND
daemon p95 `obs context` < 50 ms synthetic (CI threshold < 80 ms) before
`[graph] enabled = true` ships default-on. Config ships `enabled = false` until both
gates pass.

## Non-goals

- No Neo4j / embedded graph DB / tree-sitter. Pure SQLite.
- No LLM entity extraction (tier-2 judge pass explicitly deferred).
- Co-occurrence edges off by default (noise control).
- Only ONE new agent-facing verb (`memlayer graph query`) + ONE MCP tool
  (`memory_graph_query`). Briefing enrichment flows through existing
  `memlayer obs context` / `memory_context` untouched signatures.
- No second write connection; all writes ride the existing `WriteRequest` thread.

## Schema — migrations/V11__graph_briefing.sql

```sql
CREATE TABLE IF NOT EXISTS entities (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (kind IN ('file','symbol','concept','person','agent')),
    name        TEXT NOT NULL,
    norm_name   TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS entity_mentions (
    entity_id      INTEGER NOT NULL REFERENCES entities(id),
    observation_id INTEGER NOT NULL,
    offsets        TEXT,            -- "[start,end]" char spans, nullable
    source         TEXT NOT NULL CHECK (source IN ('anchor','backtick','topic','token'))
);
CREATE INDEX IF NOT EXISTS idx_entity_mentions_obs ON entity_mentions(observation_id);
CREATE INDEX IF NOT EXISTS idx_entity_mentions_ent ON entity_mentions(entity_id);

CREATE TABLE IF NOT EXISTS entity_edges (
    id INTEGER PRIMARY KEY,
    from_entity INTEGER NOT NULL REFERENCES entities(id),
    to_entity   INTEGER NOT NULL REFERENCES entities(id),
    relation    TEXT NOT NULL CHECK (relation IN ('mentions','fixes','contradicts','about','co_occurs')),
    weight      REAL NOT NULL DEFAULT 1.0,
    first_seen  INTEGER,
    last_seen   INTEGER,
    src_observation_id INTEGER
);
CREATE INDEX IF NOT EXISTS idx_entity_edges_from ON entity_edges(from_entity);
CREATE INDEX IF NOT EXISTS idx_entity_edges_to   ON entity_edges(to_entity);
```

Backfill: one entity row per existing `observation_anchors` entry
(`source='anchor'`, kind `file` for path part / `symbol` for symbol part), mention
rows wired in the same pass. Resumable: watermark in existing meta/registry
mechanism; `memlayer graph rebuild --full` re-runs idempotently
(`INSERT OR UPDATE`), no destructive rewrites.

Ownership rule: `entity_edges` owns entity-entity links only.
`observation_relations` remains the owner of observation-observation relations
(supersedes / conflicts_with). An edge MAY carry `src_observation_id` referencing a
relation's observation; relations are never duplicated into edges.

## Components (crate-by-crate)

### memlayer-core — config + types
- `GraphConfig { enabled: bool (false), hops: u8 = 2, boost: f64 = 0.15,
  edge_types: Vec<String> = ["mentions","fixes","contradicts"],
  budget_pct: f64 = 0.4, degree_cap: usize = 256 }` under `[graph]`.
- `EntityKind`, `EdgeRelation`, `GraphQueryOutcome` types shared by all crates.

### memlayer-extract — heuristic resolver (tier 1 only)
- `resolve.rs::extract_entities(title, content, anchors) -> Vec<RawEntity>`
  - `path::symbol` anchors → kind symbol/file (zero-cost: anchor rows already exist)
  - backtick tokens in title/content → concept/person/agent via keyword lexicon
  - path-like tokens → file
  - normalization: casefold, strip punctuation; merge by `norm_name` only
  - no LLM pass; heuristic miss = absent cue (acceptable degradation)
- Pure functions, no DB access; daemon wires into write path.

### memlayer-storage — migrations + query helpers
- V11 file (above); refinery picks it up automatically.
- `graph.rs`: `entity_lookup(conn, norm_prefix)`, `neighbors(conn, entity_id,
  hops, edge_types, degree_cap)` (recursive CTE, hops ≤ 2 hard cap),
  `observations_for_entities(conn, ids, limit, exclude_withdrawn: bool)`,
  `backfill_from_anchors(conn)`.
- Read helpers run on read connections only.

### memlayer-retrieval — fusion hook
- `graph.rs::graph_rank_list(...) -> Vec<(id, score)>`: score = Σ
  `edge_weight × type_boost × decay(hop_distance)`; capped result set (64).
- Query entity extraction reuses `memlayer-extract::resolve` normalizer.
- Hybrid path (daemon `service.rs::hybrid_search`): third rank list into the
  existing `facts_fuse::fuse_observation_lists_scored` call — no new RRF.
  Post-fusion, graph-lift-only hits get `boost` multiplier.

### memlayer-daemon — write path + context assembly
- New `WriteRequest::IndexGraph { observation, anchors }` handled inside the
  existing `process_batch` match; also enqueued post-verify from existing flows.
- `obs context` assembly: after hybrid retrieval, budgeted expansion:
  - centroid = top ≤3 query entities
  - BFS hops ≤ `graph.hops`, edge-type + degree-cap filtered
  - expansion hard-capped at `budget_pct` × `max_tokens`
  - per-hit provenance line: `via entity: <name>`
  - staleness withdrawn exactly as today (`context_filter`), graph hits
    touching withdrawn observations drop from assembly
- Latency budget: expansion allowed only if hybrid path used ≤ 30 ms of p95 budget.

### memlayer-proto — additive RPCs
- `ListEntities(prefix)`, `GetEntity(id)`, `GraphQuery(entity, relation_filter?,
  hops?)` + messages `Entity`, `GraphQueryResult`. No removals/renames;
  server trait impl in `service.rs`, new MCP tool calls `GraphQuery`.

### memlayer-mcp
- One new tool `memory_graph_query` (args: entity string, hops ≤ 2,
  relation filter). All other tool signatures unchanged.

### memlayer-cli
- `memlayer graph query <entity> [--hops N] [--relation R]`,
  `memlayer graph rebuild --full`, `memlayer graph stats`.

## Acceptance

1. Stratified multi-hop +5 pts (CI gate, Stage 0 eval job).
2. p95 `obs context` < 50 ms at graph=on over 10k-obs synthetic fixture.
3. `graph.enabled=false` → byte-identical output vs current behavior.
4. No second write connection (WriteRequest only).
5. Mention precision eyeball pass on 100-observation sample: no garbage rows
   in top-50 norm_name lookup list.
6. Round-trip readiness: `mem export` v2 (Phase 2) must carry the 3 new tables.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Entity resolver noise | norm merge, edge-type default filter, conservative boost |
| Hub-entity hop explosion | degree_cap 256 on CTE + budget_pct hard stop |
| Ranking churn | boost 0.15, off-switch retained, CI scorecard diff |
| p95 regression | expansion behind 30 ms hybrid-budget monitor; disable via config |

## References

- `MEM0_COMPARISON.md` §6.5 (entity boost), §7 (don't build Neo4j-scale graphs)
- `docs/ROADMAP.md` Spec 3 (anchors shipped, bridge still open)
- MRAgent arXiv:2606.06036 · MemORAI ACL 2026 Findings · Zep arXiv:2501.13956
