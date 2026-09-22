# Laya System-1 sidecar (StatefulMemory Wave 4)

Optional HTTP sidecar that serves [Laya](https://huggingface.co/convaiinnovations/laya)
typed decisions for the daemon. Search/context stay Rust (BM25 + BGE + local CE);
this process only answers discrete System-1 questions (router tier, decide, conflict).

## Quick start (recommended)

`smem` and `statefulmemory` are the same binary. Either install command sets up
Laya prerequisites out of the box:

```bash
smem install                 # or: statefulmemory install
# smem install --no-laya     # skip sidecar prereqs (config keys still merge)
```

What install does:

1. Merges `[laya] enabled = true` into `~/.statefulmemory/config.toml` (missing keys only)
2. Writes `~/.statefulmemory/laya-sidecar/{server.py,requirements.txt,run.sh}`
3. Creates `~/.statefulmemory/laya-venv` and `pip install -r requirements.txt`
4. Best-effort starts uvicorn on `127.0.0.1:8765`

Needs **Python 3.10+** on `PATH`. First model download can take a few minutes;
the daemon soft-retries until `/health` is ok. Loopback only for auto-spawn —
set `[laya] url` to a shared host for team setups (no auto-spawn).

Verify:

```bash
smem doctor                  # prints laya enabled/url/health
curl -s http://127.0.0.1:8765/health
```

## Manual (repo checkout)

```bash
cd tools/laya-sidecar
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
# From repo root:
make laya-sidecar
# or:
USE_TF=0 uvicorn server:app --host 127.0.0.1 --port 8765
```

## Config

```toml
# ~/.statefulmemory/config.toml
[laya]
enabled = true
url = "http://127.0.0.1:8765"   # or http://team-host:8765
timeout_ms = 80
router = true                   # Easy/Normal/Hard — fallback = heuristic only
decide = true                   # silent Claude fallback
conflict = true                 # silent Claude / heuristic fallback
model_decide = "typed-decisions"
model_router = "english"
min_confidence = 0.35
```

Env overrides: `STATEFULMEMORY_LAYA_ENABLED`, `STATEFULMEMORY_LAYA_URL`,
`STATEFULMEMORY_LAYA_TIMEOUT_MS`, `STATEFULMEMORY_LAYA_ROUTER`,
`STATEFULMEMORY_LAYA_DECIDE`, `STATEFULMEMORY_LAYA_CONFLICT`,
`STATEFULMEMORY_LAYA_MIN_CONFIDENCE`.

## Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | `{ ok, loaded, error }` |
| POST | `/v1/predict` | `{ state, questions, model? }` → normalized answers |

`model`: `typed-decisions` (decide/conflict) or `english` (router / sufficiency).

## Sidecar env

| Var | Default | Meaning |
|-----|---------|---------|
| `LAYA_PORT` | `8765` | Bind port |
| `LAYA_HOST` | `127.0.0.1` | Bind host |
| `LAYA_MODEL_DECIDE` | `typed-decisions` | Decide/conflict checkpoint nickname |
| `LAYA_MODEL_ROUTER` | `english` | Router checkpoint nickname |
| `LAYA_MODEL_ID` | — | If set, preload only this HF id / nickname |
| `USE_TF` | `0` | Avoid TF import deadlock |

## Fallbacks

| Path | On Laya miss |
|------|----------------|
| Query router | Heuristic `classify()` only (never agent CLI) |
| Decide / conflict / resolve | Silent Claude / existing heuristic |

Teacher logs: `~/.statefulmemory/laya_train/*.jsonl`.

## Fine-tune (later)

Collect teacher JSONL while Claude still backs decide/conflict. Use Convai’s
Kaggle notebook `laya_finetune_typed_decisions_2xT4_kaggle.ipynb`, then set
`LAYA_MODEL_ID` to your checkpoint. Not required for day-1 ship.
