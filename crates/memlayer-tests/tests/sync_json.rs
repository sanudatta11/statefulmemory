// Generated with AI Coding Rules Hub
//! TDD skeleton tests for sync-and-export Spec 3 — JSON path.

#[tokio::test]
async fn ts7_json_round_trip() {
    // Spec ref: TS-7, SC-8, FR4.1, FR4.2
    // SyncExportJson to file; hard-wipe DB via DeleteProject --hard;
    // SyncImportJson; assert row-for-row equality.
    panic!("TDD skeleton: not yet implemented (spec-task-7)");
}

#[tokio::test]
async fn ts17_json_missing_version() {
    // Spec ref: TS-17, EC-4
    // SyncImportJson on a hand-crafted JSON without `version` field
    // returns gRPC InvalidArgument.
    panic!("TDD skeleton: not yet implemented (spec-task-7)");
}

#[tokio::test]
async fn ts20_sigkill_mid_export_json() {
    // Spec ref: TS-20, EH-2
    // Spawn SyncExportJson from a child process; SIGKILL mid-write;
    // assert no partial output file (only .tmp may remain), re-run succeeds.
    panic!("TDD skeleton: not yet implemented (spec-task-7)");
}
