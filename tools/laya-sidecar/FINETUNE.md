# Fine-tuning Laya for StatefulMemory (Wave 4b)

Day-1 ship does **not** require a custom fine-tune. Use off-the-shelf
`typed-decisions` + english root; collect teacher logs first.

## Collect teachers

With `[laya] enabled` and Claude still backing decide/conflict, the daemon
appends JSONL under:

```text
~/.statefulmemory/laya_train/
  decide.jsonl
  conflict.jsonl
  resolve.jsonl
  router.jsonl
```

Each row: `{ ts, kind, state, questions, answers }` (content truncated; scrub
secrets before sharing).

## Train

1. Official notebook: `laya_finetune_typed_decisions_2xT4_kaggle.ipynb`
   (~4–5h on free Kaggle 2×T4, RLCD + temperature fit)
2. Base: `convaiinnovations/laya`
3. Target schemas must match production (conflict 4-way, resolve 4-way,
   decide choice options, router `{easy,normal,hard}`)
4. Push private HF checkpoint or save under `~/.statefulmemory-models/laya-smem/`

## Load custom weights

```bash
export LAYA_MODEL_ID=your-org/laya-smem
# restart sidecar / daemon
```

Or set env on the install-managed venv spawn (see README).

## Gate before flipping defaults

Compare Laya-smem vs Claude teacher on held-out fixtures (agreement %, ECE).
Only prefer the custom checkpoint when agreement is solid and router p99
does not regress vs heuristic-only.
