# Laya System-1 sidecar (StatefulMemory Wave 4)

Optional HTTP sidecar that serves [Laya](https://huggingface.co/convaiinnovations/laya)
typed decisions for the daemon. Search/context stay Rust (BM25 + BGE + local CE);
this process only answers discrete System-1 questions (router tier, decide, conflict).

## Quick start (recommended)

`smem`, `sm`, and `statefulmemory` are the same binary. Either install command sets up
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
backend = "auto"                # auto | mlx | torch
```

Env overrides: `STATEFULMEMORY_LAYA_ENABLED`, `STATEFULMEMORY_LAYA_URL`,
`STATEFULMEMORY_LAYA_TIMEOUT_MS`, `STATEFULMEMORY_LAYA_ROUTER`,
`STATEFULMEMORY_LAYA_DECIDE`, `STATEFULMEMORY_LAYA_CONFLICT`,
`STATEFULMEMORY_LAYA_MIN_CONFIDENCE`, `STATEFULMEMORY_LAYA_BACKEND`.

### Backend: MLX on Apple Silicon, torch elsewhere

`[laya] backend` selects the inference runtime (daemon passes it to the
sidecar as `LAYA_BACKEND`):

| backend | runtime | when |
|---------|---------|------|
| `auto` (default) | `laya-mlx` on Apple Silicon (macOS 14+, Python ≥ 3.11); PyTorch `laya` otherwise | best default |
| `mlx` | [`laya-mlx`](https://github.com/mizorewww/laya-mlx) — native MLX port of the same Laya checkpoints (Apache-2.0, independent port, original weights preserved) | Mac self-host; published M3 Max P50 13.4 ms (421M) / 7.4 ms (multilingual), no PyTorch |
| `torch` | PyTorch / Transformers `laya` package | Linux / CUDA servers |

`requirements.txt` installs both runtimes with platform markers
(`laya-mlx` only on darwin / Python ≥ 3.11), so `auto` just works after
`smem install`. `/health` reports the active backend:

```bash
curl -s http://127.0.0.1:8765/health
# { "ok": true, "backend": "mlx", "loaded": [...], "error": null }
```

**Timeout guidance:** the daemon default `timeout_ms = 80` fits the
single-question router/conflict calls (~30 ms on MLX) but drops decide /
resolve (2–3 questions, ~100–200 ms). Set `timeout_ms = 250` (or
`STATEFULMEMORY_LAYA_TIMEOUT_MS=250`) when using decide/conflict via Laya.

MLX accepts the same question schema the daemon sends (`question` /
`options` / score `min`+`max` — `server.py` adapts to `instructions` /
`criteria`); a forced backend that is missing fails loudly in `/health`
instead of silently swapping runtimes.

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
