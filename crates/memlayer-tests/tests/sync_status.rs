// Generated with AI Coding Rules Hub
//! Spec sections: TS-10, SC-12, FR3.1.
//!
//! spec-task-3 scope: live `SyncStatus` computation reads on-disk manifest
//! and `sync_chunks` rows. Exporting and importing are wired in spec-task-4
//! / spec-task-5; here we drive the read path directly by pre-seeding the
//! per-project `config.json` (with `repo_path`) and a stub `manifest.json`
//! before the daemon starts. Spec-task-6 will replace the hand-written
//! fixtures with a real export → import cycle.

use std::fs;

use memlayer_proto::*;
use memlayer_tests::{connect, spawn_daemon};

/// Fresh project: every field is zero/None.
#[tokio::test]
async fn ts10_status_fresh_project_all_zero() {
    let h = spawn_daemon();
    let mut client = connect(&h).await;

    let session_id = uuid::Uuid::new_v4().to_string();
    client
        .start_session(StartSessionRequest {
            project_name: "ts10-fresh".into(),
            id: session_id,
            directory: "/tmp/ts10-fresh".into(),
        })
        .await
        .unwrap();

    let resp = client
        .sync_status(SyncStatusRequest {
            project_name: Some("ts10-fresh".into()),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(resp.last_export_at, None);
    assert_eq!(resp.last_import_at, None);
    assert_eq!(resp.unseen_chunk_count, 0);
    assert_eq!(resp.last_error, None);
    assert_eq!(resp.total_exported_chunks, 0);
    assert_eq!(resp.total_imported_chunks, 0);
}

/// With a pre-seeded config.json + manifest.json, SyncStatus reads live values.
///
/// Pre-seeds all files before starting the daemon to avoid any write-ordering
/// races between the test process and the daemon process.
#[tokio::test]
async fn ts10_status_reads_manifest_length() {
    let data_dir = tempfile::TempDir::new().unwrap();
    let repo_root = tempfile::TempDir::new().unwrap();

    let normalized = "ts10-manifest";

    // Pre-seed project dir + config.json before the daemon starts.
    // ProjectRegistry::get_or_open reads config.json when it already exists.
    let project_dir = data_dir.path().join("projects").join(normalized);
    fs::create_dir_all(&project_dir).unwrap();
    let config = serde_json::json!({
        "project_display_name": normalized,
        "created_at": "2026-06-04T00:00:00Z",
        "repo_path": repo_root.path(),
    });
    fs::write(
        project_dir.join("config.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();

    // Pre-seed manifest with two chunks.
    fs::create_dir_all(repo_root.path().join(".memlayer")).unwrap();
    let manifest = serde_json::json!({
        "version": 1,
        "chunks": [
            {"chunk_id": "chunk-aaa", "created_at": "2026-06-04T00:00:00Z",
             "observations": 1, "sessions": 0, "prompts": 0},
            {"chunk_id": "chunk-bbb", "created_at": "2026-06-04T00:01:00Z",
             "observations": 2, "sessions": 0, "prompts": 0}
        ]
    });
    fs::write(
        repo_root.path().join(".memlayer").join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Spawn the daemon against the pre-seeded data dir.
    let bin = memlayer_tests::locate_binary();
    let mut child = std::process::Command::new(&bin)
        .args(["daemon", "start", "--foreground"])
        .env("MEMLAYER_DATA_DIR", data_dir.path())
        .env("MEMLAYER_LOG", "warn")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn daemon");

    // Wait for the socket then open a gRPC connection.
    use tonic::transport::{Endpoint, Uri};
    let sock = data_dir.path().join("daemon.sock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !sock.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "daemon socket never appeared");

    let sock_path = sock.clone();
    let ep = Endpoint::try_from("http://[::1]:50051")
        .unwrap()
        .connect_timeout(std::time::Duration::from_secs(2));
    let channel = {
        let mut ch = None;
        let dl = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < dl {
            let sp = sock_path.clone();
            let r = ep
                .clone()
                .connect_with_connector(tower::service_fn(move |_: Uri| {
                    let p = sp.clone();
                    async move {
                        let s = tokio::net::UnixStream::connect(p).await?;
                        Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
                    }
                }))
                .await;
            match r {
                Ok(c) => {
                    ch = Some(c);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        ch.expect("could not connect to daemon within 5s")
    };
    let mut client = memlayer_proto::memlayer_client::MemlayerClient::new(channel);

    // Trigger project open in the registry (creates the DB file).
    let session_id = uuid::Uuid::new_v4().to_string();
    client
        .start_session(StartSessionRequest {
            project_name: normalized.into(),
            id: session_id,
            directory: "/tmp/ts10-manifest".into(),
        })
        .await
        .unwrap();

    let resp = client
        .sync_status(SyncStatusRequest {
            project_name: Some(normalized.into()),
        })
        .await
        .unwrap()
        .into_inner();

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(resp.total_exported_chunks, 2, "manifest has 2 chunks");
    assert_eq!(resp.unseen_chunk_count, 2, "none imported yet");
    assert_eq!(resp.total_imported_chunks, 0);
    assert_eq!(resp.last_import_at, None);
    assert_eq!(resp.last_error, None);
}
