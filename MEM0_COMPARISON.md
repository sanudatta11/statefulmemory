# Mem0 vs statefulmemory — engineering comparison

**Status:** Untracked working doc for product decisions. Not a public marketing page.  
**Date:** 2026-09-13  
**Scope:** What each product ships (or publicly documents) today, plus honest gaps. Peer features are cited; invented peer capabilities are avoided.

---

## 1. Executive summary

| Verdict | Detail |
|---|---|
| **statefulmemory leads** | Coding-agent-native install (stdio MCP + 14 agent targets); **code anchors + git verify + stale withdrawal**; inspectable per-project SQLite; hybrid BM25+dense **without** a third-party search/embed API key on self-host; LLM steps shell out to the user’s agent CLI (no vendor LLM lock-in for core retrieval). |
| **statefulmemory trails** | Python/Node Memory SDKs; agent-framework ecosystem (LangChain/CrewAI/…); managed Platform maturity (dashboard, webhooks, orgs/projects, usage billing); Graph Memory / Dream-style background consolidation; advertised enterprise compliance (SOC 2 / HIPAA / BYOK); public multi-benchmark story (LongMemEval, BEAM) and GTM breadth. |
| **Differentiates** | statefulmemory = **local-first infrastructure for coding agents in git repos**. Mem0 = **general agent memory Platform (+ OSS library)** for apps and agents across domains, with coding-agent plugins as one GTM wedge. |

**Do not claim a public LoCoMo leaderboard win.** Local statefulmemory scorecards and Mem0’s published bands use **unmatched harnesses** (see §5).

---

## 2. Positioning

### Mem0 thesis (from product surface)

Drop-in **memory infrastructure for AI agents and apps**: add conversation → extract facts → retrieve compressed context. Primary path is **Mem0 Platform** (`MemoryClient` + API key); secondary is **OSS** (`Memory` library / Docker stack) and **OpenMemory MCP** (local MCP server). GTM spans healthcare, education, e‑commerce, support, sales — plus coding-agent integrations. Efficiency pitch: fewer tokens than full-context replay; governance pitch: SOC 2 / HIPAA / BYOK / portable deploy.

