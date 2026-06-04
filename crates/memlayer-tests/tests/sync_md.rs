// Generated with AI Coding Rules Hub
//! TDD skeleton tests for sync-and-export Spec 3 — Markdown path.

#[tokio::test]
async fn ts8b_md_byte_identical_reexport() {
    // Spec ref: TS-8 (export half), SC-10
    // Export, parse a sample frontmatter file, assert key order;
    // second export with same DB state produces byte-identical files.
    panic!("TDD skeleton: not yet implemented (spec-task-8)");
}

#[tokio::test]
async fn ts8_md_round_trip() {
    // Spec ref: TS-8 (import half), SC-9
    // Export -> wipe -> import -> diff empty.
    panic!("TDD skeleton: not yet implemented (spec-task-9)");
}

#[tokio::test]
async fn ts9_md_manual_edit() {
    // Spec ref: TS-9, SC-11, FR5.4
    // Export, hand-edit a .md body and bump updated_at, import,
    // `obs get` shows new content.
    panic!("TDD skeleton: not yet implemented (spec-task-9)");
}

#[tokio::test]
async fn ts11_md_since_filter() {
    // Spec ref: TS-11, SC-13, FR5.3
    // Pre-load rows with timestamps; --since filter cuts the set.
    panic!("TDD skeleton: not yet implemented (spec-task-8)");
}

#[tokio::test]
async fn ts18_md_malformed_yaml_skipped() {
    // Spec ref: TS-18, EC-6, EH-4
    // Drop a file with broken frontmatter into the dir;
    // import skips it, others succeed, files_skipped incremented.
    panic!("TDD skeleton: not yet implemented (spec-task-9)");
}
