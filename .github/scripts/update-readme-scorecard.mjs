#!/usr/bin/env node
// Update the README scorecard section from a scorecard.json produced by
// `statefulmemory eval --save-scorecard <file>`.
//
// Usage: node update-readme-scorecard.mjs <scorecard.json> [README.md]
//
// Looks for <!-- scorecard-start --> / <!-- scorecard-end --> markers and
// replaces the content between them.  If the markers are missing the script
// exits 0 (idempotent no-op).
//
// Renders a Mem0-style table: overall + per-category accuracy (single-hop,
// multi-hop, temporal, open-domain), retrieval R@k / MRR, latency, judge
// model disclosure. Peer numbers are cited with an unmatched-harness caveat —
// never as a leaderboard claim.

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

const pct = (v) => (v != null && !Number.isNaN(Number(v)) ? `${Number(v).toFixed(1)}%` : 'n/a');
const num = (v) => (v != null && !Number.isNaN(Number(v)) ? Number(v).toFixed(4) : 'n/a');
const ms = (v) => (v != null && !Number.isNaN(Number(v)) ? `${Number(v).toFixed(1)} ms` : 'n/a');

const commit = (card.commit_hash ?? 'unknown').slice(0, 8);
const ts = card.timestamp
  ? new Date(card.timestamp).toISOString().slice(0, 10)
  : new Date().toISOString().slice(0, 10);

const cats = card.by_category ?? {};
const catPct = (name) => (cats[name] != null ? pct(cats[name].accuracy_pct) : 'n/a');
const catN = (name) => (cats[name] != null ? `${cats[name].total}q` : '');

// Judge disclosure: LLM pin when present (LLM-judged runs), else the card is
// retrieval-level (lexical / smoke fixture).
const judge = card.judge_model
  ? `LLM answer + judge \`${card.judge_model}\``
  : 'lexical judge (gold-in-retrieved-hits, no LLM)';

const adversarial = cats.adversarial;
const advNote = adversarial
  ? ` Adversarial slice reported separately (${pct(adversarial.accuracy_pct)}, n=${adversarial.total}) — excluded from peer comparisons.`
  : '';

const bench = card.benchmark ?? 'locomo';
const smokeNote =
  card.total_queries <= 2 && /bm25/i.test(bench)
    ? '**Smoke fixture — harness wiring check only. Not a LoCoMo score.**'
    : `Retrieval scorecard, not a public leaderboard claim.${advNote}`;

const block = `<!-- scorecard-start -->
> **Published LoCoMo scorecard** — last run \`${commit}\` (${ts}) · **${card.total_queries} queries** · ${judge} · retrieval \`${bench}\`
> ${smokeNote}

| Overall | Single-hop | Multi-hop | Temporal | Open-domain |
|---|---|---|---|---|
| **${pct(card.accuracy_pct)}** | ${catPct('single_hop')} | ${catPct('multi_hop')} | ${catPct('temporal')} | ${catPct('open_domain')} |

| R@k | MRR | Retrieval p50 | Retrieval p95 | E2E p50 | Mean tokens/q |
|---|---|---|---|---|---|
| ${num(card.recall_at_k)} | ${num(card.mrr)} | ${ms(card.retrieval_p50_ms)} | ${ms(card.retrieval_p95_ms)} | ${ms(card.end_to_end_p50_ms)} | ${card.mean_prompt_tokens != null ? Math.round(card.mean_prompt_tokens) : 'n/a'} |

<sub>Peer context, **unmatched harness** (different judge models, k, fact
extraction, question subsets — compare only among runs that disclose the same
protocol): Mem0 reports **92.5** overall on 1,540 LoCoMo questions
([mem0.ai/research](https://mem0.ai/research)); paper-era Mem0 ≈ **66.9**
([arXiv:2504.19413](https://arxiv.org/abs/2504.19413)); engram-lite ≈ **68.3**.
Slice sizes here: single-hop ${catN('single_hop') || '—'}, multi-hop ${catN('multi_hop') || '—'}, temporal ${catN('temporal') || '—'}, open-domain ${catN('open_domain') || '—'}.</sub>
<!-- scorecard-end -->`;

const out = src.slice(0, si) + block + src.slice(ei + END.length);
writeFileSync(readme, out);
console.log(`README scorecard updated (${commit}, ${ts})`);
