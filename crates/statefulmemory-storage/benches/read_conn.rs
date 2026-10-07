//! Micro-bench: fresh `open_read` (+7 pragmas, per Phase-0 baseline) vs pooled
//! `checkout_read` reuse. This is the quantification harness for the Phase 1.1
//! read-connection pool.
//!
//! Dependency-free (`harness = false`) so it builds and runs offline — the
//! sandbox blocks crates.io, so criterion can't be fetched here. Swap this for
//! a criterion bench once registry access is available; the measured quantity
//! (ns/op for open vs checkout) is the same.
//!
//! Run: `cargo bench -p statefulmemory-storage` (or build with
//! `cargo build --benches -p statefulmemory-storage` and run the binary with
//! `BENCH_ITERS=2000`).

use std::time::{Duration, Instant};

use statefulmemory_storage::open_read;
use statefulmemory_storage::write::spawn_write_thread;
use statefulmemory_storage::ProjectState;

fn bench<F: FnMut()>(label: &str, iters: u32, mut f: F) -> Duration {
    for _ in 0..(iters / 10).max(1) {
        f(); // warmup
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let elapsed = start.elapsed();
    println!(
        "{label:<30} {iters} iters  total={elapsed:?}  per_op={:?}",
        elapsed / iters
    );
    elapsed
}

fn main() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("bench.db");

    // Create + migrate the DB synchronously (spawn_write_thread opens the write
    // connection before returning, so the file + schema exist for reads).
    let write = spawn_write_thread(
        "bench".into(),
        db_path.clone(),
        32,
        Duration::from_millis(1),
        None,
    )
    .expect("spawn write thread");
    let project = ProjectState::new("bench".into(), "bench".into(), db_path.clone(), write, 4);

    let iters: u32 = std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);

    let fresh = bench("open_read (fresh+pragmas)", iters, || {
        let conn = open_read(&db_path).expect("open_read");
        let _: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).unwrap();
    });

    let pooled = bench("checkout_read (pooled)", iters, || {
        let conn = project.checkout_read().expect("checkout");
        let _: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).unwrap();
    });

    if pooled.as_nanos() > 0 {
        println!(
            "speedup (fresh/pooled) = {:.1}x",
            fresh.as_nanos() as f64 / pooled.as_nanos() as f64
        );
    }
}
