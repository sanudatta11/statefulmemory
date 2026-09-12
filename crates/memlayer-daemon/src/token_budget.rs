//! Token budget packing and per-type round-robin quotas.

use memlayer_core::tokens::estimate_observation_tokens;
use memlayer_storage::Observation;

/// Round-robin across observation types with a per-type cap.
/// `max_per_type == 0` means unlimited (return `hits` unchanged).
pub fn apply_max_per_type(hits: Vec<Observation>, max_per_type: u32) -> Vec<Observation> {
    if max_per_type == 0 || hits.is_empty() {
        return hits;
    }
    use std::collections::{HashMap, VecDeque};

    let mut type_order: Vec<String> = Vec::new();
    let mut queues: HashMap<String, VecDeque<Observation>> = HashMap::new();
    for o in hits {
        if !queues.contains_key(&o.r#type) {
            type_order.push(o.r#type.clone());
            queues.insert(o.r#type.clone(), VecDeque::new());
        }
        queues.get_mut(&o.r#type).unwrap().push_back(o);
    }

    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut out = Vec::new();
    loop {
        let mut progressed = false;
        for t in &type_order {
            let used = counts.get(t).copied().unwrap_or(0);
            if used >= max_per_type {
                continue;
            }
            if let Some(q) = queues.get_mut(t) {
                if let Some(o) = q.pop_front() {
                    counts.insert(t.clone(), used + 1);
                    out.push(o);
                    progressed = true;
                }
            }
        }
        if !progressed {
            break;
        }
    }
    out
}

/// Pack observations in order until the next one would exceed `max_tokens`.
/// Never emits a partial observation. Returns `(kept, tokens_used)`.
/// `max_tokens == 0` means unlimited (still reports the sum).
pub fn pack_by_token_budget(
    hits: Vec<Observation>,
    max_tokens: u32,
) -> (Vec<Observation>, u32) {
    let mut out = Vec::new();
    let mut used: usize = 0;
    let budget = max_tokens as usize;
    for o in hits {
        let t = estimate_observation_tokens(&o.r#type, &o.title, &o.content);
        if budget > 0 && used + t > budget {
            break;
        }
        used += t;
        out.push(o);
    }
    (out, used.min(u32::MAX as usize) as u32)
}

/// Estimate tokens for already-proto observations (title+content).
pub fn pack_proto_by_token_budget(
    hits: Vec<memlayer_proto::Observation>,
    max_tokens: u32,
) -> (Vec<memlayer_proto::Observation>, u32) {
    let mut out = Vec::new();
    let mut used: usize = 0;
    let budget = max_tokens as usize;
    for o in hits {
        let t = estimate_observation_tokens(&o.r#type, &o.title, &o.content);
        if budget > 0 && used + t > budget {
            break;
        }
        used += t;
        out.push(o);
    }
    (out, used.min(u32::MAX as usize) as u32)
}

#[allow(dead_code)]
pub fn total_estimate(hits: &[Observation]) -> u32 {
    let n: usize = hits
        .iter()
        .map(|o| estimate_observation_tokens(&o.r#type, &o.title, &o.content))
        .sum();
    n.min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(id: i64, ty: &str, title: &str) -> Observation {
        Observation {
            id,
            sync_id: format!("s{id}"),
            session_id: "sess".into(),
            r#type: ty.into(),
            title: title.into(),
            content: "x".repeat(40), // ~10 tokens
            tool_name: None,
            scope: "project".into(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 1,
            duplicate_count: 1,
            last_seen_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            deleted_at: None,
            review_after: None,
            code_anchor: None,
            superseded_count: 0,
            superseded_ids: vec![],
            verify_state: "unanchored".into(),
        }
    }

    #[test]
    fn max_per_type_round_robins() {
        let hits = vec![
            obs(1, "note", "n1"),
            obs(2, "note", "n2"),
            obs(3, "note", "n3"),
            obs(4, "decision", "d1"),
            obs(5, "decision", "d2"),
        ];
        let out = apply_max_per_type(hits, 2);
        assert_eq!(out.len(), 4);
        let notes = out.iter().filter(|o| o.r#type == "note").count();
        let decisions = out.iter().filter(|o| o.r#type == "decision").count();
        assert_eq!(notes, 2);
        assert_eq!(decisions, 2);
        // Round-robin: note, decision, note, decision
        assert_eq!(out[0].r#type, "note");
        assert_eq!(out[1].r#type, "decision");
    }

    #[test]
    fn pack_respects_budget_without_partial() {
        let hits = vec![obs(1, "note", "a"), obs(2, "note", "b"), obs(3, "note", "c")];
        let one = estimate_observation_tokens("note", "a", &"x".repeat(40));
        let (kept, used) = pack_by_token_budget(hits, one as u32);
        assert_eq!(kept.len(), 1);
        assert_eq!(used as usize, one);
    }
}
