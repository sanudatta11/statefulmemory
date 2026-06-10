#!/usr/bin/env bash
# Download benchmark datasets into data/
# Run from crates/memlayer-eval/
set -euo pipefail

DATA_DIR="$(cd "$(dirname "$0")/.." && pwd)/data"
mkdir -p "$DATA_DIR"

echo "=== Downloading LoCoMo ==="
LOCOMO_DIR="$DATA_DIR/locomo"
mkdir -p "$LOCOMO_DIR"
if [ ! -f "$LOCOMO_DIR/locomo10.json" ]; then
    TMP=$(mktemp -d)
    git clone --depth=1 https://github.com/snap-research/locomo.git "$TMP/locomo" 2>/dev/null
    FOUND=$(find "$TMP/locomo" -name "locomo10.json" | head -1)
    if [ -z "$FOUND" ]; then
        echo "locomo10.json not found; listing repo contents:"
        find "$TMP/locomo" -name "*.json" | head -20
        echo "Please copy the dataset file manually to $LOCOMO_DIR/locomo10.json"
    else
        cp "$FOUND" "$LOCOMO_DIR/locomo10.json"
        echo "LoCoMo: $LOCOMO_DIR/locomo10.json"
    fi
    rm -rf "$TMP"
else
    echo "LoCoMo already present, skipping."
fi

echo ""
echo "=== Downloading LongMemEval ==="
LME_DIR="$DATA_DIR/longmemeval"
mkdir -p "$LME_DIR"

# Files are hosted on HuggingFace datasets repo.
# We use the s_cleaned (single-session) file as the default eval target.
HF_BASE="https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main"

for FNAME in longmemeval_s_cleaned.json longmemeval_m_cleaned.json longmemeval_oracle.json; do
    DEST="$LME_DIR/$FNAME"
    if [ ! -f "$DEST" ]; then
        echo "Downloading $FNAME ..."
        if command -v wget &>/dev/null; then
            wget -q --show-progress -O "$DEST" "$HF_BASE/$FNAME"
        else
            curl -L --progress-bar -o "$DEST" "$HF_BASE/$FNAME"
        fi
        echo "  -> $DEST"
    else
        echo "$FNAME already present, skipping."
    fi
done

echo ""
echo "All datasets ready. Run:"
echo "  cargo run --release --bin eval -- run --benchmark locomo --limit 20 --out reports/smoke.md"
