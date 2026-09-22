//! Token budget packing and per-type round-robin quotas.

use statefulmemory_core::tokens::estimate_observation_tokens;
use statefulmemory_storage::Observation;

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
pub fn pack_by_token_budget(hits: Vec<Observation>, max_tokens: u32) -> (Vec<Observation>, u32) {
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
    hits: Vec<statefulmemory_proto::Observation>,
    max_tokens: u32,
) -> (Vec<statefulmemory_proto::Observation>, u32) {
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

/// Slot budgets for effective-context compile (Wave 2).
/// Decisions 35% · anchored code 35% · other 30%. Never partial-cuts an obs.
pub fn pack_proto_by_slots(
    hits: Vec<statefulmemory_proto::Observation>,
    max_tokens: u32,
) -> (
    Vec<statefulmemory_proto::Observation>,
    Vec<statefulmemory_proto::Observation>,
    Vec<statefulmemory_proto::Observation>,
    u32,
) {
    let mut decisions = Vec::new();
    let mut anchored = Vec::new();
    let mut other = Vec::new();
    for o in hits {
        if o.r#type == "decision" || o.r#type == "policy" {
            decisions.push(o);
        } else if !o.code_anchor.as_deref().unwrap_or("").is_empty() {
            anchored.push(o);
        } else {
            other.push(o);
        }
    }

    if max_tokens == 0 {
        let used = total_estimate_proto(
            decisions
                .iter()
                .chain(anchored.iter())
                .chain(other.iter()),
        );
        return (decisions, anchored, other, used);
    }

    // Soft slot shares; unused budget spills forward so a tiny window still
    // packs at least one decision when it fits the total.
    let d_share = ((max_tokens as f64) * 0.35) as u32;
    let a_share = ((max_tokens as f64) * 0.35) as u32;
    let o_share = max_tokens.saturating_sub(d_share).saturating_sub(a_share);

    let (decisions, d_used) = pack_slot(decisions, d_share.max(1), max_tokens);
    let remain_after_d = max_tokens.saturating_sub(d_used);
    let a_budget = a_share
        .saturating_add(d_share.saturating_sub(d_used))
        .min(remain_after_d)
        .max(1);
    let (anchored, a_used) = pack_slot(anchored, a_budget, remain_after_d);
    let remain_after_a = remain_after_d.saturating_sub(a_used);
    let o_budget = o_share
        .saturating_add(a_budget.saturating_sub(a_used))
        .min(remain_after_a)
        .max(1);
    let (other, o_used) = pack_slot(other, o_budget, remain_after_a);
    let used = d_used.saturating_add(a_used).saturating_add(o_used);
    (decisions, anchored, other, used)
}

/// Pack a slot under `slot_budget`, allowing one oversized lead item when it
/// still fits in `total_remain` (so 35% of a small window cannot starve).
fn pack_slot(
    hits: Vec<statefulmemory_proto::Observation>,
    slot_budget: u32,
    total_remain: u32,
) -> (Vec<statefulmemory_proto::Observation>, u32) {
    if hits.is_empty() || total_remain == 0 {
        return (Vec::new(), 0);
    }
    let (packed, used) = pack_proto_by_token_budget(hits.clone(), slot_budget);
    if !packed.is_empty() {
        return (packed, used);
    }
    let first = &hits[0];
    let need = estimate_observation_tokens(&first.r#type, &first.title, &first.content) as u32;
    if need > 0 && need <= total_remain {
        return (vec![hits.into_iter().next().unwrap()], need);
    }
    (Vec::new(), 0)
}

fn total_estimate_proto<'a>(
    hits: impl Iterator<Item = &'a statefulmemory_proto::Observation>,
) -> u32 {
    let n: usize = hits
        .map(|o| estimate_observation_tokens(&o.r#type, &o.title, &o.content))
        .sum();
    n.min(u32::MAX as usize) as u32
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
        let hits = vec![
            obs(1, "note", "a"),
            obs(2, "note", "b"),
            obs(3, "note", "c"),
        ];
        let one = estimate_observation_tokens("note", "a", &"x".repeat(40));
        let (kept, used) = pack_by_token_budget(hits, one as u32);
        assert_eq!(kept.len(), 1);
        assert_eq!(used as usize, one);
    }

    #[test]
    fn pack_by_slots_reserves_decision_budget() {
        let mut hits = Vec::new();
        for i in 0..5 {
            let mut o = obs(i, "decision", &format!("d{i}"));
            o.content = "y".repeat(80);
            hits.push(to_proto(o));
        }
        for i in 5..10 {
            let mut o = obs(i, "note", &format!("n{i}"));
            o.content = "y".repeat(80);
            hits.push(to_proto(o));
        }
        let (dec, anc, oth, used) = pack_proto_by_slots(hits, 40);
        assert!(!dec.is_empty());
        assert!(anc.is_empty());
        assert!(!oth.is_empty() || used > 0);
        assert!(used <= 40);
    }

    #[test]
    fn pack_32k_keeps_decision_and_anchor_slots() {
        let mut hits = Vec::new();
        for i in 0..20 {
            let mut o = obs(i, "decision", &format!("d{i}"));
            o.content = "body ".repeat(40);
            hits.push(to_proto(o));
        }
        for i in 20..40 {
            let mut o = obs(i, "note", &format!("a{i}"));
            o.content = "code ".repeat(40);
            o.code_anchor = Some("src/foo.rs::bar".into());
            hits.push(to_proto(o));
        }
        for i in 40..80 {
            let mut o = obs(i, "note", &format!("n{i}"));
            o.content = "x ".repeat(40);
            hits.push(to_proto(o));
        }
        let (dec, anc, _oth, used) = pack_proto_by_slots(hits, 32_000);
        assert!(!dec.is_empty(), "32k window must reserve decision slots");
        assert!(!anc.is_empty(), "32k window must reserve anchor slots");
        assert!(used <= 32_000);
        assert!(used > 0);
    }

    #[test]
    fn pack_200k_scales_without_exceeding_budget() {
        let hits: Vec<_> = (0..200)
            .map(|i| {
                let ty = if i % 5 == 0 { "decision" } else { "note" };
                let mut o = obs(i, ty, &format!("t{i}"));
                o.content = "y".repeat(200);
                if i % 3 == 0 {
                    o.code_anchor = Some("crates/x/src/lib.rs::f".into());
                }
                to_proto(o)
            })
            .collect();
        let (dec, anc, oth, used) = pack_proto_by_slots(hits, 200_000);
        assert!(used <= 200_000);
        assert!(!dec.is_empty() || !anc.is_empty() || !oth.is_empty());
    }

    fn to_proto(o: Observation) -> statefulmemory_proto::Observation {
        statefulmemory_proto::Observation {
            id: o.id,
            sync_id: o.sync_id,
            session_id: o.session_id,
            r#type: o.r#type,
            title: o.title,
            content: o.content,
            tool_name: o.tool_name,
            scope: o.scope,
            created_by: o.created_by,
            topic_key: o.topic_key,
            normalized_hash: o.normalized_hash,
            revision_count: o.revision_count,
            duplicate_count: o.duplicate_count,
            last_seen_at: o.last_seen_at,
            created_at: o.created_at,
            updated_at: o.updated_at,
            deleted_at: o.deleted_at,
            review_after: o.review_after,
            project_name: None,
            code_anchor: o.code_anchor,
            supersedes_ids: o.superseded_ids,
            superseded_count: o.superseded_count,
            verify_state: Some(o.verify_state),
        }
    }
}
