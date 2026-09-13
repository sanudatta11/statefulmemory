#!/usr/bin/env node
// Eval scorecard regression gate (Stage 0b, spec: graph-briefing).
//
// Usage: node eval-gate.mjs <scorecard.json> <baseline.json>
//
// Behavior:
//   - Baseline missing  -> enforce a conservative first-run floor by copying
//     the card to the baseline path and failing loudly (so a reviewer notices
//     a baseline is being established, never silently).
//   - Baseline present   -> fail the PR when accuracy drops > 1.0 pt or
//     retrieval p95 blows past 80 ms (CI latency budget with generous margin
//     over the 50 ms production p95 target).
//   - Multi-hop slice    -> when the card carries `by_category.multi_hop`,
//     compare against the baseline's multi-hop slice (same ± 1.0 pt rule).
//     Once the graph gate (+5 pts) is exercised this is where it lands.

import { readFileSync, writeFileSync, copyFileSync } from 'node:fs';

const [, , cardPath, baselinePath] = process.argv;
if (!cardPath || !baselinePath) {
  console.error('usage: node eval-gate.mjs <scorecard.json> <baseline.json>');
  process.exit(2);
}

const readJson = (p) => JSON.parse(readFileSync(p, 'utf8'));

function exit(msg, code = 1) {
  console.error(`eval-gate: ${msg}`);
  // Establishing a baseline is intentionally fatal on first run — it forces
  // an explicit review commit of the baseline before gates go live.
  process.exit(code);
}

let baseline;
try {
  baseline = readJson(baselinePath);
} catch {
  copyFileSync(cardPath, baselinePath);
  writeFileSync(
    baselinePath,
    JSON.stringify({ ...readJson(cardPath), timestamp: 'BASELINE_SEED_' + Date.now() }, null, 2),
  );
  exit('no baseline present; seeded evaluator baseline, gate needs a review commit', 1);
}

const card = readJson(cardPath);

const ACCURACY_TOLERANCE_PT = 1.0;
const P95_BUDGET_MS = 80.0;

const failures = [];

const baseAcc = baseline.accuracy_pct ?? 0;
const cardAcc = card.accuracy_pct ?? 0;
if (cardAcc < baseAcc - ACCURACY_TOLERANCE_PT) {
  failures.push(`accuracy regression: ${cardAcc.toFixed(2)}% vs baseline ${baseAcc.toFixed(2)}% (> ${ACCURACY_TOLERANCE_PT} pt drop)`);
}

const baseP95 = baseline.retrieval_p95_ms ?? 0;
const cardP95 = card.retrieval_p95_ms ?? 0;
if (cardP95 > P95_BUDGET_MS) {
  failures.push(`retrieval p95 ${cardP95.toFixed(2)} ms exceeds CI budget ${P95_BUDGET_MS} ms (production target 50 ms)`);
}

const baseMulti = baseline.by_category?.multi_hop ?? null;
const cardMulti = card.by_category?.multi_hop ?? null;
if (baseMulti !== null && cardMulti !== null) {
  const bm = Number(baseMulti) || 0;
  const cm = Number(cardMulti) || 0;
  if (cm < bm - ACCURACY_TOLERANCE_PT) {
    failures.push(`multi-hop regression: ${cm.toFixed(2)}% vs baseline ${bm.toFixed(2)}%`);
  }
}

if (failures.length > 0) {
  failures.forEach((f) => console.error(`  - ${f}`));
  exit('scorecard gate failed');
}

console.error('eval-gate: ok');
console.error(
  `  accuracy ${cardAcc.toFixed(2)}% (baseline ${baseAcc.toFixed(2)}%)\n` +
    `  retrieval p95 ${cardP95.toFixed(2)} ms (budget ${P95_BUDGET_MS} ms)\n` +
    `  multi-hop ${cardMulti !== null ? cardMulti.toFixed(2) : 'n/a'}% (baseline ${baseMulti !== null ? baseMulti.toFixed(2) : 'n/a'}%)`,
);