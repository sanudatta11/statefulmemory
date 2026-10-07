//! In-process cache: `sha256(query text)` → query embedding vector.
//!
//! Replaces a plain `HashMap` that cleared itself wholesale on overflow — a
//! full clear evicted every hot query at once, so a burst of distinct queries
//! repeatedly cold-started the embedder. This bounds the cache and evicts the
//! single oldest entry instead (insertion-order, matching `search_cache`).
//! No TTL: a query embedding is deterministic for a given model.

use std::collections::{HashMap, VecDeque};

use parking_lot::Mutex;

const DEFAULT_CAP: usize = 256;

/// Bounded query-embedding cache shared via `DaemonState`.
pub struct QueryVecCache {
    inner: Mutex<Inner>,
}

struct Inner {
    map: HashMap<String, Vec<f32>>,
    order: VecDeque<String>,
    cap: usize,
}

impl Default for QueryVecCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAP)
    }
}

impl QueryVecCache {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
                cap: cap.max(1),
            }),
        }
    }

    /// Return a cached embedding for `key`, if present.
    pub fn get(&self, key: &str) -> Option<Vec<f32>> {
        self.inner.lock().map.get(key).cloned()
    }

    /// Insert (or refresh) an embedding, evicting the oldest entry when the
    /// cache is at capacity. Never clears the whole cache.
    pub fn put(&self, key: String, vec: Vec<f32>) {
        let mut g = self.inner.lock();
        if let Some(slot) = g.map.get_mut(&key) {
            *slot = vec;
            return;
        }
        while g.order.len() >= g.cap {
            match g.order.pop_front() {
                Some(old) => {
                    g.map.remove(&old);
                }
                None => break,
            }
        }
        g.order.push_back(key.clone());
        g.map.insert(key, vec);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_one_not_all_on_overflow() {
        let c = QueryVecCache::new(2);
        c.put("a".into(), vec![1.0]);
        c.put("b".into(), vec![2.0]);
        c.put("c".into(), vec![3.0]); // over cap → evict oldest ("a")
        assert!(c.get("a").is_none(), "oldest evicted");
        assert_eq!(c.get("b"), Some(vec![2.0]), "hot entries survive");
        assert_eq!(c.get("c"), Some(vec![3.0]));
        assert_eq!(c.len(), 2, "cache stays bounded, not cleared");
    }

    #[test]
    fn update_existing_does_not_grow() {
        let c = QueryVecCache::new(2);
        c.put("a".into(), vec![1.0]);
        c.put("a".into(), vec![9.0]);
        assert_eq!(c.get("a"), Some(vec![9.0]));
        assert_eq!(c.len(), 1);
    }
}
