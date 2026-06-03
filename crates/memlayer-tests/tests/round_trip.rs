//! TS-1, TS-2: round-trip + lifecycle smoke tests.

use memlayer_proto::*;
use memlayer_tests::{connect, spawn_daemon};

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
            key: Some(get_observation_request::Key::SyncId(saved_obs.sync_id.clone())),
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
    let s = client.daemon_status(DaemonStatusRequest {}).await.unwrap().into_inner();
    assert!(!s.version.is_empty());
    assert!(!s.read_only_mode);
    assert!(s.in_flight_rpcs >= 0);
}
