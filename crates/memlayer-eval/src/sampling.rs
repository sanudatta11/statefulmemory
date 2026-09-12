//! Query-limit sampling for eval runs.
//!
//! LoCoMo `LIMIT=N` used to be a prefix `take(N)`, which over-weights early
//! multi-hop items and omits single-hop — unfair vs Mem0/Engram overalls.
//! Stratified sampling keeps categories 1–4 proportional (adversarial excluded)
//! and round-robins across conversation projects within each category.

use crate::datasets::EvalQuery;

/// LoCoMo JSON categories included in Mem0-style LLM-judge tables.
pub const LOCOMO_JUDGE_CATEGORIES: &[&str] = &[
    "multi_hop",
    "temporal",
    "open_domain",
    "single_hop",
];

/// Cap queries. For LoCoMo-style named categories, sample proportionally
/// across [`LOCOMO_JUDGE_CATEGORIES`] (largest-remainder). Other benchmarks
/// keep prefix order.
pub fn apply_query_limit(queries: Vec<EvalQuery>, limit: Option<usize>, stratified: bool) -> Vec<EvalQuery> {
    let Some(cap) = limit else {
        return queries;
    };
    if cap == 0 || queries.len() <= cap {
        return queries.into_iter().take(cap).collect();
    }
    if stratified {
        stratify_locomo_queries(queries, cap)
    } else {
        queries.into_iter().take(cap).collect()
    }
}

/// Project key from LoCoMo query id (`conv-26-q3` → `conv-26`).
fn project_key(q: &EvalQuery) -> String {
    q.id
        .rsplit_once("-q")
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_else(|| q.id.clone())
}

