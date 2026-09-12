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

# Re-print a saved scorecard against published bands
make eval-locomo-compare SCORECARD=eval/locomo-full.json

# Staleness: supersession vs add-only baseline (committed fixture, no network)
make eval-staleness
```

Scorecards land in `eval/` (gitignored). Published reference numbers live in
`baselines/locomo.json`.

`make extract-locomo` / `make run-locomo` still drive the older `eval` binary
inside this crate (conv-26 extract, `--limit 200`). Use the `eval-locomo*`
targets for CLI e2e and local analysis.

## Metrics

memlayer `--save-scorecard` (scorecard 2.0) writes `accuracy_pct` (judge or
lexical pass rate), `recall_at_k`, `mrr`, and optional `by_category`. It does
**not** emit paper token F1. Staleness runs also report `superseded_served_pct`
(lower is better). See `baselines/locomo.json` for published LoCoMo reference
bands.
