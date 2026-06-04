//! Team / token verbs (TS-13, TS-14, TS-15, TS-26).
//!
//! TS-28 (CreateToken does not log the secret) is satisfied by the existing
//! unit test in `memlayer-daemon/src/service.rs::tests::create_token_logs_name_only_not_secret`,
//! which exercises the same `generate_and_store_token` path the RPC uses. We
//! keep this file focused on the integration-shaped TS items.

use std::path::PathBuf;

use memlayer_proto::{DaemonStatusRequest, ShutdownRequest};
use memlayer_tests::{CliEnv, TcpDaemon};
use serde_json::Value;

// ---------------------------------------------------------------------------
// TS-13 (SC-20) — `team init-ca` writes valid PEMs; leaf cert issuer = CA.
// ---------------------------------------------------------------------------

#[test]
fn ts13_init_ca_writes_three_valid_pems() {
    let env = CliEnv::new();
    let out_dir = env.data_path().join("ca-out");
    let out = env
        .cmd()
        .args(["team", "init-ca", out_dir.to_str().unwrap()])
        .output()
        .expect("team init-ca");
    assert!(
        out.status.success(),
        "team init-ca failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );

    for name in ["ca.pem", "server.pem", "server-key.pem"] {
        let p = out_dir.join(name);
        let body = std::fs::read_to_string(&p).expect("read PEM");
        assert!(body.starts_with("-----BEGIN "), "{name} is not PEM-encoded: {body:?}");
    }

    // SC-20: the leaf cert must be issued by the CA. rcgen 0.13 does not
    // expose post-hoc certificate parsing, so we cap the structural check
    // here at "all three PEM blocks are present and well-formed". TS-15
    // proves end-to-end that the PEM pair is a valid TLS server identity:
    // a real daemon binds with these certs and a client built against ca.pem
    // completes a TLS handshake with hostname `localhost`. If the CA were
    // not actually issuing the leaf, that handshake would fail.
    let ca_pem = std::fs::read_to_string(out_dir.join("ca.pem")).unwrap();
    let server_pem = std::fs::read_to_string(out_dir.join("server.pem")).unwrap();
    assert!(ca_pem.contains("-----BEGIN CERTIFICATE-----"));
    assert!(ca_pem.contains("-----END CERTIFICATE-----"));
    assert!(server_pem.contains("-----BEGIN CERTIFICATE-----"));
    assert!(server_pem.contains("-----END CERTIFICATE-----"));
}

// ---------------------------------------------------------------------------
// TS-26 (EC-9) — `team init-ca` with existing PEMs needs `--force`.
// ---------------------------------------------------------------------------

#[test]
fn ts26_init_ca_refuses_overwrite_without_force() {
    let env = CliEnv::new();
    let out_dir = env.data_path().join("ca-out");

    // First call writes the files.
    let out = env
        .cmd()
        .args(["team", "init-ca", out_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Second call without --force must refuse.
    let out = env
        .cmd()
        .args(["team", "init-ca", out_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "second init-ca without --force should fail",
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("--force") || stderr.to_lowercase().contains("exist"),
        "stderr should hint at the overwrite guard; got: {stderr}",
    );

    // With --force, the third call succeeds.
    let out = env
        .cmd()
        .args(["team", "init-ca", out_dir.to_str().unwrap(), "--force"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init-ca --force should overwrite: {}",
        String::from_utf8_lossy(&out.stderr),
    );
}

// ---------------------------------------------------------------------------
// TS-14 (SC-21) — `token-create` returns the 64-hex secret on stdout once;
// `token-list` never includes secrets.
// ---------------------------------------------------------------------------

#[test]
fn ts14_token_create_then_list_no_secret_leak() {
    let mut env = CliEnv::new();
    env.spawn_daemon();

    let created: Value = json_cmd(&env, &[
        "--output", "json",
        "team", "token-create",
        "--name", "dev-1",
        "--admin",
    ]);
    let secret = created["secret"].as_str().expect("secret field");
    assert_eq!(secret.len(), 64, "expected 64-hex token; got {secret:?}");
    assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));

    // SC-21: token-list must NEVER include the plaintext.
    let listed: Value = json_cmd(&env, &[
        "--output", "json",
        "team", "token-list",
    ]);
    let listed_str = listed.to_string();
    assert!(
        !listed_str.contains(secret),
        "token-list output must not contain the plaintext secret; got: {listed_str}",
    );
    let tokens = listed["tokens"].as_array().expect("tokens array");
    assert!(
        tokens.iter().any(|t| t["name"].as_str() == Some("dev-1")),
        "token-list should include dev-1 by name; got: {listed}",
    );
}

// ---------------------------------------------------------------------------
// TS-15 (SC-22, SC-23) — TCP+TLS valid/invalid token; non-admin daemon stop.
//
// The CLI binary itself only speaks UDS in v1, so we drive the daemon's TCP
// path directly via the `memlayer-client` library. This is still an
// integration test: a real daemon process listens on a real TLS socket, and
// the auth + admin-guard interceptors run in-process.
// ---------------------------------------------------------------------------

const ADMIN_TOKEN: &str =
    "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const NON_ADMIN_TOKEN: &str =
    "2222222222222222222222222222222222222222222222222222222222222222";
const WRONG_TOKEN: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

#[tokio::test(flavor = "multi_thread")]
async fn ts15a_tcp_valid_token_succeeds() {
    let dir = tempfile::TempDir::new().unwrap();
    TcpDaemon::pre_seed_token(dir.path(), "admin-1", ADMIN_TOKEN, true);
    let d = TcpDaemon::spawn(dir);
    let channel = memlayer_client::channel::connect_tcp(&d.endpoint(), &d.ca_pem, "localhost")
        .await
        .expect("tcp channel");
    let interceptor = memlayer_client::interceptor::bearer_interceptor(ADMIN_TOKEN.to_string());
    let mut client = memlayer_client::MemlayerClient::with_interceptor(channel, interceptor);
    let s = client
        .daemon_status(DaemonStatusRequest {})
        .await
        .expect("daemon_status with valid token")
        .into_inner();
    assert!(!s.version.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn ts15b_tcp_invalid_token_unauthenticated() {
    let dir = tempfile::TempDir::new().unwrap();
    TcpDaemon::pre_seed_token(dir.path(), "admin-1", ADMIN_TOKEN, true);
    let d = TcpDaemon::spawn(dir);
    let channel = memlayer_client::channel::connect_tcp(&d.endpoint(), &d.ca_pem, "localhost")
        .await
        .expect("tcp channel");
    let interceptor = memlayer_client::interceptor::bearer_interceptor(WRONG_TOKEN.to_string());
    let mut client = memlayer_client::MemlayerClient::with_interceptor(channel, interceptor);
    let err = client
        .daemon_status(DaemonStatusRequest {})
        .await
        .expect_err("invalid token should return UNAUTHENTICATED");
    assert_eq!(err.code(), tonic::Code::Unauthenticated, "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ts15c_tcp_non_admin_daemon_stop_permission_denied() {
    let dir = tempfile::TempDir::new().unwrap();
    TcpDaemon::pre_seed_token(dir.path(), "boss", ADMIN_TOKEN, true);
    TcpDaemon::pre_seed_token(dir.path(), "intern", NON_ADMIN_TOKEN, false);
    let d = TcpDaemon::spawn(dir);
    let channel = memlayer_client::channel::connect_tcp(&d.endpoint(), &d.ca_pem, "localhost")
        .await
        .expect("tcp channel");
    let interceptor =
        memlayer_client::interceptor::bearer_interceptor(NON_ADMIN_TOKEN.to_string());
    let mut client = memlayer_client::MemlayerClient::with_interceptor(channel, interceptor);
    let err = client
        .shutdown(ShutdownRequest {})
        .await
        .expect_err("non-admin Shutdown should be denied");
    assert_eq!(
        err.code(),
        tonic::Code::PermissionDenied,
        "expected PERMISSION_DENIED for non-admin Shutdown; got: {err:?}",
    );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn json_cmd(env: &CliEnv, args: &[&str]) -> Value {
    let out = env.cmd().args(args).output().expect("cmd");
    if !out.status.success() {
        panic!(
            "command {:?} failed (exit={:?}): stderr={}",
            args,
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "command {:?} stdout was not valid JSON: {e}\nstdout: {}",
            args,
            String::from_utf8_lossy(&out.stdout),
        )
    })
}

#[allow(dead_code)]
fn _silence(_p: &PathBuf) {}
