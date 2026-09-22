---
title: Laya System-1
description: Local, on-device System-1 decisions for query routing, Decide, and conflict judging — native MLX on Apple Silicon, PyTorch elsewhere.
---

**Laya System-1** is an optional HTTP sidecar that answers discrete typed
questions for the daemon — which retrieval tier a query needs, what a Decide
should recommend, and whether two memories conflict. It is **not** a
reranker and it is **not** in the search/context hot path. It is a local
decisioning layer: one non-autoregressive forward pass instead of an LLM
round-trip.

Laya ships by default with `smem install`. Skip it with `--no-laya`.

<video class="ml-demo-video" controls playsinline preload="metadata" src="/videos/laya-mlx-v1.mp4" poster="/videos/laya-mlx-v1-poster.jpg" title="Laya System-1 sidecar demo">
  Your browser does not support video.
</video>

## Why MLX

The sidecar runs the same Laya checkpoints (`typed-decisions`, `english`
router) on whichever runtime fits your hardware:

| backend | runtime | when |
|---|---|---|
| `auto` (default) | `laya-mlx` on Apple Silicon (macOS 14+, Python ≥ 3.11); PyTorch `laya` elsewhere | best default |
| `mlx` | [`laya-mlx`](https://github.com/mizorewww/laya-mlx) — native MLX port of the same checkpoints, no PyTorch | Mac self-host; published M3 Max P50 **13.4 ms** (421M) / **7.4 ms** (multilingual) |
| `torch` | PyTorch / Transformers `laya` package | Linux / CUDA servers |

`requirements.txt` installs both runtimes behind platform markers, so `auto`
just works after `smem install`. A forced backend that is missing fails loudly
in `/health` instead of silently swapping runtimes.

**Timeout guidance:** the daemon default `timeout_ms = 80` fits the
single-question router / conflict calls (~30 ms on MLX) but drops decide /
resolve batches (~100–200 ms for 2–3 questions). Set `timeout_ms = 250`
(or `STATEFULMEMORY_LAYA_TIMEOUT_MS=250`) for decide / conflict via Laya.

## Install

```bash
smem install                 # or: statefulmemory install / sm install
smem doctor                  # prints laya enabled / url / health
curl -s http://127.0.0.1:8765/health
# { "ok": true, "backend": "mlx", "loaded": [...], "error": null }
```

Needs **Python 3.10+** on `PATH` (MLX wants 3.11+). The first model download
can take a few minutes; the daemon soft-retries until `/health` is ok.
Loopback only for auto-spawn — set `[laya] url` to a shared host for teams.

## What it decides

Three call sites share one model, exposed through three typed surfaces:

- **Query router** — Easy / Normal / Hard. Easy skips dense; Hard expands +
  graph. Fallback is heuristic-only.
- **Decide** — recommends a side from retrieved evidence, with an abstain
  guard when evidence is thin. Fallback is the agent CLI.
- **Conflict judge** — `SUPERSEDES` / keep-new verdicts for supersession.
  Fallback is the agent CLI or heuristic supersession.

Every decision returns a confidence score; `min_confidence = 0.35` sits below
the acceptance tick, so the daemon can soft-fail to the fallback instead of
acting on a weak call.

## Config

```toml
# ~/.statefulmemory/config.toml
[laya]
enabled = true
url = "http://127.0.0.1:8765"   # or http://team-host:8765
timeout_ms = 250                # decide/conflict batches need > 80 ms
backend = "auto"                # auto | mlx | torch
```

Env overrides: `STATEFULMEMORY_LAYA_ENABLED`, `STATEFULMEMORY_LAYA_URL`,
`STATEFULMEMORY_LAYA_TIMEOUT_MS`, `STATEFULMEMORY_LAYA_ROUTER`,
`STATEFULMEMORY_LAYA_DECIDE`, `STATEFULMEMORY_LAYA_CONFLICT`,
`STATEFULMEMORY_LAYA_MIN_CONFIDENCE`, `STATEFULMEMORY_LAYA_BACKEND`.

All three backends run the same checkpoints with identical typed output
schemas — the choice is purely about hardware.

## Fallback behavior

Search and context stay fully local (BM25 + BGE + local CE) whether or not
Laya is running. With Laya off:

- Router falls back to heuristics.
- Decide / conflict fall back to the agent CLI (or heuristic supersession).

So Laya is strictly an upgrade to latency and privacy, never a hard
dependency. See [Config](/docs/config/) for the full knob list and
[Decide](/docs/decide/) for the decision surface it feeds.

## Repo notes

The sidecar lives in `tools/laya-sidecar/` (FastAPI server, run.sh, venv
setup). Fine-tuning your own checkpoint is Wave 4b, tracked in
`tools/laya-sidecar/FINETUNE.md`. Laya weights are Apache-2.0
(`convaiinnovations/laya`); the `laya-mlx` port is an independent Apache-2.0
implementation of the same checkpoints.