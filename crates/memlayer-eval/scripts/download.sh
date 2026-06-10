#!/usr/bin/env bash
# Download benchmark datasets into data/
# Run from crates/memlayer-eval/
set -euo pipefail

DATA_DIR="$(cd "$(dirname "$0")/.." && pwd)/data"
mkdir -p "$DATA_DIR"

echo "=== Downloading LoCoMo ==="
LOCOMO_DIR="$DATA_DIR/locomo"
mkdir -p "$LOCOMO_DIR"
if [ ! -f "$LOCOMO_DIR/locomo10_test.json" ]; then
    # Clone sparse (only the data file we need)
    TMP=$(mktemp -d)
    git clone --depth=1 --filter=blob:none --sparse \
        https://github.com/snap-research/locomo.git "$TMP/locomo" 2>/dev/null
    cd "$TMP/locomo"
    git sparse-checkout set data
    cp data/locomo10_test.json "$LOCOMO_DIR/"
    cd - > /dev/null
    rm -rf "$TMP"
    echo "LoCoMo: $LOCOMO_DIR/locomo10_test.json"
else
    echo "LoCoMo already present, skipping."
fi

echo ""
echo "=== Downloading LongMemEval ==="
LME_DIR="$DATA_DIR/longmemeval"
mkdir -p "$LME_DIR"
if [ ! -f "$LME_DIR/questions.jsonl" ]; then
    TMP=$(mktemp -d)
    git clone --depth=1 --filter=blob:none --sparse \
        https://github.com/xiaowu0162/LongMemEval.git "$TMP/lme" 2>/dev/null
    cd "$TMP/lme"
    git sparse-checkout set data
    # LongMemEval ships data under data/ — copy what we need
    cp data/questions.jsonl "$LME_DIR/" 2>/dev/null || \
        find data -name "*.jsonl" -exec cp {} "$LME_DIR/" \;
    cp data/sessions.json "$LME_DIR/" 2>/dev/null || \
        find data -name "sessions.json" -exec cp {} "$LME_DIR/" \;
    cd - > /dev/null
    rm -rf "$TMP"
    echo "LongMemEval: $LME_DIR/"
else
    echo "LongMemEval already present, skipping."
fi

echo ""
echo "All datasets ready. Run:"
echo "  cargo run --release --bin eval -- run --benchmark locomo --limit 20 --out reports/smoke.md"
