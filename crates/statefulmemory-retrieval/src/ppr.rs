//! HippoRAG-style Personalized PageRank over a small entity subgraph.
//!
//! Seeds get equal teleport mass. Iterates
//! `p ← (1-α)·e + α·Aᵀp` until `iters` (default α=0.5, matching HippoRAG's
//! damping). Callers aggregate node scores onto passages / observations.

use std::collections::HashMap;

/// Run PPR on a dense adjacency built from weighted directed edges.
///
/// `edges` are `(from, to, weight)` with node ids in `0..n`. Self-loops and
/// zero-weight edges are ignored. Isolated nodes keep only teleport mass.
pub fn personalized_pagerank(
    n: usize,
    edges: &[(usize, usize, f64)],
    seeds: &[usize],
    damping: f64,
    iters: u32,
) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let alpha = damping.clamp(0.0, 1.0);
    let mut teleport = vec![0.0f64; n];
    let seed_n = seeds.iter().filter(|&&i| i < n).count().max(1) as f64;
    for &s in seeds {
        if s < n {
            teleport[s] += 1.0 / seed_n;
        }
    }
    // If no valid seeds, uniform teleport.
    if seeds.iter().all(|&i| i >= n) {
        let u = 1.0 / n as f64;
        teleport.fill(u);
    }

    // Column-stochastic transition: for each from-node, normalize outgoing.
    let mut out_w = vec![0.0f64; n];
    for &(u, v, w) in edges {
        if u < n && v < n && w > 0.0 && u != v {
            out_w[u] += w;
        }
    }
    // Sparse CSR-ish: list of (to, prob) per from.
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(u, v, w) in edges {
        if u < n && v < n && w > 0.0 && u != v {
            let p = w / out_w[u];
            adj[u].push((v, p));
        }
    }

    let mut p = teleport.clone();
    let mut next = vec![0.0f64; n];
    let steps = iters.max(1);
    for _ in 0..steps {
        next.fill(0.0);
        for u in 0..n {
            let mass = p[u];
            if mass == 0.0 {
                continue;
            }
            if adj[u].is_empty() {
                // Dangling: redistribute via teleport.
                for i in 0..n {
                    next[i] += alpha * mass * teleport[i];
                }
            } else {
                for &(v, prob) in &adj[u] {
                    next[v] += alpha * mass * prob;
                }
            }
        }
        for i in 0..n {
            next[i] += (1.0 - alpha) * teleport[i];
        }
        p.clone_from(&next);
    }
    p
}

/// Compact entity-id graph → dense indices, run PPR, return scores keyed by
/// original entity id. Optionally multiply by node specificity `1/degree`.
pub fn ppr_entity_scores(
    entity_ids: &[i64],
    edges: &[(i64, i64, f64)],
    seed_ids: &[i64],
    damping: f64,
    iters: u32,
    specificity: &HashMap<i64, f64>,
) -> HashMap<i64, f64> {
    let mut index: HashMap<i64, usize> = HashMap::new();
    for &id in entity_ids {
        if !index.contains_key(&id) {
            let i = index.len();
            index.insert(id, i);
        }
    }
    for &(a, b, _) in edges {
        if !index.contains_key(&a) {
            let i = index.len();
            index.insert(a, i);
        }
        if !index.contains_key(&b) {
            let i = index.len();
            index.insert(b, i);
        }
    }
    let n = index.len();
    if n == 0 {
        return HashMap::new();
    }
    let dense_edges: Vec<(usize, usize, f64)> = edges
        .iter()
        .filter_map(|&(a, b, w)| Some((*index.get(&a)?, *index.get(&b)?, w)))
        .collect();
    let seeds: Vec<usize> = seed_ids
        .iter()
        .filter_map(|id| index.get(id).copied())
        .collect();
    let scores = personalized_pagerank(n, &dense_edges, &seeds, damping, iters);
    let mut inv: Vec<i64> = vec![0; n];
    for (&id, &i) in &index {
        inv[i] = id;
    }
    let mut out = HashMap::new();
    for (i, s) in scores.into_iter().enumerate() {
        let id = inv[i];
        let spec = specificity.get(&id).copied().unwrap_or(1.0);
        out.insert(id, s * spec);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_retain_mass_on_disconnected() {
        let scores = personalized_pagerank(3, &[], &[0, 2], 0.5, 10);
        assert!((scores[0] - scores[2]).abs() < 1e-9);
        assert!(scores[0] > scores[1]);
    }

    #[test]
    fn neighbor_receives_mass() {
        // 0 → 1 → 2; seed 0
        let edges = vec![(0, 1, 1.0), (1, 2, 1.0)];
        let scores = personalized_pagerank(3, &edges, &[0], 0.5, 20);
        assert!(scores[1] > 0.0);
        assert!(scores[2] > 0.0);
        assert!(scores[0] > scores[2]); // teleport keeps seed high
    }

    #[test]
    fn entity_scores_map_back() {
        let edges = vec![(10, 20, 1.0), (20, 30, 1.0)];
        let ids = vec![10, 20, 30];
        let scores = ppr_entity_scores(&ids, &edges, &[10], 0.5, 15, &HashMap::new());
        assert!(scores.get(&20).copied().unwrap_or(0.0) > 0.0);
        assert!(scores.get(&30).copied().unwrap_or(0.0) > 0.0);
    }
}
