//! TS-3, TS-4, TS-5: dedupe / topic-key / sync_id idempotency.

use memlayer_proto::*;
use memlayer_tests::{connect, spawn_daemon};

async fn start_session(client: &mut memlayer_proto::memlayer_client::MemlayerClient<tonic::transport::Channel>, project: &str, id: &str) {
    client.start_session(StartSessionRequest {
        project_name: project.into(),
        id: id.into(),
        directory: "/tmp".into(),
    }).await.unwrap();
}

#[tokio::test]
async fn ts3_topic_key_upsert() {
    let h = spawn_daemon();
    let mut c = connect(&h).await;
    start_session(&mut c, "p", "s1").await;

    let _ = c.save_observation(SaveObservationRequest {
        project_name: "p".into(),
        sync_id: None,
        session_id: "s1".into(),
        r#type: "decision".into(),
        title: "first".into(),
        content: "first body".into(),
        tool_name: None,
        scope: "project".into(),
        created_by: None,
        topic_key: Some("policy/auth".into()),
        code_anchor: None,
    }).await.unwrap();
    let r2 = c.save_observation(SaveObservationRequest {
        project_name: "p".into(),
        sync_id: None,
        session_id: "s1".into(),
        r#type: "decision".into(),
        title: "second".into(),
        content: "second body".into(),
        tool_name: None,
        scope: "project".into(),
        created_by: None,
        topic_key: Some("policy/auth".into()),
        code_anchor: None,
    }).await.unwrap().into_inner();
    let obs = r2.observation.unwrap();
    assert_eq!(obs.revision_count, 2);
    assert_eq!(obs.title, "second");
}

#[tokio::test]
async fn ts5_sync_id_idempotency() {
    let h = spawn_daemon();
    let mut c = connect(&h).await;
    start_session(&mut c, "p", "s1").await;
    let sync = "fixed-sync-id".to_string();
    let _ = c.save_observation(SaveObservationRequest {
        project_name: "p".into(),
        sync_id: Some(sync.clone()),
        session_id: "s1".into(),
        r#type: "note".into(),
        title: "a".into(),
        content: "first".into(),
        tool_name: None,
        scope: "project".into(),
        created_by: None,
        topic_key: None,
        code_anchor: None,
    }).await.unwrap();
    let r2 = c.save_observation(SaveObservationRequest {
        project_name: "p".into(),
        sync_id: Some(sync.clone()),
        session_id: "s1".into(),
        r#type: "note".into(),
        title: "b".into(),
        content: "second".into(),
        tool_name: None,
        scope: "project".into(),
        created_by: None,
        topic_key: None,
        code_anchor: None,
    }).await.unwrap().into_inner();
    let obs = r2.observation.unwrap();
    assert_eq!(obs.title, "a", "sync_id idempotency: original row returned unchanged");
    assert_eq!(obs.content, "first");
}
