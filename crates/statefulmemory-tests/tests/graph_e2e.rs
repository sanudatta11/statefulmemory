//! E2E: entity-graph save-index → rebuild → reindex → export → wipe → import.
//!
//! Covers the full graph lifecycle the CI workflows only sample:
//! - save-path `IndexGraph` (graph.enabled) produces entities/mentions/edges
//! - `graph rebuild` is idempotent (anchor backfill adds nothing twice)
//! - `obs reindex` (embed path) never mutates the graph
//! - `graph query` RPC round-trips edges (hops + relation filter)
//! - `mem export` reports exact graph counts == `graph stats`
//! - wipe + `mem import --mode merge` restores entities/mentions/edges with
//!   relation map intact, query still walks edges, rebuild stays idempotent
//!
//! Live daemon required (integration crate pattern).

use std::process::Command;

use serde_json::Value;
use statefulmemory_proto::{
    GetEntityRequest, GraphQueryRequest, ListEntitiesRequest, SaveObservationRequest,
    StartSessionRequest,
};
use statefulmemory_tests::{connect, spawn_daemon, start_session, CliEnv};

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

fn run_ok(env: &CliEnv, args: &[&str]) {
    let out = env.cmd().args(args).output().expect("cmd");
    assert!(
        out.status.success(),
        "command {:?} failed: stdout={}\nstderr={}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// `graph stats` reads the project DB directly (daemon-independent).
fn stats(env: &CliEnv) -> Value {
    json_cmd(env, &["--output", "json", "graph", "stats"])
}

/// Total counts as a comparable tuple (entities, mentions, edges, relation map).
fn totals(s: &Value) -> (i64, i64, i64, Value) {
    (
        s["total_entities"].as_i64().expect("total_entities"),
        s["total_mentions"].as_i64().expect("total_mentions"),
        s["total_edges"].as_i64().expect("total_edges"),
        s["edges_by_relation"].clone(),
    )
}

fn init_git_fixture(env: &CliEnv) {
    let dir = env.data_path();
    let git = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        let st = statefulmemory_core::process::output_with_timeout(
            &mut cmd,
            std::time::Duration::from_secs(30),
        )
        .expect("git");
        assert!(
            st.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&st.stderr)
        );
    };
    git(&["init", "-q", "."]);
    git(&["config", "user.email", "e2e@ci"]);
    git(&["config", "user.name", "e2e"]);
    std::fs::write(dir.join("main.rs"), "fn main(){}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
}

fn graph_query(env: &CliEnv, entity: &str, extra: &[&str]) -> Value {
    let mut args = vec!["graph", "query", entity, "--json"];
    args.extend_from_slice(extra);
    json_cmd(env, &args)
}

#[test]
fn graph_save_rebuild_reindex_export_import_e2e() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    init_git_fixture(&env);

    // graph.enabled BEFORE saves — save-path IndexGraph must run.
    run_ok(&env, &["config", "set", "graph.enabled", "true"]);

    let session = uuid_short();
    start_session(&env, &session);

    // Three observations sharing backtick entities + anchored symbols
    // (mirrors the graph-smoke fixture).
    for (title, content, anchor) in [
        (
            "use validate before refresh",
            "auth pipeline: run `validate` then `refresh` token",
            "main.rs::validate_mw",
        ),
        (
            "refresh conn backoff",
            "on 5xx `validate` retries with backoff `refresh`",
            "main.rs::backoff",
        ),
        (
            "rate limit caps",
            "cap `validate` calls at 100/min, `refresh` at 20/min",
            "main.rs::limit",
        ),
    ] {
        run_ok(&env, &[
            "obs", "save", "--type", "decision",
            "--title", title,
            "--content", content,
            "--anchor", anchor,
            "--session", &session,
        ]);
    }

    // --- save-path graph index (no manual rebuild yet) ---
    let s0 = stats(&env);
    let (e0, m0, x0, rel0) = totals(&s0);
    assert!(e0 >= 3, "save-path IndexGraph must create entities, got {e0}");
    assert!(m0 >= 3, "mentions required, got {m0}");
    assert!(x0 >= 1, "pairwise mention edges required, got {x0}");
    assert!(
        rel0.as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "edges_by_relation must be populated: {rel0}"
    );

    // --- rebuild idempotence ---
    run_ok(&env, &["graph", "rebuild"]);
    let s1 = stats(&env);
    run_ok(&env, &["graph", "rebuild"]);
    let s2 = stats(&env);
    assert_eq!(
        totals(&s1),
        totals(&s2),
        "graph rebuild must be idempotent (entities/mentions/edges/relations)"
    );

    // --- obs reindex never mutates the graph (embed pool may be absent;
    //     either outcome is fine — the invariant is graph stability) ---
    let _ = env
        .cmd()
        .args(["obs", "reindex", "--force"])
        .output()
        .expect("obs reindex");
    let s3 = stats(&env);
    assert_eq!(
        totals(&s3),
        totals(&s2),
        "obs reindex must not touch entities/mentions/edges"
    );

    // --- graph query over the live daemon (hops + relation filter) ---
    let q1 = graph_query(&env, "validate", &["--hops", "1"]);
    assert!(
        q1["edges"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "hops=1 query must return edges: {q1}"
    );
    assert!(
        q1["entities"].as_array().map(|a| a.len() >= 2).unwrap_or(false),
        "seed + neighbor entities expected: {q1}"
    );
    let q2 = graph_query(&env, "validate", &["--hops", "2"]);
    assert!(
        q2["entities"].as_array().expect("entities").len()
            >= q1["entities"].as_array().expect("entities").len(),
        "hops=2 must cover hops=1"
    );
    let qf = graph_query(&env, "validate", &["--hops", "2", "--relation", "mentions"]);
    assert!(
        qf["edges"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "--relation mentions must return edges: {qf}"
    );
    for e in qf["edges"].as_array().expect("filtered edges") {
        assert_eq!(
            e["relation"].as_str(),
            Some("mentions"),
            "--relation filter leaked a non-mentions edge: {e}"
        );
    }
    // The fixture only ever creates `mentions` edges — a filter for another
    // relation must come back EMPTY. This is the non-vacuous half: if the
    // server ignores relation_filter, mentions edges leak through and fail.
    let qf2 = graph_query(&env, "validate", &["--hops", "2", "--relation", "fixes"]);
    assert!(
        qf2["edges"].as_array().map(|a| a.is_empty()).unwrap_or(false),
        "--relation fixes must return zero edges (graph only has mentions): {qf2}"
    );

    // hops above the server cap clamp to 2 — same answer as an explicit 2.
    let q99 = graph_query(&env, "validate", &["--hops", "99"]);
    assert_eq!(
        q99, q2,
        "--hops 99 must clamp to 2 (identical response to --hops 2)"
    );

    // Unknown entity fails with a clear error (exit + stderr, no JSON).
    let miss = env
        .cmd()
        .args(["graph", "query", "definitely-no-such-entity-zzz", "--json"])
        .output()
        .expect("cmd");
    assert!(
        !miss.status.success(),
        "query for a missing entity must fail, got exit 0"
    );
    let miss_err = String::from_utf8_lossy(&miss.stderr);
    assert!(
        miss_err.contains("entity not found"),
        "stderr must say entity not found, got: {miss_err}"
    );

    // --- export reports exact graph counts ---
    let archive = env.data_path().join("graph-e2e.mem");
    let archive_s = archive.to_string_lossy().into_owned();
    let exp = json_cmd(
        &env,
        &["--output", "json", "mem", "export", "--out", &archive_s],
    );
    assert!(archive.exists(), "archive written");
    assert_eq!(exp["entities"].as_i64(), Some(e0), "export entities == stats");
    // rebuild may have added anchor-file entities after s0 — compare to latest
    let (e_now, m_now, x_now, rel_now) = totals(&s3);
    assert_eq!(exp["entities"].as_i64(), Some(e_now), "export entities == live stats");
    assert_eq!(exp["mentions"].as_i64(), Some(m_now), "export mentions == live stats");
    assert_eq!(exp["edges"].as_i64(), Some(x_now), "export edges == live stats");

    // --- wipe + import ---
    run_ok(&env, &["daemon", "stop"]);
    let projects = env.data_path().join("projects");
    if projects.exists() {
        std::fs::remove_dir_all(&projects).expect("wipe projects");
    }
    let global = env.data_path().join("global.sqlite");
    if global.exists() {
        std::fs::remove_file(&global).expect("wipe global.sqlite");
    }
    let imp = json_cmd(
        &env,
        &[
            "--output", "json", "mem", "import", &archive_s, "--mode", "merge",
        ],
    );
    assert_eq!(
        imp["entities_imported"].as_i64(),
        Some(e_now),
        "import entity count"
    );
    assert_eq!(
        imp["mentions_imported"].as_i64(),
        Some(m_now),
        "import mention count"
    );
    assert_eq!(
        imp["edges_imported"].as_i64(),
        Some(x_now),
        "import edge count"
    );

    // --- post-import graph fidelity: counts + relation map + live traversal ---
    let s4 = stats(&env);
    assert_eq!(
        totals(&s4),
        (e_now, m_now, x_now, rel_now),
        "graph stats after import must equal pre-export stats exactly"
    );
    let q3 = graph_query(&env, "validate", &["--hops", "2"]);
    assert!(
        q3["edges"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "edges must survive import remap: {q3}"
    );

    // --- rebuild after import stays idempotent ---
    run_ok(&env, &["graph", "rebuild"]);
    let s5 = stats(&env);
    assert_eq!(
        totals(&s5),
        totals(&s4),
        "post-import rebuild must not duplicate graph rows"
    );

    // --- observations survived too ---
    let srch = json_cmd(
        &env,
        &["--output", "json", "obs", "search", "rate limit", "--mode", "hybrid"],
    );
    let has_hit = srch
        .get("observations")
        .and_then(|r| r.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    assert!(has_hit, "search after import must return hits: {srch}");

    run_ok(&env, &["daemon", "stop"]);
}

/// RPC-level graph edge cases the CLI path cannot reach:
/// - server-side hop clamp (99 → 2, 0 → 1) compares equal responses
/// - `ListEntities` empty prefix = all + limit, kind_filter, unknown kind error
/// - `GetEntity` missing id errors; `GraphQuery` unknown seed → empty
/// - `relation_filter` narrows server-side (fixes on a mentions-only graph → ∅)
#[tokio::test]
async fn graph_rpc_edge_cases() {
    let handle = spawn_daemon();
    // Global config BEFORE first save: save-path IndexGraph is gated on
    // `graph.enabled`, re-resolved per save (spawn happens first — fine).
    std::fs::write(
        handle.data_dir.path().join("config.toml"),
        "[graph]\nenabled = true\n",
    )
    .expect("write global config");
    let mut client = connect(&handle).await;
    let project = "rpc-graph";

    client
        .start_session(StartSessionRequest {
            project_name: project.into(),
            id: "s1".into(),
            directory: "/tmp".into(),
        })
        .await
        .expect("start_session");

    // Anchors → file + symbol entities; backticks → concept entities.
    // Save RPC blocks until IndexGraph finishes (grx awaited in service).
    for (title, content, anchor) in [
        ("t1", "run `validate` now", "src/main.rs::boot"),
        ("t2", "then `refresh`", "src/main.rs::step"),
    ] {
        client
            .save_observation(SaveObservationRequest {
                project_name: project.into(),
                sync_id: None,
                session_id: "s1".into(),
                r#type: "note".into(),
                title: title.into(),
                content: content.into(),
                tool_name: None,
                scope: "project".into(),
                created_by: None,
                topic_key: None,
                code_anchor: None,
                anchors: vec![anchor.into()],
            })
            .await
            .expect("save_observation");
    }

    // Empty prefix = all entities, limit applies (ListEntities proto contract).
    let all = client
        .list_entities(ListEntitiesRequest {
            project_name: project.into(),
            norm_prefix: String::new(),
            kind_filter: String::new(),
            limit: 2,
        })
        .await
        .expect("list_entities")
        .into_inner();
    assert_eq!(
        all.entities.len(),
        2,
        "empty prefix + limit=2 must return exactly 2 of the seeded entities"
    );

    // kind_filter narrows to `file` (anchors produce file entities).
    let files = client
        .list_entities(ListEntitiesRequest {
            project_name: project.into(),
            norm_prefix: String::new(),
            kind_filter: "file".into(),
            limit: 50,
        })
        .await
        .expect("list_entities kind=file")
        .into_inner();
    assert!(!files.entities.is_empty(), "anchor file entities expected");
    for e in &files.entities {
        assert_eq!(e.kind, "file", "kind_filter leaked: {e:?}");
    }

    // Unknown kind → invalid_argument.
    let bad_kind = client
        .list_entities(ListEntitiesRequest {
            project_name: project.into(),
            norm_prefix: String::new(),
            kind_filter: "banana".into(),
            limit: 10,
        })
        .await;
    assert!(bad_kind.is_err(), "unknown kind_filter must error");

    // GetEntity missing id → error (not an empty success).
    let missing = client
        .get_entity(GetEntityRequest {
            project_name: project.into(),
            id: 999_999,
        })
        .await;
    assert!(missing.is_err(), "GetEntity for a missing id must error");

    // Real seed for graph_query clamps.
    let named = client
        .list_entities(ListEntitiesRequest {
            project_name: project.into(),
            norm_prefix: "validate".into(),
            kind_filter: String::new(),
            limit: 10,
        })
        .await
        .expect("list validate")
        .into_inner();
    let seed_id = named.entities[0].id;
    let q = |hops: u32, entity_id: i64, relation_filter: &str| {
        GraphQueryRequest {
            project_name: project.into(),
            entity_id,
            hops,
            edge_types: Vec::new(),
            limit: 64,
            relation_filter: relation_filter.into(),
        }
    };

    let q2 = client
        .graph_query(q(2, seed_id, ""))
        .await
        .expect("hops=2")
        .into_inner();
    assert!(
        !q2.edges.is_empty(),
        "mentions edges expected around the seed"
    );

    let q99 = client
        .graph_query(q(99, seed_id, ""))
        .await
        .expect("hops=99")
        .into_inner();
    assert_eq!(q99, q2, "server must clamp hops 99 → 2");

    let q0 = client
        .graph_query(q(0, seed_id, ""))
        .await
        .expect("hops=0")
        .into_inner();
    let q1 = client
        .graph_query(q(1, seed_id, ""))
        .await
        .expect("hops=1")
        .into_inner();
    assert_eq!(q0, q1, "server must clamp hops 0 → 1");

    // Unknown seed id: empty response, not an error.
    let qm = client
        .graph_query(q(2, 999_999, ""))
        .await
        .expect("unknown seed must not error")
        .into_inner();
    assert!(
        qm.entities.is_empty() && qm.edges.is_empty(),
        "unknown seed must return empty graph: {qm:?}"
    );

    // relation_filter narrows server-side: only `mentions` edges exist.
    let fixes = client
        .graph_query(q(2, seed_id, "fixes"))
        .await
        .expect("relation filter")
        .into_inner();
    assert!(
        fixes.edges.is_empty(),
        "relation_filter=fixes must return no edges on a mentions-only graph: {fixes:?}"
    );
    let mentions = client
        .graph_query(q(2, seed_id, "mentions"))
        .await
        .expect("relation filter mentions")
        .into_inner();
    assert_eq!(
        mentions.edges.len(),
        q2.edges.len(),
        "relation_filter=mentions must match the unfiltered default on this graph"
    );
    for e in &mentions.edges {
        assert_eq!(e.relation, "mentions");
    }
}

fn uuid_short() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}
