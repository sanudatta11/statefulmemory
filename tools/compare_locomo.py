#!/usr/bin/env python3
"""Compare a memlayer LoCoMo scorecard against published baseline bands.

Scorecards come from `memlayer eval --save-scorecard`. This script never
invents a memlayer score; it only prints the JSON you pass in next to the
tracked paper / LLM-judge / retrieval notes.
"""

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


def infer_protocol(card: dict) -> str:
    queries = int(card.get("total_queries") or 0)
    if queries <= 5:
        return "smoke-or-tiny-slice"
    return "full-or-large-slice"


def fmt_pct(value) -> str:
    if value is None:
        return "n/a"
    return f"{float(value):.2f}"


def fmt_ratio(value) -> str:
    if value is None:
        return "n/a"
    return f"{float(value):.4f}"


def render(card: dict, baselines: dict) -> str:
    protocol = infer_protocol(card)
    paper = baselines.get("metrics", {}).get("paper_token_f1", {})
    judge = baselines.get("metrics", {}).get("llm_judge_accuracy", {})
    recall = baselines.get("metrics", {}).get("retrieval_recall", {})
    how = baselines.get("how_to_beat_or_compare", [])
    ml = baselines.get("memlayer_scorecard", {})

    f1 = card.get("f1_score")
    token_f1 = card.get("token_f1")
    f1_note = "n/a (not emitted in scorecard 2.0)"
    if f1 is not None:
        f1_note = f"{fmt_pct(f1)}  (legacy field; not paper token F1)"
    elif token_f1 is not None:
        f1_note = f"{fmt_pct(token_f1)}  (token_f1)"

    lines = [
        "=== LoCoMo local analysis ===",
        f"scorecard benchmark:  {card.get('benchmark', '?')}",
        f"scorecard_version:    {card.get('scorecard_version', '?')}",
        f"commit:               {card.get('commit_hash', '?')}",
        f"queries:              {card.get('total_queries', '?')}",
        f"correct:              {card.get('correct', '?')}",
        f"accuracy_pct:         {fmt_pct(card.get('accuracy_pct'))}  (memlayer judge / lexical pass rate)",
        f"recall_at_k:          {fmt_ratio(card.get('recall_at_k'))}",
        f"mrr:                  {fmt_ratio(card.get('mrr'))}",
        f"f1 / token_f1:        {f1_note}",
        f"inferred protocol:    {protocol}",
        "",
        "--- Published reference (not a substitute for a matched harness) ---",
        f"paper token F1 human:           {fmt_pct(paper.get('human_overall'))}",
        f"paper token F1 GPT-4-turbo:     {fmt_pct(paper.get('gpt4_turbo_128k_overall'))}",
        f"LLM-judge typical band:         {judge.get('typical_published_range', '?')}%",
        f"LLM-judge high-end (drift):     {fmt_pct(judge.get('high_end_with_protocol_drift'))}%",
        f"strong retrieval R@5 (typical): {fmt_pct(recall.get('typical_strong_r_at_5'))}%",
        "",
        "memlayer smoke: " + ml.get("smoke", ""),
        "memlayer full:  " + ml.get("full", ""),
        "",
        "How to compare / beat:",
    ]
    for item in how:
        lines.append(f"  - {item}")

    by_category = card.get("by_category") or {}
    if by_category:
        lines.extend(["", "By category:"])
        for name in sorted(by_category.keys()):
            stats = by_category[name] or {}
            lines.append(
                f"  {name}: {fmt_pct(stats.get('accuracy_pct'))}% "
                f"({stats.get('correct', '?')}/{stats.get('total', '?')})"
            )

    if protocol == "smoke-or-tiny-slice":
        lines.extend(
            [
                "",
                "WARNING: this scorecard looks like smoke or a tiny slice.",
                "Do not compare accuracy_pct to paper F1 or to published locomo10 leaderboards.",
            ]
        )
    else:
        acc = card.get("accuracy_pct")
        band = judge.get("typical_published_range") or [67, 80]
        if isinstance(acc, (int, float)) and len(band) == 2:
            lo, hi = float(band[0]), float(band[1])
            if acc < lo:
                lines.append(
                    f"\nLocal accuracy_pct ({acc:.1f}) is below the typical LLM-judge band "
                    f"({lo:.0f}–{hi:.0f}). Check retrieval (hybrid+rerank), evidence window, "
                    "and that the answer/judge model matches a published protocol."
                )
            elif acc <= hi:
                lines.append(
                    f"\nLocal accuracy_pct ({acc:.1f}) sits inside the typical LLM-judge band "
                    f"({lo:.0f}–{hi:.0f}). Still confirm judge model and category filter before claiming a win."
                )
            else:
                lines.append(
                    f"\nLocal accuracy_pct ({acc:.1f}) is above the typical LLM-judge band "
                    f"({lo:.0f}–{hi:.0f}). Disclose judge model, k, and adversarial inclusion "
                    "before comparing to a named leaderboard run."
                )

    paper_f1 = paper.get("gpt4_turbo_128k_overall")
    acc = card.get("accuracy_pct")
    if isinstance(acc, (int, float)) and isinstance(paper_f1, (int, float)):
        lines.append(
            f"\nPaper GPT-4-turbo token F1 is {paper_f1}. memlayer accuracy_pct is a different "
            "metric — a higher accuracy_pct does not mean you beat 51.6 F1."
        )

    return "\n".join(lines) + "\n"


