// Generated with AI Coding Rules Hub
//! Skeleton (TS-1b): expand_evidence walks ±N observations clamped to session.

use std::path::PathBuf;
use std::sync::Mutex;
use memlayer_core::paths;
use memlayer_eval::{
    datasets::EvalMemory,
    ingest::ingest_memories,
    retrieve::expand_evidence,
};
use memlayer_storage::ProjectRegistry;
use std::sync::Arc;
use std::time::Duration;

/// `MEMLAYER_DATA_DIR` is process-global; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fresh_data_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("memlayer-eval-tests")
        .join(format!("{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mk_mem(project: &str, session: &str, title: &str, content: &str) -> EvalMemory {
    EvalMemory {
        project: project.to_string(),
        session_id: session.to_string(),
        obs_type: "conversation".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        topic_key: None,
    }
}

#[tokio::test]
async fn expand_evidence_returns_window_clamped_to_session() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("expand-evidence");
    let session_a = uuid::Uuid::new_v4().to_string();
    let session_b = uuid::Uuid::new_v4().to_string();
    let mut mems: Vec<EvalMemory> = (0..5)
        .map(|i| mk_mem("ee-proj", &session_a, &format!("a{i}"), &format!("session-A turn {i}")))
        .collect();
    // Different session afterward — must NOT bleed into expansion.
    mems.extend((0..5).map(|i| mk_mem("ee-proj", &session_b, &format!("b{i}"), &format!("session-B turn {i}"))));
    ingest_memories(&data_dir, &mems, 1024).await.unwrap();

    // Open the project DB read-only to find a seed obs id.
    std::env::set_var("MEMLAYER_DATA_DIR", &data_dir);
    paths::ensure_dirs(&data_dir).ok();
    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project = registry.get_or_open("ee-proj").unwrap();
    let conn = project.open_read_conn().unwrap();

    // Pick the 3rd obs of session_a (id=3 typically; AUTOINCREMENT starts at 1).
    let seed_id: i64 = conn
        .query_row(
            "SELECT id FROM observations WHERE session_id=?1 ORDER BY id ASC LIMIT 1 OFFSET 2",
            [&session_a],
            |r| r.get(0),
        )
        .unwrap();

    let rows = expand_evidence(&conn, seed_id, 2).unwrap();
    // window=2 should return up to 5 rows from session_a (ids seed-2..=seed+2),
    // and importantly NONE from session_b even if their ids fall within that range.
    assert!(rows.len() >= 3 && rows.len() <= 5, "got {} rows: {:?}", rows.len(), rows);
    for (_, content) in &rows {
        assert!(content.contains("session-A"), "leaked from another session: {content}");
    }
}

#[tokio::test]
async fn expand_evidence_window_zero_returns_seed_only() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("expand-evidence-zero");
    let session = uuid::Uuid::new_v4().to_string();
    let mems: Vec<EvalMemory> = (0..3)
        .map(|i| mk_mem("ee0-proj", &session, &format!("t{i}"), &format!("turn {i}")))
        .collect();
    ingest_memories(&data_dir, &mems, 1024).await.unwrap();

    std::env::set_var("MEMLAYER_DATA_DIR", &data_dir);
    paths::ensure_dirs(&data_dir).ok();
    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project = registry.get_or_open("ee0-proj").unwrap();
    let conn = project.open_read_conn().unwrap();

    let seed_id: i64 = conn
        .query_row(
            "SELECT id FROM observations WHERE session_id=?1 ORDER BY id ASC LIMIT 1",
            [&session],
            |r| r.get(0),
        )
        .unwrap();

    let rows = expand_evidence(&conn, seed_id, 0).unwrap();
    assert_eq!(rows.len(), 1, "window=0 should return only the seed");
}
