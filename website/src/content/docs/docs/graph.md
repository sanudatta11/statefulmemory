---
title: Entity graph
description: The cue graph that links observations through entities and edges — powers budgeted multi-hop briefing expansion in `obs context`.
---

:::note[Heuristics only]
Entity extraction is **heuristics-only** (`anchor` → backtick → path token). No LLM
runs on the save path or retrieval for the graph substrate. The graph is
per-project SQLite, with `co_occurs` edges written but excluded from traversal by
default (noise control).
:::

The **entity graph** (schema head `V11`) links observations through shared
entities — files, symbols, concepts, people, agents — and typed edges
(`mentions`, `fixes`, `contradicts`, `about`). It is what makes
`obs context` return the *right neighborhood* on the agent's first call.

## How it works

1. **Save** — `WriteRequest::IndexGraph` rides the per-project write thread:
   entities are upserted (dedupe by `norm_name`), mentions wired, pairwise
   `mentions` edges accumulated.
2. **Query** — `obs search` / `obs context` extract query entities, walk up to
   `graph.hops` hops over `graph.edge_types`, and fuse a third rank list into
   the hybrid RRF path via `graph_rank::graph_score`.
3. **Briefing** — `obs context` appends budgeted graph-expanded hits capped at
   `graph.budget_pct × max_tokens`; it **never displaces primary hits**, and
   stale/withdrawn observations drop out with the same rules as the main path.

## CLI

```bash
memlayer graph query <entity> --hops 2          # traverse the neighborhood
memlayer graph query <entity> --relation fixes  # narrow to a relation
memlayer graph rebuild                          # backfill entities from anchors
memlayer graph stats
```

## Config

```toml
[graph]
enabled = true       # default off until the CI multi-hop gate passes (+5 pts)
hops = 2             # hard cap 2
boost = 0.15         # post-fusion multiplier for graph-lift-only hits
edge_types = ["mentions", "fixes", "contradicts"]
budget_pct = 0.4     # max share of briefing tokens from expansion
degree_cap = 256     # skip traversal into hub entities
max_query_entities = 3
```

Environment overrides: `MEMLAYER_GRAPH_ENABLED`, `MEMLAYER_GRAPH_HOPS`,
`MEMLAYER_GRAPH_BOOST`.

## Rationale (SQLite, not a graph database)

The graph stays in the same SQLite file as observations ("inspectable by
default" thesis). Recursive-CTE traversal at ≤2 hops with a degree cap returns
in milliseconds; a dedicated graph engine (Neo4j-grade) is out of scope by
design — see the gap list's "do not build Neo4j-scale graphs" rule.

## MCP

`memory_graph_query(entity, hops?, relation_filter?)` returns the neighborhood
as a tree of `from --[relation]--> to` edges plus observation provenance when
an edge carries `src_observation_id`.

## RPCs

Additive: `ListEntities(status)`, `GetEntity`, `GraphQuery`.