def self_check() -> int:
    import tempfile

    baselines = json.loads(Path(__file__).resolve().parents[1].joinpath(
        "crates/memlayer-eval/baselines/locomo.json"
    ).read_text(encoding="utf-8"))
    smoke = {
        "scorecard_version": "2.0",
        "benchmark": "locomo",
        "commit_hash": "test",
        "total_queries": 1,
        "correct": 1,
        "accuracy_pct": 100.0,
        "recall_at_k": 1.0,
        "mrr": 1.0,
        "by_category": {"single_hop": {"total": 1, "correct": 1, "accuracy_pct": 100.0}},
        "token_f1": None,
    }
    full = {
        "scorecard_version": "2.0",
        "benchmark": "locomo",
        "commit_hash": "test",
        "total_queries": 1540,
        "correct": 1100,
        "accuracy_pct": 71.4,
        "recall_at_k": 0.91,
        "mrr": 0.72,
        "by_category": {
            "multi_hop": {"total": 100, "correct": 60, "accuracy_pct": 60.0},
        },
    }
    # Legacy v1 shape without f1 must still render.
    legacy_no_f1 = {
        "benchmark": "locomo",
        "commit_hash": "test",
        "total_queries": 10,
        "correct": 7,
        "accuracy_pct": 70.0,
    }
    smoke_out = render(smoke, baselines)
    full_out = render(full, baselines)
    legacy_out = render(legacy_no_f1, baselines)
    assert "WARNING" in smoke_out
    assert "recall_at_k" in smoke_out
    assert "By category:" in smoke_out
    assert "inside the typical LLM-judge band" in full_out
    assert "n/a (not emitted" in legacy_out
    with tempfile.TemporaryDirectory() as tmp:
        p = Path(tmp) / "card.json"
        p.write_text(json.dumps(smoke), encoding="utf-8")
        loaded = load_json(p)
        assert loaded["total_queries"] == 1
    print("self-check ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scorecard", type=Path, help="memlayer --save-scorecard JSON")
    parser.add_argument(
        "--baselines",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "crates/memlayer-eval/baselines/locomo.json",
        help="tracked published-reference JSON",
    )
    parser.add_argument(
        "--self-check",
        action="store_true",
        help="run a tiny fixture test and exit",
    )
    args = parser.parse_args()

    if args.self_check:
        return self_check()

    if args.scorecard is None:
        parser.error("--scorecard is required (or pass --self-check)")

    card = load_json(args.scorecard)
    baselines = load_json(args.baselines)
    sys.stdout.write(render(card, baselines))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
