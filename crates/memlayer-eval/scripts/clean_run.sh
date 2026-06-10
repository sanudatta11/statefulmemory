#!/usr/bin/env bash
# Clean rebuild + smoke test for memlayer-eval.
# Run from crates/memlayer-eval/ (or anywhere; uses script-relative paths).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
EVAL_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKSPACE_DIR="$(cd "$EVAL_DIR/../.." && pwd)"

cd "$WORKSPACE_DIR"

echo "=== [1/4] Cleaning previous artifacts ==="
rm -rf "$EVAL_DIR/data/projects"
rm -rf "$EVAL_DIR/reports"
cargo clean -p memlayer-eval 2>/dev/null || true
echo "  cleaned: data/projects, reports/, target/release/memlayer-eval"

echo ""
echo "=== [2/4] Verifying datasets are present ==="
if [ ! -f "$EVAL_DIR/data/locomo/locomo10.json" ] \
|| [ ! -f "$EVAL_DIR/data/longmemeval/longmemeval_s_cleaned.json" ]; then
    echo "  datasets missing — running download.sh"
    "$SCRIPT_DIR/download.sh"
else
    echo "  LoCoMo + LongMemEval present"
fi

echo ""
echo "=== [3/4] Building memlayer-eval (release) ==="
cargo build --release -p memlayer-eval

echo ""
echo "=== [4/4] Running smoke test (locomo, 20 queries) ==="
mkdir -p "$EVAL_DIR/reports"
cd "$EVAL_DIR"
cargo run --release --bin eval -- \
    run --benchmark locomo --limit 20 --out reports/smoke.md

echo ""
echo "=== Done ==="
echo "  Markdown report: $EVAL_DIR/reports/smoke.md"
echo "  JSON report:     $EVAL_DIR/reports/smoke.json"
