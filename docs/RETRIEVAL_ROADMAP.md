# memlayer Retrieval Roadmap

> Promotion plan for the three eval-side crates (`memlayer-embed`,
> `memlayer-extract`, `memlayer-eval`) from scaffolding into the
> production daemon. Companion to [ROADMAP.md](ROADMAP.md), which covers
> the broader v1.0 milestone.

## Why this exists

The crates are all scaffolded and self-tested but the daemon never calls
them. This doc maps each crate to its production-promotion benefits,
costs, and the order in which they should land.

The single most important question answered here: **what do we get from
fully integrating these three crates, and in what order?**

## 1. `memlayer-embed` — dense semantic retrieval

**What it is today:** `BgeSmallEmbedder` (BGE-small-en-v1.5, 384-dim,
candle-rs CPU inference) + `EmbeddingCache` (SQLite, sha256-keyed BLOB
storage) + `quantize` module stubbed for int8 compression.

**What full production integration adds:**

| Benefit | Mechanism | Quantitative impact |
|---|---|---|
| Paraphrase recall | Cosine similarity over BGE-small finds "JWT validation" when query is "auth token check" | +8-12 pts on LoCoMo open-domain category |
| Better supersession candidate detection | V3's BM25 picks lexical neighbor; embedding picks semantic neighbor — fewer false negatives where prior obs uses different vocabulary | Multi-hop / adversarial categories: +1-2 pts |
| `memlayer obs similar <id>` | Cosine top-K against any observation as anchor | New UX, no benchmark category |
| Cross-language fuzz match | "auth", "authentification", "authn" all collide in embedding space | Real-world session usability lift |
| Hybrid + RRF default | RRF(BM25 top-30, dense top-30) — implemented in `memlayer-eval/retrieve_hybrid.rs`, just needs daemon wiring | Composite +10-15 pts on LoCoMo overall |

**Costs:**

| Cost | Magnitude | Mitigation |
|---|---|---|
| Save latency | +20-50ms per save (CPU BGE-small) | Async embedding worker thread; save returns before embed done; vector backfilled on next read |
| Storage | +1.5 KB per observation (384 × f32) | int8 quantize halves to 768 B (4× with 768-dim if upgrading to BGE-base) |
| Cold-start | First save needs HuggingFace model download (~120 MB) | Bundle model in install package OR lazy fetch on first save with progress bar |
| sqlite-vec dep | Loadable extension, +~600 KB binary | Statically link via `rusqlite` features; already proven in `memlayer-eval` |
| 10M-scale memory | ~15 GB embeddings raw, ~3.75 GB int8 | Quantize promotion (Spec 5 in primary roadmap) |

**Schema impact (V4 migration):**

```sql
-- Per-project DB
CREATE VIRTUAL TABLE observations_vec USING vec0(
    embedding float[384]
);
CREATE TABLE observation_embedding_meta (
    observation_id  INTEGER PRIMARY KEY REFERENCES observations(id),
    model           TEXT NOT NULL,    -- "bge-small-en-v1.5"
    dim             INTEGER NOT NULL, -- 384
    created_at      TEXT NOT NULL,
    quantized       INTEGER NOT NULL DEFAULT 0
);
```

**Backfill strategy:** on daemon startup, if `observations_vec` row count
< `observations` non-deleted count, schedule a background worker thread
to embed missing rows. Bounded by `MEMLAYER_EMBED_WORKERS` (default 2)
so it doesn't compete with foreground saves.

## 2. `memlayer-extract` — fact-level retrieval

**What it is today:** `HaikuExtractor` shells out to Claude Haiku per turn
→ returns `Fact { subject, predicate, object, temporal, salience,
evidence_obs_id }` triples. `ExtractionCache` (sqlite, sha256+fingerprint
keyed) prevents re-extraction. Tolerant JSON parser recovers from noisy
model output.

**What full production integration adds:**

This is the **single most powerful token-reduction lever** memlayer can
pull. Today, retrieving "what's John's deadline" returns the full 800-char
observation that *mentions* the deadline. With facts, retrieval returns:

```
(John, deadline, "2026-06-19", source: obs #42)
```

— ~30 bytes vs. 800. **~25× token reduction per fact-level hit.**

