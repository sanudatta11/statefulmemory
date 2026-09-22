#!/usr/bin/env python3
"""StatefulMemory Laya System-1 sidecar.

Exposes POST /v1/predict and GET /health. Loads typed-decisions + english
checkpoints when available; soft-fails to HTTP 503 if laya is missing.
"""

from __future__ import annotations

import os
import sys
import time
from typing import Any

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

# Avoid TensorFlow import deadlocks during model load (per Laya docs).
os.environ.setdefault("USE_TF", "0")

app = FastAPI(title="statefulmemory-laya-sidecar", version="0.1.0")

_router = None
_loaded: list[str] = []
_load_error: str | None = None
_backend: str | None = None

# Nickname → pre-converted MLX checkpoint (independent MLX port, Apache-2.0).
_MLX_IDS = {
    "typed-decisions": "aac6fef/laya-typed-decisions-mlx",
    "typed_decisions": "aac6fef/laya-typed-decisions-mlx",
    "english": "aac6fef/laya-mlx",
    "multilingual": "aac6fef/laya-multilingual-mlx",
}


def _backend_want() -> str:
    """Configured backend: `auto` (default) | `mlx` | `torch`."""
    v = os.environ.get("LAYA_BACKEND", "auto").strip().lower()
    return v if v in ("auto", "mlx", "torch") else "auto"


def _resolve_backend() -> tuple[str, Any]:
    """Pick and import the Laya runtime module.

    Returns (backend, module). `auto` prefers `laya_mlx` on Apple Silicon
    (macOS 14+, Python ≥ 3.11) and falls back to the PyTorch `laya` package.
    A forced backend (`mlx` / `torch`) raises on ImportError so `/health`
    reports the misconfiguration instead of silently swapping runtimes.
    """
    want = _backend_want()
    if want == "mlx":
        import laya_mlx as mod  # noqa: PLC0415

        return "mlx", mod
    if want == "torch":
        import laya as mod  # noqa: PLC0415

        return "torch", mod
    # auto
    if sys.platform == "darwin" and sys.version_info >= (3, 11):
        try:
            import laya_mlx as mod  # noqa: PLC0415

            return "mlx", mod
        except ImportError:
            pass
    import laya as mod  # noqa: PLC0415

    return "torch", mod


def _mlx_id(name: str) -> str:
    """Map a model nickname / HF id to a pre-converted MLX checkpoint when known."""
    return _MLX_IDS.get(name, name)


def _adapt_questions(
    questions: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, tuple[int, float, float]]]:
    """Translate daemon question schemas to the Laya runtime schema.

    Daemon (`laya_schemas.rs`) sends `question` / `options` / score `min`+`max`;
    both `laya` and `laya_mlx` require `instructions` + `criteria` (choice
    options / score legend labels). Returns (adapted, score_meta) where
    score_meta[qid] = (denominator, lo, hi) to rescale the expected-index score
    output back into [lo, hi].
    """
    out: dict[str, Any] = {}
    meta: dict[str, tuple[int, float, float]] = {}
    for qid, q in questions.items():
        if not isinstance(q, dict):
            out[qid] = q
            continue
        a = dict(q)
        if "instructions" not in a and "question" in a:
            a["instructions"] = a.pop("question")
        kind = a.get("type")
        if kind == "choice" and "criteria" not in a and "options" in a:
            a["criteria"] = a.pop("options")
        else:
            a.pop("question", None)
        if kind == "score" and "criteria" not in a:
            lo = float(a.get("min", 0.0))
            hi = float(a.get("max", 1.0))
            n = 11
            step = (hi - lo) / (n - 1)
            a["criteria"] = [f"{lo + step * i:g}" for i in range(n)]
            meta[qid] = (n - 1, lo, hi)
        out[qid] = a
    return out, meta


def _rescale_scores(raw: Any, meta: dict[str, tuple[int, float, float]]) -> None:
    """Rewrite score answers from expected-index to the daemon's [lo, hi] range."""
    if not meta or not isinstance(raw, dict):
        return
    answers = raw.get("answers")
    if not isinstance(answers, dict):
        return
    for qid, (denom, lo, hi) in meta.items():
        ans = answers.get(qid)
        if isinstance(ans, dict) and "score" in ans:
            try:
                ans["value"] = float(ans["score"]) / denom * (hi - lo) + lo
            except (TypeError, ValueError):
                pass


def _init_router() -> None:
    global _router, _loaded, _load_error, _backend
    if _router is not None or _load_error is not None:
        return
    try:
        backend, laya = _resolve_backend()
        _backend = backend

        r = laya.Router()
        models: list[str] = []
        decide_id = os.environ.get("LAYA_MODEL_DECIDE", "typed-decisions")
        router_id = os.environ.get("LAYA_MODEL_ROUTER", "english")
        for name in (decide_id, router_id):
            if name and name not in models:
                models.append(name)
        # Also accept a full HF id override.
        if mid := os.environ.get("LAYA_MODEL_ID"):
            models = [mid]
        # Both runtimes accept the upstream nicknames / HF ids as-is
        # (mlx Router presets: english, multilingual, typed-decisions).
        try:
            r.preload(models)
            _loaded = list(models)
        except Exception as e:  # noqa: BLE001 — soft-load
            # Fall back to on-demand load via predict(model=...).
            _loaded = []
            _load_error = f"preload soft-failed: {e}"
        _router = r
    except Exception as e:  # noqa: BLE001
        _load_error = str(e)
        _router = None


