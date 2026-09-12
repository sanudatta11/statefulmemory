#!/usr/bin/env bash
# Preference-evolution demo — memory is the point.
#
# Walks: save decision A → context across a "new session" → supersede with B →
# optional decide / verify. Needs `memlayer` on PATH (make install).
#
# Usage: ./demos/preference-evolution.sh
set -euo pipefail

if ! command -v memlayer >/dev/null 2>&1; then
  echo "memlayer not on PATH. Run: make prereqs && make install && export PATH=\"\$HOME/.local/bin:\$PATH\"" >&2
  exit 1
fi

uuid() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen
  else
    python3 -c 'import uuid; print(uuid.uuid4())'
  fi
}

echo "== Day 1: prefer GORM =="
SID1=$(uuid)
memlayer obs save \
  --type decision \
  --title "ORM: use GORM" \
  --content "Team chose GORM for the Go service. Keep models in internal/models." \
  --session "$SID1"

echo
echo "== Later session: briefing still finds Day 1 =="
memlayer obs context --query "which ORM should we use?" --limit 5

echo
echo "== Day 20: supersede with pgx =="
SID2=$(uuid)
memlayer obs save \
  --type decision \
  --title "ORM: use pgx not GORM" \
  --content "Revisited: prefer raw SQL via pgx. GORM is superseded for new code." \
  --session "$SID2"

echo
echo "== Context after change =="
memlayer obs context --query "which ORM should we use?" --limit 5

echo
echo "== Decide (needs agent CLI on PATH) =="
if memlayer decide "Should new Go DB code use GORM or pgx?" 2>/dev/null; then
  :
else
  echo "(decide skipped or failed — install/wire an agent CLI, or ignore for CLI-only demo)"
fi

echo
echo "== Optional verify (no-op without anchors) =="
memlayer verify --quiet || true

echo
echo "Done. Inspect with: memlayer obs search ORM --limit 10"
echo "Anchors + verify: https://memlayer.org/docs/anchors-verify/"