| Benefit | Mechanism | Quantitative impact |
|---|---|---|
| Atomic-fact retrieval | Each obs → 3-7 facts indexed in `facts` table; `obs context` returns facts not full bodies | 5-25× token reduction per query |
| Temporal reasoning | Each fact carries `temporal` field; "what was true on 2026-06-15" filterable | +3-5 pts on LoCoMo temporal category |
| Salience ranking | Facts carry `salience` score (0-1) from extractor; rare/specific facts ranked above generic ones | Better top-K precision |
| Multi-hop chaining | Facts share `subject` keys; "John works at X" + "X is in Y" → answer "where is John?" | +2-4 pts on multi-hop |
| Fact-level supersession | When new fact contradicts old (same subject+predicate, different object), supersede atomically without disturbing the parent observation | Cleaner history |

**Costs:**

| Cost | Magnitude | Mitigation |
|---|---|---|
| Per-save LLM call | ~200-500 ms + ~$0.0001 (Haiku) | Async; `obs save` returns before extract; facts backfilled |
| Cost runaway at scale | $0.0001 × 10M obs = $1000 | Opt-in via `MEMLAYER_EXTRACT=1` env var; default off |
| Network dependency | Daemon needs Anthropic API access | Cache hits avoid network; `claude` CLI fallback (already supported in `memlayer-extract`) |
| Extraction quality | Haiku occasionally hallucinates facts | `evidence_obs_id` lets users audit; `MEMLAYER_AUDIT_FULL=1` logs the extraction chain |

**Schema impact (V5 migration):**

```sql
CREATE TABLE facts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    obs_id          INTEGER NOT NULL REFERENCES observations(id),
    subject         TEXT NOT NULL,
    predicate       TEXT NOT NULL,
    object          TEXT NOT NULL,
    temporal        TEXT,
    salience        REAL NOT NULL DEFAULT 0.5,
    superseded_by   INTEGER REFERENCES facts(id),
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_facts_obs ON facts(obs_id);
CREATE INDEX idx_facts_subject ON facts(subject);
CREATE VIRTUAL TABLE facts_fts USING fts5(
    subject, predicate, object, content='facts', content_rowid='id'
);
```

**Schema decision:** facts hang off observations (1:N). Deleting an
observation cascades to its facts. This avoids a parallel storage system
— observations remain the source of truth, facts are a *projection*.

## 3. `memlayer-eval` — continuous benchmarking

**What it is today:** standalone benchmark harness with
`BenchmarkKind::{Locomo, Longmemeval, Beam1m, Beam10m}` + `RunReport` +
the full retrieval pipeline (`retrieve.rs`, `retrieve_hybrid.rs`,
`rrf.rs`, `rerank.rs`). Targets: LoCoMo 91.6%, LongMemEval 94.8%, BEAM-1M
64.1%, BEAM-10M 48.6%.

**What full production integration adds:**

| Benefit | Mechanism | Quantitative impact |
|---|---|---|
| Public scorecard | README badge: "LoCoMo: 71.2% (live)". Stored in repo at `eval/scorecard.json`, regenerated by CI | Adoption / trust signal |
| PR-level regression gate | GitHub Action runs LoCoMo on every PR; bot comments score delta vs. main; merge gated on no >2pt regression | Prevents silent retrieval-quality drops |
| Rerank as opt-in production feature | `obs search --rerank` and `obs context --rerank` shell into Haiku reranker (already implemented) | +3-5 pts at query time, opt-in for cost reasons |
| Per-config tuning | `RetrievalConfig` per-profile (default, hybrid, hybrid-rerank) → CLI flag picks profile | Lets ops trade quality for latency |
| Eval-driven roadmap | When LoCoMo plateaus, the harness identifies which category is weakest → next spec targets that gap | Data-driven prioritization |

**Costs:**

| Cost | Magnitude | Mitigation |
|---|---|---|
| CI compute | LoCoMo full run ≈ 10-30 min; LongMemEval ≈ 30-60 min | GitHub Actions cache; nightly full + PR-level subset (50 questions) |
| Dataset licensing | LoCoMo dataset terms permit research use | Document in README; downstream users self-acquire |
| Maintenance burden | Eval scoring metrics drift between papers | Pin to a stable F1 / accuracy definition; document in `eval/METRICS.md` |

**Production wiring:**

1. New `memlayer eval` CLI verb (or standalone binary `memlayer-eval`) —
   runs a benchmark and prints / writes JSON.
2. CI job (`.github/workflows/eval.yml`) — runs PR-level subset on every
   PR; writes scorecard delta as a PR comment.
