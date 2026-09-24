//! TS-1, TS-2: round-trip + lifecycle smoke tests.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use statefulmemory_proto::*;
use statefulmemory_tests::{connect, spawn_daemon};

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

#[tokio::test]
async fn verify_anchors_keeps_daemon_alive() {
    let repo = tempfile::TempDir::new().unwrap();
    run_git(repo.path(), &["init", "-q", "."]);
    run_git(repo.path(), &["config", "user.email", "verify@example.com"]);
    run_git(repo.path(), &["config", "user.name", "verify"]);
    fs::create_dir_all(repo.path().join("src")).unwrap();
    fs::write(repo.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    run_git(repo.path(), &["add", "."]);
    run_git(repo.path(), &["commit", "-qm", "initial"]);

    let handle = spawn_daemon();
    let mut client = connect(&handle).await;
    let project = "verify-rpc";
    let session_id = uuid::Uuid::new_v4().to_string();
    client
        .start_session(StartSessionRequest {
            project_name: project.into(),
            id: session_id.clone(),
            directory: repo.path().to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    client
        .sync_export(SyncExportRequest {
            project_name: project.into(),
            repo_path: Some(repo.path().to_string_lossy().into_owned()),
        })
        .await
        .unwrap();

    let saved = client
        .save_observation(SaveObservationRequest {
            project_name: project.into(),
            session_id: session_id.clone(),
            r#type: "note".into(),
            title: "anchored note".into(),
            content: "remember this".into(),
            scope: "project".into(),
            anchors: vec!["src/main.rs".into()],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .observation
        .unwrap();

    fs::write(
        repo.path().join("src/main.rs"),
        "fn main() { changed(); }\n",
    )
    .unwrap();
    run_git(repo.path(), &["add", "."]);
    run_git(repo.path(), &["commit", "-qm", "changed"]);

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client.verify_anchors(VerifyAnchorsRequest {
            project_name: project.into(),
            observation_id: Some(saved.id),
        }),
    )
    .await
    .expect("verify timed out");
    let response = match response {
        Ok(response) => response,
        Err(status) => {
            let stderr = fs::read_to_string(handle.data_dir.path().join("daemon.stderr"))
                .unwrap_or_default();
            panic!("verify RPC failed: {status:?}; daemon stderr: {stderr}");
        }
    }
    .into_inner();
    assert_eq!(response.stale, 1);
    assert!(response.results.iter().any(|r| r.state == "stale"));

    client
        .daemon_status(DaemonStatusRequest {})
        .await
        .expect("daemon must remain available after verify");
}

#[tokio::test]
async fn ts2_save_get_roundtrip() {
    let h = spawn_daemon();
    let mut client = connect(&h).await;

    // Start a session so observations have a valid foreign key.
    let session_id = uuid::Uuid::new_v4().to_string();
    client
        .start_session(StartSessionRequest {
            project_name: "test-proj".into(),
            id: session_id.clone(),
            directory: "/tmp/test".into(),
        })
        .await
        .unwrap();

    let saved = client
        .save_observation(SaveObservationRequest {
            project_name: "test-proj".into(),
            sync_id: None,
            session_id: session_id.clone(),
            r#type: "note".into(),
            title: "first".into(),
            content: "hello world".into(),
            tool_name: None,
            scope: "project".into(),
            created_by: None,
            topic_key: None,
            code_anchor: None,
            anchors: vec![],
        })
        .await
        .unwrap()
        .into_inner();
    let saved_obs = saved.observation.unwrap();
    assert_eq!(saved_obs.title, "first");
    assert_eq!(saved_obs.revision_count, 1);
    assert_eq!(saved_obs.duplicate_count, 1);

    let got = client
        .get_observation(GetObservationRequest {
            project_name: "test-proj".into(),
            key: Some(get_observation_request::Key::SyncId(
                saved_obs.sync_id.clone(),
            )),
        })
        .await
        .unwrap()
        .into_inner();
    let got_obs = got.observation.unwrap();
    assert_eq!(got_obs.id, saved_obs.id);
    assert_eq!(got_obs.content, "hello world");
}

#[tokio::test]
async fn ts1_daemon_status_responds() {
    let h = spawn_daemon();
    let mut client = connect(&h).await;
    let s = client
        .daemon_status(DaemonStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(!s.version.is_empty());
    assert!(!s.read_only_mode);
    assert!(s.in_flight_rpcs >= 0);
}
