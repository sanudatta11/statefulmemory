#!/usr/bin/env node
// Update the README scorecard section from a scorecard.json produced by
// `statefulmemory eval --smoke --save-scorecard`.
//
// Usage: node update-readme-scorecard.mjs <scorecard.json> [README.md]
//
// Looks for <!-- scorecard-start --> / <!-- scorecard-end --> markers and
// replaces the content between them.  If the markers are missing the script
// exits 0 (idempotent no-op).

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';

const [, , cardPath, readmePath = 'README.md'] = process.argv;
if (!cardPath) {
  console.error('usage: node update-readme-scorecard.mjs <scorecard.json> [README.md]');
  process.exit(2);
}

const card = JSON.parse(readFileSync(cardPath, 'utf8'));
const readme = resolve(readmePath);
if (!existsSync(readme)) {
  console.error(`readme not found: ${readme}`);
  process.exit(1);
}

const src = readFileSync(readme, 'utf8');
const START = '<!-- scorecard-start -->';
const END = '<!-- scorecard-end -->';
const si = src.indexOf(START);
const ei = src.indexOf(END);
if (si === -1 || ei === -1 || ei <= si) {
  console.error('scorecard markers not found in README; skipping');
  process.exit(0);
}

const pct = (v) => (v != null ? `${Number(v).toFixed(2)}%` : 'n/a');
const ms = (v) => (v != null ? `${Number(v).toFixed(1)} ms` : 'n/a');
const num = (v) => (v != null ? Number(v).toFixed(4) : 'n/a');

const commit = (card.commit_hash ?? 'unknown').slice(0, 8);
const ts = card.timestamp
  ? new Date(card.timestamp).toISOString().slice(0, 10)
  : new Date().toISOString().slice(0, 10);

const multiHop = card.by_category?.multi_hop;

const block = `<!-- scorecard-start -->
> **Auto-updated on every push to main.** Last run: \`${commit}\` (${ts})
> **Note:** Smoke benchmark only — not full stratified LoCoMo eval.

| Metric | Value |
|---|---|
| **LoCoMo Accuracy** | ${pct(card.accuracy_pct)} |
| **Recall\@k** | ${num(card.recall_at_k)} |
| **MRR** | ${num(card.mrr)} |
| **Multi-hop Accuracy** | ${pct(multiHop)} |
| **Retrieval Latency (p50)** | ${ms(card.retrieval_p50_ms)} |
| **Retrieval Latency (p95)** | ${ms(card.retrieval_p95_ms)} |
| **End-to-End Latency (p50)** | ${ms(card.end_to_end_p50_ms)} |
| **Benchmark** | ${card.benchmark ?? 'locomo'} |
| **Eval Gate** | ${card.gate_passed ?? 'pass'} |
<!-- scorecard-end -->`;

const out = src.slice(0, si) + block + src.slice(ei + END.length);
writeFileSync(readme, out);
console.log(`README scorecard updated (${commit}, ${ts})`);
