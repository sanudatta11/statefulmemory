#!/usr/bin/env python3
"""StatefulMemory Laya System-1 sidecar.

Exposes POST /v1/predict and GET /health. Loads typed-decisions + english
checkpoints when available; soft-fails to HTTP 503 if laya is missing.
"""

from __future__ import annotations

import os
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


def _init_router() -> None:
    global _router, _loaded, _load_error
    if _router is not None or _load_error is not None:
        return
    try:
        from laya import Router

        r = Router()
        models: list[str] = []
        decide_id = os.environ.get("LAYA_MODEL_DECIDE", "typed-decisions")
        router_id = os.environ.get("LAYA_MODEL_ROUTER", "english")
        # Prefer HF ids / nicknames the Router understands.
        for name in (decide_id, router_id):
            if name and name not in models:
                models.append(name)
        # Also accept a full HF id override.
        if mid := os.environ.get("LAYA_MODEL_ID"):
            models = [mid]
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
    t0 = time.perf_counter()
    try:
        # Router.predict(state, questions, model=...)
        raw = _router.predict(state, req.questions, model=model)
    except TypeError:
        # Older signature without model kwarg — load agent directly.
        try:
            import laya

            agent = laya.load(
                "convaiinnovations/laya-typed-decisions"
                if model in ("typed-decisions", "typed_decisions")
                else "convaiinnovations/laya"
            )
            raw = agent.predict(state, req.questions)
        except Exception as e:  # noqa: BLE001
            raise HTTPException(status_code=500, detail=str(e)) from e
    except Exception as e:  # noqa: BLE001
        raise HTTPException(status_code=500, detail=str(e)) from e

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