3. Scorecard publisher — `tools/publish_scorecard.py` updates
   `eval/scorecard.json` on main merges; README parses + displays.

**Schema impact:** none. Eval reads from production storage in read-only
mode against a copy of the project DB.

## 4. Sequencing — what to land first

The three crates are not independent. The right order:

1. **`memlayer-eval` production wiring first.** Without a scorecard, the
   embed and extract promotions can't be measured. ~1 week. No daemon
   changes needed — eval runs standalone, just adds CI + scorecard
   publishing.

2. **`memlayer-embed` second.** Wire the embedder into the daemon's save
   path (async background) + add `--mode hybrid` to search/context.
   Validate via `memlayer-eval` that LoCoMo lifts +10-15 pts. ~2-3 weeks.

3. **`memlayer-extract` third.** Add the `facts` table + opt-in
   extraction worker (`MEMLAYER_EXTRACT=1`). Validate via `memlayer-eval`
   that LoCoMo temporal/multi-hop categories lift. ~2 weeks.

This ordering means each promotion is *measurable* at the moment it
lands. Promoting embed before eval would mean shipping it without
confirming the LoCoMo lift; promoting extract before embed would mean
wasting Haiku calls on observations that aren't yet retrievable in
hybrid mode.

## 5. Combined trajectory (production daemon path)

| Stack | LoCoMo | LongMemEval | BEAM-1M | Tokens reduced (mixed agentic) |
|---|---|---|---|---|
| Today (BM25 only) | 55-62% | 60-65% | not measured | ~2-3× |
| + eval scorecard published | 55-62% (no change — measurement only) | 60-65% | 50-55% | ~2-3× |
| + memlayer-embed in daemon | 70-77% | 80-85% | 58-62% | ~2-3× (retrieval is not the token driver here — content is) |
| + memlayer-extract (facts) | 73-80% | 84-89% | 60-65% | **8-15×** ← fact-level retrieval is the token-reduction lever |
| + Haiku rerank (opt-in) | 78-85% | 88-92% | 62-65% | 8-15× |
| Eval-tuned ceiling (per spec targets) | 91.6% | 94.8% | 64.1% / 48.6% | n/a |

The gap between the stack-end (78-85%) and the eval-tuned ceiling (91.6%)
is the cost of being a daemon — production cannot afford the
full-rerank, full-cache, full-extract path on every query.

## 6. Risks

- **Embedding model drift.** BGE-small evolves — switching from v1.5 → v2
  invalidates stored vectors. Plan: `model` column on
  `observation_embedding_meta` plus a re-embed migration that reads old +
  writes new in batches. Costs CPU but no data loss.
- **Fact extraction quality.** Haiku occasionally hallucinates facts that
  don't appear in the source text. Plan: every fact carries
  `evidence_obs_id`; consumers can verify; `MEMLAYER_AUDIT_FULL=1` logs
  the prompt+response for spot-checks.
- **Eval gameability.** A change can lift LoCoMo while regressing real
  production behavior (e.g. overfitting to LoCoMo's question
  distribution). Plan: also run LongMemEval + a held-out internal eval
  set on every PR to catch this.
- **Cost runaway with extract.** If a user has `MEMLAYER_EXTRACT=1` and
  saves 10K observations, that's $1 of Haiku. Plan: per-day budget cap
  env var, bail with a stderr warning.

## 7. Out of scope

- Choosing a different embedding model (BGE-large, bge-m3, OpenAI
  ada-002). BGE-small is locked by the existing scaffold; switching is a
  separate spec.
- LLM-as-fact-extractor alternatives (GPT-4o-mini, local llama). The
  `Extractor` trait makes swap easy but BGE+Haiku is the proven path.
- Streaming / incremental retrieval (return early hits while still
  searching). Production answers in <50ms, no need yet.
- Per-user / per-team embeddings (vs. per-project). Cross-project mirror
  DB stays BM25.

## 8. References

- `crates/memlayer-embed/` — `BgeSmallEmbedder`, `EmbeddingCache`, `quantize`
- `crates/memlayer-extract/` — `HaikuExtractor`, `Fact`, `ExtractionCache`
- `crates/memlayer-eval/` — `BenchmarkKind`, `retrieve_hybrid`, `rrf`,
  `rerank`
- `.catalyst/specs/retrieval-upgrade-v1/spec.md` — eval-side hybrid
  retrieval design (already approved, partially implemented)
- `docs/ROADMAP.md` — broader v1.0 milestone plan
