//! In-process LRU for search / context result ids (Wave 3).
//!
//! Keys fingerprint project + mode + query + filters + limit + config bits.
//! Values are observation ids; callers re-hydrate from SQLite on hit.
//! TTL default 30s; invalidate a project on any successful write.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

const DEFAULT_CAP: usize = 256;
const DEFAULT_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Entry {
    ids: Vec<i64>,
    inserted: Instant,
}

/// Process-wide search result cache shared via [`DaemonState`].
pub struct SearchCache {
    inner: Mutex<Inner>,
}

struct Inner {
    map: HashMap<String, Entry>,
    order: VecDeque<String>,
    cap: usize,
    ttl: Duration,
    hits: u64,
    misses: u64,
}

impl Default for SearchCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAP, DEFAULT_TTL)
    }
}

impl SearchCache {
    pub fn new(cap: usize, ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
                cap: cap.max(1),
                ttl,
                hits: 0,
                misses: 0,
            }),
        }
    }

    pub fn get(&self, key: &str) -> Option<Vec<i64>> {
        let mut g = self.inner.lock();
        let now = Instant::now();
        let ttl = g.ttl;
        let fresh = g
            .map
            .get(key)
            .filter(|e| now.duration_since(e.inserted) <= ttl)
            .map(|e| e.ids.clone());
        if let Some(ids) = fresh {
            g.hits += 1;
            return Some(ids);
        }
        g.map.remove(key);
        g.misses += 1;
        None
    }

    pub fn put(&self, key: String, ids: Vec<i64>) {
        let mut g = self.inner.lock();
        if let Some(entry) = g.map.get_mut(&key) {
            entry.ids = ids;
            entry.inserted = Instant::now();
            return;
        }
        while g.order.len() >= g.cap {
            if let Some(old) = g.order.pop_front() {
                g.map.remove(&old);
            } else {
                break;
            }
        }
        g.order.push_back(key.clone());
        g.map.insert(
            key,
            Entry {
                ids,
                inserted: Instant::now(),
            },
        );
    }

    /// Drop every entry whose key starts with `project:`.
    pub fn invalidate_project(&self, project: &str) {
        let prefix = format!("{project}\0");
        let mut g = self.inner.lock();
        g.order.retain(|k| !k.starts_with(&prefix));
        g.map.retain(|k, _| !k.starts_with(&prefix));
    }

    pub fn stats(&self) -> (u64, u64, usize) {
        let g = self.inner.lock();
        (g.hits, g.misses, g.map.len())
    }
}

/// Build a stable cache key. `cfg_fp` should capture mode/rerank/router bits.
pub fn cache_key(
    project: &str,
    query: &str,
    mode: &str,
    limit: i32,
    type_filter: Option<&str>,
    scope_filter: Option<&str>,
    cfg_fp: &str,
) -> String {
    let mut h = Sha256::new();
    h.update(query.trim().to_ascii_lowercase().as_bytes());
    h.update(b"|");
    h.update(mode.as_bytes());
    h.update(b"|");
    h.update(limit.to_string().as_bytes());
    h.update(b"|");
    h.update(type_filter.unwrap_or("").as_bytes());
    h.update(b"|");
    h.update(scope_filter.unwrap_or("").as_bytes());
    h.update(b"|");
    h.update(cfg_fp.as_bytes());
    let dig = hex::encode(h.finalize());
    format!("{project}\0{dig}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_miss_and_invalidate() {
        let c = SearchCache::new(8, Duration::from_secs(60));
        let k = cache_key("p", "q", "hybrid", 10, None, None, "fp");
        assert!(c.get(&k).is_none());
        c.put(k.clone(), vec![1, 2, 3]);
        assert_eq!(c.get(&k), Some(vec![1, 2, 3]));
        c.invalidate_project("p");
        assert!(c.get(&k).is_none());
    }
}
