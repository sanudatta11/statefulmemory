#!/usr/bin/env python3
"""Compare staleness scorecards (default arm vs no-supersede baseline)."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def load_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        sys.stderr.write(f"error: file not found: {path}\n")
        sys.exit(2)
    except json.JSONDecodeError as exc:
        sys.stderr.write(f"error: invalid JSON in {path}: {exc}\n")
        sys.exit(2)


def pct(card: dict) -> float | None:
    v = card.get("superseded_served_pct")
    if v is None:
        return None
    return float(v)


def render(scorecard: dict, baseline: dict) -> str:
    s = pct(scorecard)
    b = pct(baseline)
    lines = [
        "=== Staleness local analysis ===",
        f"default superseded_served_pct:  {s if s is not None else 'n/a'}",
        f"baseline superseded_served_pct: {b if b is not None else 'n/a'}",
        f"default accuracy_pct:           {scorecard.get('accuracy_pct', 'n/a')}",
        f"baseline accuracy_pct:          {baseline.get('accuracy_pct', 'n/a')}",
        f"default recall_at_k:            {scorecard.get('recall_at_k', 'n/a')}",
        f"baseline recall_at_k:           {baseline.get('recall_at_k', 'n/a')}",
    ]
    if s is not None and b is not None:
        lines.append(f"delta (baseline - default):     {b - s:.2f} pp")
        if b > s:
            lines.append(
                "Baseline (add-only) serves superseded values more often — "
                "expected when supersession is working."
            )
        elif b < s:
            lines.append(
                "WARNING: default arm served more superseded values than baseline."
            )
        else:
            lines.append("Arms tied on superseded_served_pct.")
    return "\n".join(lines) + "\n"


def self_check() -> int:
    scorecard = {
        "superseded_served_pct": 0.0,
        "accuracy_pct": 100.0,
        "recall_at_k": 1.0,
    }
    baseline = {
        "superseded_served_pct": 40.0,
        "accuracy_pct": 60.0,
        "recall_at_k": 0.9,
    }
    out = render(scorecard, baseline)
    assert "40.00" in out or "40.0" in out
    assert "delta" in out
    assert "add-only" in out
    print("self-check ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scorecard", type=Path, help="default-arm scorecard JSON")
    parser.add_argument("--baseline", type=Path, help="--no-supersede scorecard JSON")
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if args.scorecard is None or args.baseline is None:
        parser.error("--scorecard and --baseline are required (or pass --self-check)")
    sys.stdout.write(render(load_json(args.scorecard), load_json(args.baseline)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
