// Generated with AI Coding Rules Hub
//! Integration tests for sync-and-export Spec 3 — git-chunk path.
//!
//! Spec sections: TS-1, TS-2, TS-13 (spec-task-4).
//! TS-3 / TS-4 / TS-5 / TS-6 / TS-16 covered by spec-task-5.
//! TS-12 / TS-15 covered by spec-task-11.
//! TS-19 lives in sync_chaos.rs (concurrency tests).

use std::fs;

use memlayer_proto::*;
use memlayer_tests::{connect, spawn_daemon, CliEnv};

// ---------------------------------------------------------------------------
// TS-1: SyncExport writes a valid compressed chunk (FR1.1, SC-1, SC-2)
// ---------------------------------------------------------------------------

/// Save N observations, call SyncExport.
/// Assert: chunk file exists at the right path, decompressed content has N
/// JSONL lines each with a `_kind` tag, and sha256(uncompressed)[..16] ==
/// returned chunk_id.
#[tokio::test]
async fn ts1_export_writes_valid_chunk() {
    let repo_dir = tempfile::TempDir::new().unwrap();
    let mut env = CliEnv::new().with_project("ts1-export");
    env.spawn_daemon();
    let mut client = connect_cli(&env).await;

    // Seed: start a session + save 3 observations.
    let session_id = uuid::Uuid::new_v4().to_string();
    client
        .start_session(StartSessionRequest {
            project_name: "ts1-export".into(),
            id: session_id.clone(),
            directory: env.data_path().to_string_lossy().into(),
        })
        .await
        .unwrap();

    for i in 0..3 {
        client
            .save_observation(SaveObservationRequest {
                project_name: "ts1-export".into(),
                sync_id: Some(uuid::Uuid::new_v4().to_string()),
                session_id: session_id.clone(),
                r#type: "fact".into(),
                title: format!("obs-{i}"),
                content: format!("content-{i}"),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    // Export.
    let resp = client
        .sync_export(SyncExportRequest {
            project_name: "ts1-export".into(),
            repo_path: Some(repo_dir.path().to_string_lossy().into()),
        })
        .await
        .unwrap()
        .into_inner();

    let cid = resp.chunk_id.expect("chunk_id must be present for non-empty export");
    assert_eq!(cid.len(), 16, "chunk_id must be 16 hex chars");
    assert!(cid.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(resp.observations_exported, 3);
    assert_eq!(resp.sessions_exported, 1);

    // Chunk file must exist.
    let chunk_path = repo_dir
        .path()
        .join(".memlayer")
        .join("chunks")
        .join(format!("{cid}.jsonl.zst"));
    assert!(chunk_path.exists(), "chunk file not found at {}", chunk_path.display());
    assert!(chunk_path.metadata().unwrap().len() > 0, "chunk file is empty");

    // Decompress and parse JSONL.
    let compressed = fs::read(&chunk_path).unwrap();
    let raw = zstd::decode_all(compressed.as_slice()).expect("zstd decompress");
    let text = String::from_utf8(raw).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // 3 observations + 1 session = 4 lines (prompts=0 seeded).
    assert_eq!(lines.len(), 4, "expected 4 JSONL lines, got: {}", lines.len());
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("valid JSON line");
        assert!(v.get("_kind").is_some(), "missing _kind discriminator: {line}");
    }

    // chunk_id must match sha256(uncompressed)[..16 hex chars].
    let re_compressed = fs::read(&chunk_path).unwrap();
    let uncompressed = zstd::decode_all(re_compressed.as_slice()).unwrap();
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(&uncompressed);
    let expected_cid = hex::encode(&hash[..8]);
    assert_eq!(
        cid, expected_cid,
        "chunk_id mismatch: returned={cid} computed={expected_cid}"
    );

    // Manifest must be updated.
    let manifest_path = repo_dir.path().join(".memlayer").join("manifest.json");
    assert!(manifest_path.exists(), "manifest.json not written");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    let chunks = manifest["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0]["chunk_id"].as_str().unwrap(), cid);
}

// ---------------------------------------------------------------------------
// TS-2: Second SyncExport is idempotent (FR1.2, SC-2, EC-1)
// ---------------------------------------------------------------------------

/// After a successful export, a second call with no new unexported rows must
/// return chunk_id = None and must not write a new file.
#[tokio::test]
async fn ts2_export_idempotent() {
    let repo_dir = tempfile::TempDir::new().unwrap();
    let mut env = CliEnv::new().with_project("ts2-idem");
    env.spawn_daemon();
    let mut client = connect_cli(&env).await;

    let session_id = uuid::Uuid::new_v4().to_string();
    client
        .start_session(StartSessionRequest {
            project_name: "ts2-idem".into(),
            id: session_id.clone(),
            directory: env.data_path().to_string_lossy().into(),
        })
        .await
        .unwrap();
    client
        .save_observation(SaveObservationRequest {
            project_name: "ts2-idem".into(),
            sync_id: Some(uuid::Uuid::new_v4().to_string()),
            session_id: session_id.clone(),
            r#type: "fact".into(),
            title: "once".into(),
            content: "only exported once".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // First export → must succeed.
    let r1 = client
        .sync_export(SyncExportRequest {
            project_name: "ts2-idem".into(),
            repo_path: Some(repo_dir.path().to_string_lossy().into()),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(r1.chunk_id.is_some(), "first export must return a chunk_id");

    let chunks_dir = repo_dir.path().join(".memlayer").join("chunks");
    let files_after_first: Vec<_> = fs::read_dir(&chunks_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(files_after_first.len(), 1, "expected exactly one chunk after first export");

    // Second export — same project, same repo_path, no new rows.
    let r2 = client
        .sync_export(SyncExportRequest {
            project_name: "ts2-idem".into(),
            repo_path: Some(repo_dir.path().to_string_lossy().into()),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        r2.chunk_id, None,
        "second export must return chunk_id = None (no unexported rows)"
    );
    assert_eq!(r2.observations_exported, 0);
    assert_eq!(r2.sessions_exported, 0);
    assert_eq!(r2.prompts_exported, 0);

    // No new files.
    let files_after_second: Vec<_> = fs::read_dir(&chunks_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        files_after_second.len(),
        1,
        "second export must not write a new chunk file"
    );
}

// ---------------------------------------------------------------------------
// TS-13: Multi-project export — filesystem isolation (SC-15)
// ---------------------------------------------------------------------------

/// Three projects, three sequential SyncExport calls with separate repo dirs.
/// Assert each project's chunk lands in its own repo, no cross-contamination.
#[tokio::test]
async fn ts13_export_all_no_cross_contamination() {
    let repo_a = tempfile::TempDir::new().unwrap();
    let repo_b = tempfile::TempDir::new().unwrap();
    let repo_c = tempfile::TempDir::new().unwrap();

    let h = spawn_daemon();
    let mut client = connect(&h).await;

    for (proj, repo) in [
        ("ts13-alpha", repo_a.path()),
        ("ts13-beta", repo_b.path()),
        ("ts13-gamma", repo_c.path()),
    ] {
        let session_id = uuid::Uuid::new_v4().to_string();
        client
            .start_session(StartSessionRequest {
                project_name: proj.into(),
                id: session_id.clone(),
                directory: "/tmp/ts13".into(),
            })
            .await
            .unwrap();
        client
            .save_observation(SaveObservationRequest {
                project_name: proj.into(),
                sync_id: Some(uuid::Uuid::new_v4().to_string()),
                session_id,
                r#type: "fact".into(),
                title: format!("{proj}-obs"),
                content: format!("{proj}-content"),
                ..Default::default()
            })
            .await
            .unwrap();

        let r = client
            .sync_export(SyncExportRequest {
                project_name: proj.into(),
                repo_path: Some(repo.to_string_lossy().into()),
            })
            .await
            .unwrap()
            .into_inner();
        assert!(r.chunk_id.is_some(), "{proj}: expected a chunk_id");
    }

    // Each repo must have exactly one chunk, and they must not share IDs.
    let mut all_chunk_ids: Vec<String> = Vec::new();
    for repo in [&repo_a, &repo_b, &repo_c] {
        let chunks_dir = repo.path().join(".memlayer").join("chunks");
        let files: Vec<_> = fs::read_dir(&chunks_dir)
            .unwrap_or_else(|_| panic!("chunks_dir missing at {}", chunks_dir.display()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(files.len(), 1, "each repo must have exactly 1 chunk");
        let name = files[0].file_name().into_string().unwrap();
        let cid = name.strip_suffix(".jsonl.zst").unwrap().to_string();
        all_chunk_ids.push(cid);
    }
    // All chunk_ids must be distinct (different data → different hash).
    let unique: std::collections::HashSet<_> = all_chunk_ids.iter().collect();
    assert_eq!(unique.len(), 3, "chunk_ids must be distinct across projects: {:?}", all_chunk_ids);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn connect_cli(env: &CliEnv) -> memlayer_proto::memlayer_client::MemlayerClient<tonic::transport::Channel> {
    use tonic::transport::{Endpoint, Uri};
    let sock = env.socket_path();
    let ep = Endpoint::try_from("http://[::1]:50051")
        .unwrap()
        .connect_timeout(std::time::Duration::from_secs(2));
    let path = sock.clone();
    ep.connect_with_connector(tower::service_fn(move |_: Uri| {
        let p = path.clone();
        async move {
            let s = tokio::net::UnixStream::connect(p).await?;
            Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
        }
    }))
    .await
    .map(memlayer_proto::memlayer_client::MemlayerClient::new)
    .expect("connect to daemon socket")
}
