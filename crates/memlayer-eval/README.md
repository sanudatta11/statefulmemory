# memlayer-eval

Benchmark harness for LoCoMo, LongMemEval, BEAM, and a staleness fixture. The
daemon does not depend on this crate.

## Local LoCoMo (preferred)

From the repo root:

```bash
# Wiring check: BM25 + lexical judge, no dataset, no LLM
make eval-locomo-smoke

# Fetch SNAP locomo10.json into ./data/locomo/
make eval-locomo-fetch

# Full e2e: ingest + hybrid-rerank + LLM answer/judge (needs BGE + agent CLI)
make eval-locomo

# Smoke then full
make eval-locomo-e2e

# Cheaper full slice
LIMIT=50 make eval-locomo

# Optional: extract facts.db for all conversations, then run
EXTRACT=1 LIMIT=50 make eval-locomo
# or: make eval-locomo-extract

# Parallel answer/judge (default 4)
MEMLAYER_EVAL_CONCURRENCY=4 LIMIT=50 make eval-locomo

# Re-print a saved scorecard against published bands
make eval-locomo-compare SCORECARD=eval/locomo-full.json

# Staleness: supersession vs add-only baseline (committed fixture, no network)
make eval-staleness
```

### Pinned measure recipe

```bash
export MEMLAYER_LLM_PROVIDER=opencode
export MEMLAYER_LLM_BIN=opencode
export MEMLAYER_LLM_MODEL=opencode-go/deepseek-v4.1-flash
export MEMLAYER_EVAL_CONCURRENCY=4
LIMIT=50 make eval-locomo
```

Compare `accuracy_pct`, evidence-based `recall_at_k` / `mrr`, optional
`gold_substring_recall`, `rerank_skipped_pct`, retrieval/e2e p50, and
`by_category`. Save scorecards under `eval/` with descriptive names.

Scorecards land in `eval/` (gitignored). Published reference numbers live in
`baselines/locomo.json`.

`make extract-locomo` / `make run-locomo` still drive the older `eval` binary
inside this crate (conv-26 extract, `--limit 200`). Use the `eval-locomo*`
targets for CLI e2e and local analysis.

## Metrics

memlayer `--save-scorecard` (scorecard 2.0) writes `accuracy_pct` (judge or
lexical pass rate), evidence-aware `recall_at_k` / `mrr` (falls back to gold
substring when the dataset has no evidence ids), optional
`gold_substring_recall`, `rerank_skipped_pct`, and optional `by_category`. It
does **not** emit paper token F1. Staleness runs also report
`superseded_served_pct` (lower is better). See `baselines/locomo.json` for
published LoCoMo reference bands.
