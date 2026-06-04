// Generated with AI Coding Rules Hub
//! TDD skeleton tests for sync-and-export Spec 3 — chaos + concurrency.

#[tokio::test]
async fn ts12_sigkill_mid_export() {
    // Spec ref: TS-12, SC-14
    // Spawn daemon, start SyncExport from CLI subprocess; sleep 50-200ms;
    // SIGKILL the daemon. Restart, verify no partial chunk file (only .tmp may remain),
    // exported_at NULL for all rows, re-run SyncExport succeeds.
    panic!("TDD skeleton: not yet implemented (spec-task-11)");
}

#[tokio::test]
async fn ts19_concurrent_export_no_duplicate_rows() {
    // Spec ref: TS-19, EC-7
    // Two tokio::spawn tasks calling SyncExport concurrently against the same project;
    // assert exactly one chunk file written, total exported rows = N, no duplicates,
    // exported_at populated exactly once. Tests the per-project tokio::sync::Mutex guard.
    panic!("TDD skeleton: not yet implemented (spec-task-4)");
}
