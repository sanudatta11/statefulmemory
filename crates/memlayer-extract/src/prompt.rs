// Generated with AI Coding Rules Hub
//! Extraction prompt template + tolerant JSON parser (spec-task-14).
//!
//! This module owns two responsibilities for the P2 extraction pipeline:
//!
//! 1. [`build_extraction_prompt`] — assembles the Haiku prompt that turns a
//!    window of conversation [`Turn`]s into atomic facts. Fact-extract inspired:
//!    asks for `(subject, predicate, object)` plus an optional resolved
//!    `temporal` field and a forward-compat `entities` array (parsed in
//!    spec-task-19a, ignored here).
//! 2. [`parse_facts`] — robustly extracts a `Vec<Fact>` from whatever the
//!    model returns. Implements EH-4: prose-tolerant slicing, per-object
//!    repair, and graceful degradation to `Ok(empty)` on hopeless garbage.
//!
//! Out-of-range `evidence_turn_idx` values are logged and dropped (we never
//! crash because the model hallucinated a turn index).

use crate::{Fact, Turn};
use serde::Deserialize;

/// Wire-format fact as emitted by Haiku. Lifted into [`Fact`] by `parse_facts`
/// after resolving `evidence_turn_idx` against the prompted turn window.
#[derive(Debug, Deserialize)]
struct RawFact {
    subject: String,
    predicate: String,
    object: String,
    #[serde(default)]
    temporal: Option<String>,
    #[serde(default = "default_salience")]
    salience: f32,
    evidence_turn_idx: usize,
    // `entities` is documented in the prompt for spec-task-19a but we don't
    // parse it here — `#[serde(default)]` plus untyped catch-all isn't needed
    // because serde ignores unknown fields by default.
}

fn default_salience() -> f32 {
    1.0
}

/// Build the LLM extraction prompt for a window of consecutive turns.
///
/// `current_date` (optional, ISO `YYYY-MM-DD`) is forwarded to the model so
/// it can resolve relative phrases ("last Tuesday") into absolute dates in
/// the `temporal` field. Pass `None` for replay/eval scenarios where the
/// session date is unknown — the model will then leave temporal as-stated
/// or null.
pub fn build_extraction_prompt(turns: &[Turn], current_date: Option<&str>) -> String {
    let mut out = String::with_capacity(1024 + turns.len() * 128);

    out.push_str(
        "You extract atomic, durable facts from a short window of dialogue.\n\
         Output a single JSON array. No prose, no markdown fences, no commentary.\n\
         Each array element is an object with the following keys:\n\
         \n\
         - subject: string — the entity the fact is about.\n\
         - predicate: string — a short verb phrase (e.g. \"attended\", \"prefers\").\n\
         - object: string — the value/target of the predicate.\n\
         - temporal: string | null — ISO date (YYYY-MM-DD) when the fact is time-bound,\n\
           else null. Resolve relative dates against the current date below when given.\n\
         - salience: number in [0,1] — how durable / retrieval-worthy the fact is.\n\
         - evidence_turn_idx: integer — 1-based index of the numbered turn that\n\
           supports this fact.\n\
         - entities: array of strings — proper-noun entities mentioned (people,\n\
           places, organizations). Optional; may be empty. Used by the entity\n\
           index in a later task; safe to omit.\n\
         \n\
         Rules:\n\
         - Extract atomic facts only — split compound statements.\n\
         - Skip greetings, pleasantries, questions without a stated answer.\n\
         - Prefer the speaker as `subject` when the speaker is asserting about\n\
           themselves (e.g. \"I went to X\" -> subject = speaker).\n\
         - If a fact has no obvious time reference, set temporal to null.\n",
    );

    if let Some(date) = current_date {
        out.push_str("\nCurrent date: ");
        out.push_str(date);
        out.push('\n');
    }

    out.push_str("\nTurns:\n");
    for (i, turn) in turns.iter().enumerate() {
        // 1-based numbering matches `evidence_turn_idx` in the schema above.
        out.push_str(&format!("{}. {}: {}\n", i + 1, turn.speaker, turn.text));
    }

    out.push_str("\nRespond with the JSON array now.");
    out
}

/// Parse a (possibly noisy) Haiku response into [`Fact`]s. Implements EH-4:
///
/// 1. Slice between the first `[` and last `]` to strip prose.
/// 2. Try to parse the slice as `Vec<RawFact>` directly.
/// 3. On failure, walk the slice with a brace-counter and try each
///    top-level `{...}` block individually; collect successes.
/// 4. If no facts could be recovered, log a warning and return `Ok(vec![])`.
///
/// Facts whose `evidence_turn_idx` is out of range for `turns` are dropped
/// with a warning rather than failing the whole batch.
pub fn parse_facts(raw: &str, turns: &[Turn]) -> anyhow::Result<Vec<Fact>> {
    let slice = match (raw.find('['), raw.rfind(']')) {
        (Some(start), Some(end)) if end > start => &raw[start..=end],
        _ => {
            tracing::warn!(
                target: "memlayer_extract::parse_facts",
                "no JSON array brackets found; returning empty fact list"
            );
            return Ok(Vec::new());
        }
    };

    let raw_facts: Vec<RawFact> = match serde_json::from_str::<Vec<RawFact>>(slice) {
        Ok(v) => v,
        Err(_) => {
            // Per-object repair: scan for top-level `{...}` blocks and parse
            // each independently. This is what saves us when Haiku slips a
            // trailing comma, an unquoted token, or extra prose into the
            // middle of the array.
            let recovered = repair_objects(slice);
            if recovered.is_empty() {
                tracing::warn!(
                    target: "memlayer_extract::parse_facts",
                    "JSON array unparseable and no objects recovered; returning empty"
                );
            }
            recovered
        }
    };

    let mut out = Vec::with_capacity(raw_facts.len());
    for rf in raw_facts {
        if rf.evidence_turn_idx == 0 || rf.evidence_turn_idx > turns.len() {
            tracing::warn!(
                target: "memlayer_extract::parse_facts",
                idx = rf.evidence_turn_idx,
                window = turns.len(),
                "evidence_turn_idx out of range; dropping fact"
            );
            continue;
        }
        let turn = &turns[rf.evidence_turn_idx - 1];
        out.push(Fact {
            subject: rf.subject,
            predicate: rf.predicate,
            object: rf.object,
            temporal: rf.temporal,
            salience: rf.salience,
            evidence_obs_id: turn.obs_id,
            source_session: turn.session_id.clone(),
        });
    }
    Ok(out)
}

/// Walk `slice` looking for top-level `{...}` blocks (respecting strings and
/// escaping) and try to deserialize each as a `RawFact`. Successes are kept,
/// failures silently skipped.
fn repair_objects(slice: &str) -> Vec<RawFact> {
    let bytes = slice.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = find_matching_brace(bytes, i) {
                let candidate = &slice[i..=end];
                if let Ok(rf) = serde_json::from_str::<RawFact>(candidate) {
                    out.push(rf);
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Given `bytes[start] == b'{'`, return the byte index of the matching `}`,
/// honoring string literals (and their `\"` escapes). Returns `None` if the
/// braces never balance — the caller treats that as "no recoverable object".
fn find_matching_brace(bytes: &[u8], start: usize) -> Option<usize> {
    debug_assert_eq!(bytes[start], b'{');
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}
