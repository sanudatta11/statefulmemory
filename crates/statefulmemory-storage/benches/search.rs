//! Micro-bench: BM25 search read-path latency — plain `search` vs the
//! column-weighted `search_scored` (Phase 2.2), over a seeded corpus.
//!
//! Dependency-free (`harness = false`, std::time) so it runs offline. Pairs
//! with `read_conn.rs` (connection-acquire cost) to cover the read hot path.
//! The full daemon save/search latency (write thread + embedder + fusion)
//! needs a live service harness and is measured by the eval benchmark; this
//! isolates the SQLite read cost.
//!
//! Run: `cargo bench -p statefulmemory-storage --bench search`
//! (or `cargo build --benches -p statefulmemory-storage` then run the binary;
//! `BENCH_ROWS` seeds the corpus, `BENCH_ITERS` the query count).

use std::time::Instant;

use statefulmemory_storage::{open_write, read};

fn bench<F: FnMut()>(label: &str, iters: u32, mut f: F) {
    for _ in 0..(iters / 10).max(1) {
        f(); // warmup
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let elapsed = start.elapsed();
    println!(
        "{label:<28} {iters} iters  total={elapsed:?}  per_op={:?}",
        elapsed / iters
    );
}

fn main() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("search_bench.db");
    let conn = open_write(&path).expect("open_write");
    conn.execute(
        "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
        [],
    )
    .expect("seed session");

    let rows: usize = std::env::var("BENCH_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    for i in 0..rows {
        // Inlined values (bench data, no untrusted input). Half the rows carry
        // the query terms so BM25 has real hits to rank.
        let body = if i % 2 == 0 {
            format!("content {i} auth jwt rotation gamma delta")
        } else {
            format!("content {i} unrelated lorem ipsum sit amet")
        };
        let sql = format!(
            "INSERT INTO observations (sync_id, session_id, type, title, content) \
             VALUES ('sync-{i}', 's1', 'note', 'title {i} alpha beta', '{body}')"
        );
        conn.execute(&sql, []).expect("seed observation");
    }

    let iters: u32 = std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);

    println!("corpus: {rows} observations");
    bench("search (bm25)", iters, || {
        let _ = read::search(&conn, "auth jwt rotation", None, None, 10).expect("search");
    });
    bench("search_scored (weighted)", iters, || {
        let _ = read::search_scored(&conn, "auth jwt rotation", None, None, 60).expect("scored");
    });
}