class PredictRequest(BaseModel):
    state: str | dict[str, Any] = ""
    questions: dict[str, Any] = Field(default_factory=dict)
    model: str | None = None


class AnswerOut(BaseModel):
    kind: str
    value: Any
    confidence: float | None = None


class PredictResponse(BaseModel):
    answers: dict[str, AnswerOut]
    latency_ms: float
    model: str | None = None


def _normalize_answers(raw: Any) -> dict[str, AnswerOut]:
    """Map Laya predict output into a stable JSON shape for Rust."""
    out: dict[str, AnswerOut] = {}
    if raw is None:
        return out

    # dict of name → answer object / scalar
    if isinstance(raw, dict):
        # Some versions nest under "answers"
        payload = raw.get("answers", raw)
        if not isinstance(payload, dict):
            return out
        for name, ans in payload.items():
            out[name] = _one_answer(ans)
        return out

    # object with attributes
    answers = getattr(raw, "answers", None)
    if answers is None and hasattr(raw, "items"):
        try:
            for name, ans in raw.items():  # type: ignore[attr-defined]
                out[str(name)] = _one_answer(ans)
            return out
        except Exception:  # noqa: BLE001
            pass
    if isinstance(answers, dict):
        for name, ans in answers.items():
            out[str(name)] = _one_answer(ans)
        return out
    if answers is not None and hasattr(answers, "items"):
        for name, ans in answers.items():  # type: ignore[attr-defined]
            out[str(name)] = _one_answer(ans)
    return out


def _one_answer(ans: Any) -> AnswerOut:
    if isinstance(ans, dict):
        kind = str(ans.get("type") or ans.get("kind") or "choice")
        value = ans.get("value", ans.get("choice", ans.get("label")))
        if value is None:
            value = ans.get("score")
        if value is None:
            value = ans.get("noul")
        conf = ans.get("confidence", ans.get("prob", ans.get("p")))
        if value is None and "argmax" in ans:
            value = ans["argmax"]
        return AnswerOut(
            kind=kind,
            value=value,
            confidence=float(conf) if conf is not None else None,
        )
    # Attribute-style result
    kind = str(getattr(ans, "type", None) or getattr(ans, "kind", None) or "choice")
    value = getattr(ans, "value", None)
    if value is None:
        value = getattr(ans, "choice", None)
    if value is None:
        value = getattr(ans, "label", None)
    if value is None:
        value = getattr(ans, "score", None)
    if value is None:
        value = getattr(ans, "noul", None)
    conf = getattr(ans, "confidence", None)
    if conf is None:
        conf = getattr(ans, "prob", None)
    return AnswerOut(
        kind=kind,
        value=value,
        confidence=float(conf) if conf is not None else None,
    )


@app.on_event("startup")
def startup() -> None:
    _init_router()


@app.get("/health")
def health() -> dict[str, Any]:
    _init_router()
    ok = _router is not None
    return {
        "ok": ok,
        "backend": _backend,
        "loaded": _loaded,
        "error": _load_error,
    }


@app.post("/v1/predict", response_model=PredictResponse)
def predict(req: PredictRequest) -> PredictResponse:
    _init_router()
    if _router is None:
        raise HTTPException(status_code=503, detail=_load_error or "laya unavailable")

    state = req.state
    if isinstance(state, dict):
        # Laya accepts str or structured state; stringify dict for portability.
        import json

        state = json.dumps(state, ensure_ascii=False)

    model = req.model or os.environ.get("LAYA_MODEL_DECIDE", "typed-decisions")
    qdefs, score_meta = _adapt_questions(req.questions)
    t0 = time.perf_counter()
    try:
        # Router.predict(state, questions, model=...) — accepts nicknames
        # ("typed-decisions") and HF ids on both backends.
        raw = _router.predict(state, qdefs, model=model)
    except TypeError:
        # Older signature without model kwarg — load agent directly.
        try:
            _, laya = _resolve_backend()
            typed = model in ("typed-decisions", "typed_decisions")
            model_id = (
                _mlx_id("typed-decisions" if typed else "english")
                if _backend == "mlx"
                else "convaiinnovations/laya-typed-decisions"
                if typed
                else "convaiinnovations/laya"
            )
            agent = laya.load(model_id)
            raw = agent.predict(state, qdefs)
        except Exception as e:  # noqa: BLE001
            raise HTTPException(status_code=500, detail=str(e)) from e
    except Exception as e:
        # Custom HF id (contains "/") — Router presets reject it; load directly.
        if "/" in model:
            try:
                _, laya = _resolve_backend()
                agent = laya.load(model)
                raw = agent.predict(state, qdefs)
            except Exception as e2:  # noqa: BLE001
                raise HTTPException(status_code=500, detail=str(e2)) from e2
        else:
            raise HTTPException(status_code=500, detail=str(e)) from e

    _rescale_scores(raw, score_meta)
    latency_ms = (time.perf_counter() - t0) * 1000.0
    return PredictResponse(
        answers=_normalize_answers(raw),
        latency_ms=latency_ms,
        model=model,
    )


if __name__ == "__main__":
    import uvicorn

    host = os.environ.get("LAYA_HOST", "127.0.0.1")
    port = int(os.environ.get("LAYA_PORT", "8765"))
    uvicorn.run("server:app", host=host, port=port, reload=False)
