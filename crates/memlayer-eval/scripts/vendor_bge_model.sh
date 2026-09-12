#!/usr/bin/env bash
# Vendor BGE-small-en-v1.5 model files into a local directory so memlayer-embed
# can bypass HuggingFace Hub. Required when corporate TLS interception breaks
# hf-hub's bundled rustls root chain (the symptom is `UnknownIssuer` on first
# embedding call).
#
# Usage:
#   ./scripts/vendor_bge_model.sh                # writes to ~/.memlayer-models/bge-small
#   MEMLAYER_BGE_MODEL_DIR=/some/path ./scripts/vendor_bge_model.sh
#
# Then point the eval binary at it:
#   export MEMLAYER_BGE_MODEL_DIR=~/.memlayer-models/bge-small
#   cargo run --release --bin eval -- run --benchmark locomo --mode hybrid ...
#
# curl honours macOS keychain via Secure Transport when built with --with-secure-transport,
# which is the macOS system curl. If you've installed a corp curl via brew that uses
# OpenSSL, set CURL_CA_BUNDLE to your corporate cert bundle first.

set -euo pipefail

DEST="${MEMLAYER_BGE_MODEL_DIR:-$HOME/.memlayer-models/bge-small}"
BASE="https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main"
# QUIET=1 when invoked from Make (env already exported for child recipes).
QUIET="${QUIET:-0}"

mkdir -p "$DEST"
if [ "$QUIET" != "1" ]; then
    echo "=== Vendoring BAAI/bge-small-en-v1.5 to $DEST ==="
fi

NEED_DOWNLOAD=0
for FNAME in config.json tokenizer.json model.safetensors; do
    OUT="$DEST/$FNAME"
    if [ -f "$OUT" ]; then
        if [ "$QUIET" != "1" ]; then
            echo "  $FNAME already present, skipping"
        fi
        continue
    fi
    NEED_DOWNLOAD=1
    echo "  downloading $FNAME ..."
    curl -L --progress-bar -o "$OUT" "$BASE/$FNAME"
done

if [ "$QUIET" = "1" ]; then
    if [ "$NEED_DOWNLOAD" = "1" ]; then
        echo "BGE model ready at $DEST"
    else
        echo "BGE model already present at $DEST"
    fi
else
    echo ""
    echo "Done. For a one-off shell session:"
    echo "  export MEMLAYER_BGE_MODEL_DIR=$DEST"
    echo "Make eval targets export this automatically."
fi