/// Round-robin across projects so LIMIT slices are not all from conv-26.
fn take_across_projects(bucket: Vec<EvalQuery>, take_n: usize) -> Vec<EvalQuery> {
    use std::collections::{BTreeMap, VecDeque};

    if take_n == 0 || bucket.is_empty() {
        return Vec::new();
    }
    if bucket.len() <= take_n {
        return bucket;
    }

    let mut by_proj: BTreeMap<String, VecDeque<EvalQuery>> = BTreeMap::new();
    for q in bucket {
        by_proj.entry(project_key(&q)).or_default().push_back(q);
    }
    let mut keys: Vec<String> = by_proj.keys().cloned().collect();
    // Stable order by project name.
    keys.sort();

    let mut out = Vec::with_capacity(take_n);
    while out.len() < take_n {
        let mut progressed = false;
        for key in &keys {
            if out.len() >= take_n {
                break;
            }
            if let Some(q) = by_proj.get_mut(key).and_then(|dq| dq.pop_front()) {
                out.push(q);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    out
}

fn stratify_locomo_queries(queries: Vec<EvalQuery>, cap: usize) -> Vec<EvalQuery> {
    use std::collections::HashMap;

    let mut by_cat: HashMap<String, Vec<EvalQuery>> = HashMap::new();
    let mut other: Vec<EvalQuery> = Vec::new();
    for q in queries {
        match q.category.as_deref() {
            Some(c) if LOCOMO_JUDGE_CATEGORIES.contains(&c) => {
                by_cat.entry(c.to_string()).or_default().push(q);
            }
            Some("adversarial") => {
                // Drop — Mem0 protocol excludes these.
            }
            _ => other.push(q),
        }
    }

    let eligible: usize = LOCOMO_JUDGE_CATEGORIES
        .iter()
        .map(|c| by_cat.get(*c).map(|v| v.len()).unwrap_or(0))
        .sum();
    if eligible == 0 {
        return take_across_projects(other, cap);
    }

    // Largest-remainder allocation across categories that have items.
    let mut alloc: Vec<(&str, usize, f64)> = Vec::new();
    for &name in LOCOMO_JUDGE_CATEGORIES {
        let n = by_cat.get(name).map(|v| v.len()).unwrap_or(0);
        if n == 0 {
            continue;
        }
        let exact = cap as f64 * (n as f64) / (eligible as f64);
        let floor = exact.floor() as usize;
        let frac = exact - floor as f64;
        alloc.push((name, floor.min(n), frac));
    }

    let used: usize = alloc.iter().map(|(_, n, _)| *n).sum();
    let mut rem = cap.saturating_sub(used);
    // Prefer categories with largest fractional remainder, then larger pools.
    alloc.sort_by(|a, b| {
        b.2
            .partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let na = by_cat.get(a.0).map(|v| v.len()).unwrap_or(0);
                let nb = by_cat.get(b.0).map(|v| v.len()).unwrap_or(0);
                nb.cmp(&na)
            })
    });
    for entry in &mut alloc {
        if rem == 0 {
            break;
        }
        let avail = by_cat.get(entry.0).map(|v| v.len()).unwrap_or(0);
        if entry.1 < avail {
            entry.1 += 1;
            rem -= 1;
        }
    }

    // Stable output: category order multi_hop → … → single_hop, then leftover.
    let mut out = Vec::with_capacity(cap);
    for &name in LOCOMO_JUDGE_CATEGORIES {
        let take_n = alloc
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, n, _)| *n)
            .unwrap_or(0);
        if take_n == 0 {
            continue;
        }
        if let Some(bucket) = by_cat.remove(name) {
            out.extend(take_across_projects(bucket, take_n));
        }
    }

    if out.len() < cap {
        out.extend(take_across_projects(other, cap - out.len()));
    }
    out.truncate(cap);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: &str, cat: &str) -> EvalQuery {
        EvalQuery {
            id: id.into(),
            question: "q".into(),
            gold_answer: "a".into(),
            judge_context: None,
            category: Some(cat.into()),
            anti_answer: None,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn stratified_limit_includes_all_judge_categories() {
        let mut qs = Vec::new();
        for i in 0..20 {
            qs.push(q(&format!("mh-{i}"), "multi_hop"));
        }
        for i in 0..20 {
            qs.push(q(&format!("t-{i}"), "temporal"));
        }
        for i in 0..10 {
            qs.push(q(&format!("od-{i}"), "open_domain"));
        }
        for i in 0..50 {
            qs.push(q(&format!("sh-{i}"), "single_hop"));
        }
        // Adversarial must be dropped.
        for i in 0..5 {
            qs.push(q(&format!("adv-{i}"), "adversarial"));
        }

        let out = apply_query_limit(qs, Some(50), true);
        assert_eq!(out.len(), 50);
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for q in &out {
            *counts.entry(q.category.clone().unwrap()).or_default() += 1;
        }
        assert!(counts.contains_key("multi_hop"), "{counts:?}");
        assert!(counts.contains_key("temporal"), "{counts:?}");
        assert!(counts.contains_key("open_domain"), "{counts:?}");
        assert!(counts.contains_key("single_hop"), "{counts:?}");
        assert!(!counts.contains_key("adversarial"));
        // Single-hop is majority of pool → largest share.
        assert!(counts["single_hop"] >= counts["multi_hop"]);
        assert!(counts["single_hop"] >= 20);
    }

    #[test]
    fn stratified_round_robins_across_projects() {
        let mut qs = Vec::new();
        for i in 0..30 {
            qs.push(q(&format!("conv-26-q{i}"), "single_hop"));
        }
        for i in 0..30 {
            qs.push(q(&format!("conv-42-q{i}"), "single_hop"));
        }
        let out = apply_query_limit(qs, Some(10), true);
        assert_eq!(out.len(), 10);
        let p26 = out.iter().filter(|q| q.id.starts_with("conv-26-")).count();
        let p42 = out.iter().filter(|q| q.id.starts_with("conv-42-")).count();
        assert!(p26 >= 4 && p42 >= 4, "expected mix across projects, got p26={p26} p42={p42}");
    }

    #[test]
    fn prefix_limit_when_not_stratified() {
        let qs: Vec<_> = (0..10).map(|i| q(&format!("x-{i}"), "multi_hop")).collect();
        let out = apply_query_limit(qs, Some(3), false);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].id, "x-0");
        assert_eq!(out[2].id, "x-2");
    }
}
