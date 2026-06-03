//! TS-6: FTS search + soft-delete invisibility.

use memlayer_proto::*;
use memlayer_tests::{connect, spawn_daemon};

#[tokio::test]
async fn ts6_search_returns_ranked_hits() {
    let h = spawn_daemon();
    let mut c = connect(&h).await;
    c.start_session(StartSessionRequest {
        project_name: "p".into(),
        id: "s1".into(),
        directory: "/tmp".into(),
    }).await.unwrap();

    for body in &[
        "the auth strategy is JWT",
        "we use postgres for storage",
        "the auth flow uses OAuth in v2",
    ] {
        c.save_observation(SaveObservationRequest {
            project_name: "p".into(),
            sync_id: None,
            session_id: "s1".into(),
            r#type: "note".into(),
            title: "x".into(),
            content: (*body).into(),
            tool_name: None,
            scope: "project".into(),
            created_by: None,
            topic_key: None,
        }).await.unwrap();
    }

    let res = c.search_observations(SearchObservationsRequest {
        project_name: "p".into(),
        query: "auth".into(),
        r#type: None,
        scope: None,
        all_projects: false,
        limit: 10,
    }).await.unwrap().into_inner();
    assert_eq!(res.observations.len(), 2, "expected 2 hits for 'auth'");
}

#[tokio::test]
async fn ts10_soft_deleted_invisible() {
    let h = spawn_daemon();
    let mut c = connect(&h).await;
    c.start_session(StartSessionRequest {
        project_name: "p".into(),
        id: "s1".into(),
        directory: "/tmp".into(),
    }).await.unwrap();
    let saved = c.save_observation(SaveObservationRequest {
        project_name: "p".into(),
        sync_id: None,
        session_id: "s1".into(),
        r#type: "note".into(),
        title: "x".into(),
        content: "alpha-unique-marker".into(),
        tool_name: None,
        scope: "project".into(),
        created_by: None,
        topic_key: None,
    }).await.unwrap().into_inner().observation.unwrap();
    c.delete_observation(DeleteObservationRequest {
        project_name: "p".into(),
        key: Some(delete_observation_request::Key::Id(saved.id)),
        hard: false,
    }).await.unwrap();
    let res = c.search_observations(SearchObservationsRequest {
        project_name: "p".into(),
        query: "alpha-unique-marker".into(),
        r#type: None,
        scope: None,
        all_projects: false,
        limit: 10,
    }).await.unwrap().into_inner();
    assert_eq!(res.observations.len(), 0);
}
