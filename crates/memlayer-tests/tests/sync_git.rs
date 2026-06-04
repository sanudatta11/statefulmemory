// Generated with AI Coding Rules Hub
//! TDD skeleton tests for sync-and-export Spec 3 — git-chunk path.
//!
//! These tests MUST fail until the corresponding implementation tasks land.
//! Each function has a `panic!` body keyed to the task that will implement it.

#[tokio::test]
async fn ts1_export_writes_valid_chunk() {
    // Spec ref: TS-1, SC-1, FR1.1
    // Save N=3 obs, call SyncExport. Assert chunk file exists, file is non-empty,
    // decompressing yields N JSONL lines, sha256(uncompressed)[..16] == returned chunk_id.
    panic!("TDD skeleton: not yet implemented (spec-task-4)");
}

#[tokio::test]
async fn ts2_export_idempotent() {
    // Spec ref: TS-2, SC-2, FR1.2
    // After ts1's export, second SyncExport returns chunk_id: None and writes no new file.
    panic!("TDD skeleton: not yet implemented (spec-task-4)");
}

#[tokio::test]
async fn ts3_export_then_import_round_trip() {
    // Spec ref: TS-3, SC-3, SC-4
    // Export on tempdir A; copy .memlayer/ to tempdir B (separate daemon, separate
    // MEMLAYER_DATA_DIR); import on B; assert all rows present, second import = 0 counts.
    panic!("TDD skeleton: not yet implemented (spec-task-5)");
}

#[tokio::test]
async fn ts4_conflict_later_wins() {
    // Spec ref: TS-4, SC-5, FR2.1
    // Pre-populate B with sync_id X having older updated_at; import overwrites.
    // Pre-populate with newer; import retained.
    panic!("TDD skeleton: not yet implemented (spec-task-5)");
}

#[tokio::test]
async fn ts5_corrupt_chunk_skipped() {
    // Spec ref: TS-5, SC-6, FR2.2, EH-3
    // Write a <id>.jsonl.zst with random bytes; import skips it,
    // sync_status shows last_error, other chunks succeed, DB intact.
    panic!("TDD skeleton: not yet implemented (spec-task-5)");
}

#[tokio::test]
async fn ts6_full_git_round_trip() {
    // Spec ref: TS-6, SC-7
    // Two tempdirs each with fresh git init; SyncExport on A; cp -r A/.memlayer B/;
    // SyncImport on B (separate daemon, separate MEMLAYER_DATA_DIR); obs search finds A's rows.
    panic!("TDD skeleton: not yet implemented (spec-task-11)");
}

#[tokio::test]
async fn ts13_export_all_no_cross_contamination() {
    // Spec ref: TS-13, SC-15
    // Three projects, each gets one chunk via three sequential SyncExport calls;
    // assert filesystem isolation (per-project chunk dirs).
    panic!("TDD skeleton: not yet implemented (spec-task-4)");
}

#[tokio::test]
async fn ts15_empty_project_export() {
    // Spec ref: TS-15, EC-1
    // SyncExport on project with zero unexported rows returns chunk_id: None,
    // writes no chunk file, manifest unchanged.
    panic!("TDD skeleton: not yet implemented (spec-task-11)");
}

#[tokio::test]
async fn ts16_missing_chunk_file() {
    // Spec ref: TS-16, EC-2
    // Manifest references a chunk_id whose file was deleted;
    // import skips with warning, others imported, last_error populated.
    panic!("TDD skeleton: not yet implemented (spec-task-5)");
}