Sources: [mem0.ai](https://mem0.ai/), [docs overview](https://docs.mem0.ai/), [Platform vs OSS](https://docs.mem0.ai/platform/platform-vs-oss).

### statefulmemory thesis (from this repo)

**Persistent memory for coding agents**: thin CLI + MCP → gRPC daemon → per-project SQLite (FTS5 + optional sqlite-vec). Capture decisions/patterns/fixes; retrieve via `obs search` / `obs context` / Decide. Self-host is first-class and open source; **StatefulMemory Cloud** is a stated managed SaaS offering (signup/pricing still rolling out — do not invent regions/SLA). Peer names stay out of product UI; comparison belongs in docs like this.

Sources: `README.md`, `PRODUCT.md`, `AGENTS.md`, [why-statefulmemory](https://statefulmemory.dev/docs/why-statefulmemory/).

---

## 3. Feature matrix

Legend: **Shipped** = documented/working today · **Partial** = subset or roadmap · **Offering** = claimed product path, not fully public · **—** = not claimed / not shipped.

| Capability | Mem0 | statefulmemory |
|---|---|---|
| **Memory write** | `add` (messages → LLM extract; `infer=False` for raw). Platform + OSS. | `obs save` / MCP `memory_add` / proto `SaveObservation`. Typed observations (decision, …). |
| **Memory read** | `search` / `get` / `get_all` with entity filters. Multi-signal fusion on Platform. | `obs search` / `obs context` / MCP `memory_search`/`memory_context`/`memory_recent`. Hybrid BM25+dense RRF (default hybrid). |
| **Extract / facts** | Default path: LLM distillation to memories; ADD-only extraction (docs). | Optional extract worker → atomic facts (`GetFacts` / `memory_facts`). Config-gated (`extract.enabled`, default off in code defaults; install bootstrap may enable). |
| **Graph** | **Platform:** native Graph Memory (entity nodes + co-occurrence links → ranking boost). Graph **view** Pro+. **OSS:** entity boost; external Neo4j-style graph store **removed** in v3. | No entity graph. Code **anchors** (`path::symbol`) + planned graphify bridge (not shipped). |
| **Rerank** | Configurable on OSS; advanced retrieval on Platform. | Optional LLM `--rerank` / `search.rerank` via agent CLI; timeout + fallback. |
| **Supersession / consolidation** | Write-path merge/supersede; **Dream** background synthesis (Pro+; weekly). Search `latest_only` / `include_merged`. | LLM conflict judge + soft supersession (`supersedes_ids` / history). No Dream-like scheduled synthesis. |
| **Anchors / verify / staleness** | No git/code-anchor product. Blog content on “memory staleness” + Dream. | **Shipped:** multi-anchor table, `statefulmemory verify`, git hooks, context withdraws `stale`/`invalidated`/`unprovable` unless `serve_stale` / `--include-stale`. |
| **Time decay** | Platform **Memory Decay** (opt-in). | Config `search.decay_lambda` (default 0 = off). |
| **Token budget** | Token-efficiency marketing (~7k mean tokens on research page). | `max_tokens` on search/context → `tokens_used` estimate. |
| **MCP** | Hosted `https://mcp.mem0.ai/mcp` (CRUD-heavy tool set). OpenMemory local MCP (Docker). | Local **stdio** `statefulmemory mcp`: `memory_search`, `memory_recent`, `memory_context`, `memory_add`, `memory_facts`, `memory_health`, `memory_decide`. |
| **CLI** | `mem0` / `mem0-cli` / `@mem0/cli` (Platform init, manage). | `statefulmemory` binary (obs, verify, install, doctor, tui, mem export, eval, …). |
| **SDK** | **Python + Node/TS** Platform + OSS. REST OpenAPI. | **No** Python/TS Memory SDK. Agents use MCP or shell-out. gRPC proto for clients. |
| **Self-host** | OSS library + Docker Compose (Postgres/pgvector, dashboard). OpenMemory MCP local. | Daemon auto-spawn UDS; team TCP+TLS + bearer tokens. Single binary + SQLite. |
| **Cloud / managed** | Mature Platform (Hobby→Enterprise pricing). | StatefulMemory Cloud **offering**; public signup/pricing not claimed as live. |
| **Enterprise / compliance** | Marketing: SOC 2, HIPAA, BYOK, K8s/private/air-gap. Trust Center: **SOC 2 Type I** Compliant; **SOC 2 Type II** In Progress; **HIPAA** Self-Attested (see §8 unverified). On-prem on Enterprise plan. | No published SOC 2 / HIPAA / BYOK. Self-host = data on user’s machine. |
| **Agent integrations** | Broad: coding tools (Claude Code, Cursor, Codex, OpenCode, …) **and** LangChain, LangGraph, CrewAI, LlamaIndex, Vercel AI SDK, voice, etc. | Coding-agent install targets (14). Framework SDKs **not yet**. |
| **Archives / portability** | Platform export jobs; OSS history DB. | `.mem` export/import; sync JSON/MD paths. |
| **Decide / judgment UX** | App-driven; no dedicated “decide” product verb. | `statefulmemory decide` / MCP `memory_decide`. |

---

## 4. Architecture differences

```text
Mem0 (Platform mental model)
  App / agent SDK ──HTTPS──► Mem0 Platform
                               ├─ LLM extract (ADD-only) + temporal metadata
                               ├─ Vector + SQL + entity/graph store
                               ├─ Multi-signal search (semantic + BM25 + entity + temporal)
                               └─ Dream (background merge / supersede / synthesize)

Mem0 OSS
  App ──Memory()──► your LLM + embedder + vector store (default OpenAI + local Qdrant)
  or Docker REST server (Postgres/pgvector + dashboard)

statefulmemory
  Coding agent ──MCP stdio / CLI──► statefulmemory-daemon (gRPC)
                                      ├─ Write thread → project SQLite (FTS5)
                                      ├─ optional sqlite-vec (BGE-small embed worker)
                                      ├─ extract / resolve / verify workers (try_queue)
                                      ├─ hybrid RRF retrieval (+ optional LLM rerank)
                                      └─ global.sqlite BM25 mirror (cross-project)
```

| Axis | Mem0 | statefulmemory |
|---|---|---|
| Unit of memory | Extracted **memories** (facts) scoped by `user_id` / `agent_id` / `run_id` (/ `app_id` on Platform) | **Observations** (title+content+type) + optional atomic **facts**; sessions; code anchors |
| Hot path | LLM extraction on add (async options) | Save is local DB write; LLM extract/conflict/verify off hot path |
| Retrieval default | Dense + keyword + entity (+ temporal on Platform) | BM25 + dense RRF; decay/evidence window/max_per_type config-gated |
| LLM dependency | Platform: Mem0-managed. OSS: typically `OPENAI_API_KEY` (or configured providers). | Core hybrid search: **local** BGE. Extract/judge/rerank/Decide: agent CLI on `PATH` |
| Data locality | Platform cloud; OSS/OpenMemory on your infra | Default `~/.statefulmemory/` inspectable SQLite |
| Wire protocol | REST / SDK; hosted MCP over HTTPS | gRPC (`proto/statefulmemory.proto`); local stdio MCP |

---

## 5. Benchmark honesty (LoCoMo and friends)

### What Mem0 advertises

| Source | Claim (approx.) | Notes |
|---|---|---|
| arXiv [2504.19413](https://arxiv.org/abs/2504.19413) | Mem0 LoCoMo **LLM-as-a-Judge ~66.88%** overall (Table 2); graph variant slightly higher | Paper-era pipeline; GPT-4o-mini era eval |
| [docs Memory Evaluation](https://docs.mem0.ai/core-concepts/memory-evaluation) / [Research](https://mem0.ai/research) | LoCoMo **92.5**, LongMemEval **94.4**, BEAM 1M **64.1** / 10M **48.6**; ~7k mean tokens; top_200 retrieval | Explicitly: **managed Platform** + proprietary opts; OSS “directionally similar, not identical” |
| GitHub issues ([#2800](https://github.com/mem0ai/mem0/issues/2800), [#3943](https://github.com/mem0ai/mem0/issues/3943)) | OSS local LoCoMo often **much lower** than paper; maintainers: paper numbers were **SaaS `MemoryClient`**, not OSS `Memory` | Unmatched OSS vs Platform |

**Peer band used in statefulmemory docs:** Mem0 ~**66.9** LLM-judge — cite as **published / unmatched harness**, not a head-to-head.

### What statefulmemory has locally

From `eval/locomo-full.json` (local, gitignored-style scorecard; **not** a disclosed public win):

| Field | Value |
|---|---|
| Slice | `LIMIT=50` stratified (cats 1–4) |
| Profile | hybrid-rerank, evidence window w=6 |
| Answer / judge | `opencode-go/deepseek-v4.1-flash` |
| `accuracy_pct` | **82.0** (41/50) |
| `recall_at_k` | **0.92** |
| `mrr` | ~0.77 |

**Caveats (mandatory):**

1. Different judge models, k, retrieval budget (Mem0 research uses top_200), category filters, and answer models → **not comparable**.
2. statefulmemory 50-query stratified slice ≠ full locomo10 disclosure.
3. Mem0’s **92.5** and paper **66.9** are themselves different generations of their product — do not mix them casually.
4. statefulmemory product policy: **no public leaderboard win** until a stratified disclosed scorecard (judge, k, categories, facts on/off) is ready. See `website/src/content/docs/docs/locomo.md`.

### Other Mem0 benchmarks

LongMemEval and BEAM are advertised with open eval tooling ([memory-benchmarks](https://github.com/mem0ai/memory-benchmarks) per docs). statefulmemory-eval scaffolds those names; production scorecard pressure today is LoCoMo + staleness fixtures.

---

## 6. Gap list for statefulmemory (prioritized)

### P0 — competitive table-stakes for coding-agent buyers comparing Mem0

1. **Disclosed stratified LoCoMo scorecard** — same public methodology page as today, but commit/publish a full cats 1–4 card with pinned answer/judge models; keep unmatched-peer footnotes. Blocks honest GTM.
2. **StatefulMemory Cloud path clarity** — signup, regions-or-explicit-TBD, pricing-or-waitlist, how MCP/CLI auth works vs self-host. Avoid “Cloud vapor” perception vs Mem0 Hobby free tier.
3. **Document OpenMemory-vs-statefulmemory local MCP** — one-pager: stdio vs Docker SSE, SQLite vs Qdrant/Postgres, no OpenAI key for hybrid search, anchors/verify. Captures local-privacy buyers Mem0 already courts.

### P1 — close product gaps without abandoning niche

4. **Background consolidation (“Dream-lite”)** — scheduled merge/dedupe/synthesize **or** explicit `statefulmemory dream` offline pass; statefulmemory already has supersession + history; needs UX + non-destructive lifecycle states.
5. **Entity / multi-hop retrieval boost** — lighter than full Graph Memory: extract entities on save, boost at search (aligns with LoCoMo multi-hop weakness on local card: ~55% multi_hop on LIMIT=50).
6. **Python or TypeScript thin client** — even a generated gRPC/HTTP wrapper lowers barrier vs `mem0ai` SDK muscle memory.
7. **Auto-anchor + graphify bridge** — roadmap Spec 3 remainder; unique vs Mem0 if shipped.
8. **Eval CI scorecard promotion** — regression gate; LongMemEval/BEAM smoke when affordable.

### P2 — nice-to-have / selectively copy

9. Hosted MCP (streamable HTTP) for shared team servers (roadmap follow-up).
10. Dashboard for memory browse (TUI exists; web UI is Mem0 strength).
11. Framework cookbooks (LangChain/…) only if expanding beyond coding agents.
12. Compliance pack (SOC 2) only if Cloud becomes a real revenue path.
13. Multimodal memory (images/PDFs) — Mem0 Platform feature; low priority for coding-agent text/decisions.

---

## 7. Where statefulmemory should NOT copy Mem0

| Temptation | Why not |
|---|---|
| Become a general “AI companion / healthcare / CRM memory” Platform | Dilutes coding-agent wedge; Mem0 already owns that GTM. |
| Require cloud extract LLM for every save | Breaks local-first / air-gapped coding workflows; hybrid search without API keys is a real differentiator. |
| Chase Mem0’s **92.5** LoCoMo with top_200 + proprietary Platform-only tricks as marketing | Violates unmatched-harness honesty; invites reproducibility backlash Mem0 already faces on OSS. |
| Reimplement Neo4j-scale knowledge graphs | Roadmap correctly prefers graphify bridge + anchors over 30k+ LOC AST graph. |
| Hide memory in opaque SaaS only | Inspectable SQLite + git-tied staleness is the product thesis. |
| Mirror Dream synthesis as silent cloud rewrite of project decisions | Coding decisions need reviewable supersession + git verify, not weekly opaque “sleep” merges. |
| Inflate star-count / “150k developers” style social proof | Engineering buyers care about install path and inspectability. |

**Stay in niche:** decisions, conventions, fixes, session memory, anchors vs HEAD — infrastructure next to Cursor/Claude Code/OpenCode, not a horizontal Memory SaaS clone.

---

## 8. Sources

### Mem0

- Product home: https://mem0.ai/
- Pricing: https://mem0.ai/pricing
- Research / benchmark UI: https://mem0.ai/research
- Docs index: https://docs.mem0.ai/ · https://docs.mem0.ai/llms.txt
- How it works: https://docs.mem0.ai/core-concepts/how-it-works
- Memory evaluation (LoCoMo 92.5 / LongMemEval / BEAM): https://docs.mem0.ai/core-concepts/memory-evaluation
- Platform vs OSS: https://docs.mem0.ai/platform/platform-vs-oss
- Graph Memory: https://docs.mem0.ai/platform/features/graph-memory
- Dream docs: https://docs.mem0.ai/platform/features/dream
- Dream announcement: https://mem0.ai/blog/dream-background-memory-consolidation-for-ai-agents
- Hosted MCP: https://docs.mem0.ai/platform/mem0-mcp · https://mcp.mem0.ai
- OpenMemory MCP launch: https://mem0.ai/blog/introducing-openmemory-mcp · https://github.com/mem0ai/mem0/tree/main/openmemory
- Trust Center: https://trust.mem0.ai/
- Paper: https://arxiv.org/abs/2504.19413
- OSS vs paper scores: https://github.com/mem0ai/mem0/issues/2800 · https://github.com/mem0ai/mem0/issues/3943
- GitHub org/repo: https://github.com/mem0ai/mem0

### statefulmemory

- Repo: `README.md`, `PRODUCT.md`, `AGENTS.md`, `CLAUDE.md`, `docs/ROADMAP.md`
- Site: https://statefulmemory.dev/ · https://statefulmemory.dev/docs/why-statefulmemory/ · https://statefulmemory.dev/docs/locomo/
- Local scorecard: `eval/locomo-full.json` (LIMIT=50, 82% / recall@k 0.92 — unmatched)
- Proto / MCP: `proto/statefulmemory.proto`, `statefulmemory mcp` tools listed in `AGENTS.md`

---

## Appendix A — Claims that could not be fully verified

| Claim | Status |
|---|---|
| Mem0 **Kubernetes / air-gapped** “same API everywhere” | Homepage marketing. Public docs emphasize Docker Compose / library self-host; **no public Helm/K8s runbook verified** in this pass. Enterprise “on-prem” is on the pricing page — treat as sales-led until docs link. |
| **BYOK / zero-trust** | Named on homepage; **not validated** against Trust Center control list in this pass (Trust Center is a JS app; search snippets show SOC 2 Type I Compliant, Type II In Progress, HIPAA Self-Attested). |
| **GDPR** | Nav “GDPR Ready”; `/gdpr` fetch 404’d. Likely marketing/legal page elsewhere — **not independently verified**. |
| LoCoMo **92.5** judge/answer model defaults | Docs say configurable ±1 pt; defaults deferred to eval repo — **not pinned** here. |
| **Memory Compression Engine** | Homepage feature blurb; pipeline described as extract+retrieve — **engine internals not separately documented** beyond research “token-efficient” algorithm posts. |
| **Mem0 Gateway** | Mentioned in OpenMemory blog as team control plane; **not deep-dived**. |
| OpenMemory vs current OSS Docker | OpenMemory blog (2025) vs 2026 OSS setup docs may have drifted — confirm against `mem0/openmemory` before product claims. |
| StatefulMemory Cloud live SLA/regions/pricing | Explicitly **not** claimed as shipped in `PRODUCT.md`. |

---

## Appendix B — Quick decision guide

| Buyer need | Prefer |
|---|---|
| Ship memory into a Python/TS app tomorrow with hosted API | **Mem0 Platform** |
| Local coding agents, git-tied staleness, no embed API key | **statefulmemory self-host** |
| Local MCP across Cursor/Claude with Docker UI | **OpenMemory MCP** (Mem0) *or* statefulmemory stdio MCP — compare ops cost |
| SOC 2 / HIPAA questionnaire for enterprise procurement | **Mem0** (with Trust Center caveats) today |
| Inspect SQLite under `~/.statefulmemory/` and `statefulmemory doctor` | **statefulmemory** |
