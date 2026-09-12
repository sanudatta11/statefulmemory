---
title: LoCoMo eval
description: How to run LoCoMo smoke and full scorecards locally — public win claims wait on stratified disclosed runs.
---

Use the Makefile from the repo root. Smoke needs only a release binary. Full
needs `data/locomo/locomo10.json`, a BGE model directory, and an agent CLI for
answer + judge.

**Hold on product claims:** do not treat smoke, prefix-LIMIT cards, or unmatched
peer leaderboard numbers as memlayer product truth. Public “wins” wait on
disclosed stratified scorecards (same judge, k, and category filter).

```bash
make eval-locomo-smoke          # fixture, BM25, lexical judge
make eval-locomo-fetch          # SNAP locomo10.json → ./data/locomo/
make eval-locomo                # full locomo10 e2e
make eval-locomo-e2e            # smoke then full
LIMIT=50 make eval-locomo       # cheaper stratified slice (cats 1–4)
# facts.db is built automatically when missing (EXTRACT=0 to skip)
make eval-locomo-compare SCORECARD=eval/locomo-full.json
make eval-staleness             # supersession vs --no-supersede baseline
```

Scorecards are written to `eval/` (not committed). Equivalent CLI:

```bash
memlayer eval --smoke --benchmark locomo --save-scorecard eval/locomo-smoke.json
MEMLAYER_EVAL_DATA="$PWD/data" memlayer eval --benchmark locomo --save-scorecard eval/locomo-full.json
memlayer eval --smoke --benchmark staleness --save-scorecard eval/staleness.json
memlayer eval --smoke --benchmark staleness --no-supersede --save-scorecard eval/staleness-baseline.json
```

## What the numbers mean

| Run | Metric | Comparable to |
| --- | --- | --- |
| Smoke | Lexical contain on one fixture query | Nothing published |
| Full `accuracy_pct` | LLM-judge pass rate on locomo10 | Other locomo10 LLM-judge harnesses **if** judge model, k, and category filter match |
| `recall_at_k` / `mrr` | Gold present in retrieved set / reciprocal rank | Retrieval-only diagnostics |
| Maharana et al. ACL 2024 | Token F1 on gold answers | Human 87.9, GPT-4-turbo 51.6 overall |
| Staleness `superseded_served_pct` | Superseded value served without gold | Lower is better; compare default vs `--no-supersede` |

Published LLM-judge peers (**unmatched harness** — cite only as context, not as
a memlayer claim): Mem0 ~66.9%, engram-lite ~68.3%, ENGRAM paper ~77.6%,
Engram marketing ~80%. Those bands are not comparable until our stratified
scorecard is disclosed with the same judge and filters.

`LIMIT=N` is **stratified** across SNAP categories mapped as `multi_hop` /
`temporal` / `open_domain` / `single_hop` (adversarial excluded). Prefix
`take(N)` is no longer used for LoCoMo.

To fairly test those bands later: keep the default LoCoMo profile (hybrid +
rerank + evidence window + facts.db), ingest speaker-prefixed turns with
`dia_id` in content, and disclose the answer/judge model. Do not quote smoke
or old unfair cards against a leaderboard.

## Public claims (hold)

memlayer does **not** publish a LoCoMo leaderboard win on this site until a
disclosed, stratified scorecard (model, k, categories 1–4, facts on/off) is
ready. Peer percentages above are **published elsewhere / unmatched harness**.
Do not treat smoke, 10-query flash cards, or unfair prefix-`LIMIT` slices as
product accuracy.
