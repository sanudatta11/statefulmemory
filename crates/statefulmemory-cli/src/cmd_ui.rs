//! `statefulmemory ui` — loopback web dashboard for browsable memory.
//!
//! Serves an embedded HTML/JS app (see `ui.html`) on 127.0.0.1 (`--port`,
//! default 4687). Every API call talks to the daemon through the same UDS
//! gRPC channel the CLI uses (auto-spawns). The server binds the loopback
//! interface only and prints its URL; no auth token because the listener
//! never leaves the machine.

// The tiny-http serve loop handles one request at a time; the Mutex only ever
// guards the single gRPC client against the one RPC future. Holding the guard
// across `await` is benign here (no second request can interleave).
#![allow(clippy::await_holding_lock)]

use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use tiny_http::{Header, Method, Request as HReq, Response, Server, StatusCode};

use statefulmemory_client::{channel, StatefulMemoryClient};

use crate::cli::UiArgs;
use crate::cmd_daemon::default_autospawn_config;
use crate::exit;

const UI_HTML: &str = include_str!("ui.html");

type Client = StatefulMemoryClient<tonic::transport::Channel>;

pub async fn dispatch(project_flag: Option<String>, args: &UiArgs) -> ExitCode {
    if !matches!(args.host.as_str(), "127.0.0.1" | "::1" | "localhost") {
        eprintln!(
            "statefulmemory: refusing to bind {}. statefulmemory UI is loopback-only; use a reverse proxy for LAN exposure.",
            args.host,
        );
        return ExitCode::from(exit::USAGE);
    }

    let project = {
        let env_override = std::env::var("STATEFULMEMORY_PROJECT")
            .ok()
            .filter(|s| !s.is_empty());
        let cwd = match std::env::current_dir() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("statefulmemory: getcwd: {e}");
                return ExitCode::from(exit::GENERAL);
            }
        };
        match crate::project_detect::detect_in(&cwd, env_override, project_flag) {
            Ok(d) => d.normalized,
            Err(e) => {
                eprintln!("statefulmemory: {e}");
                return ExitCode::from(e.exit_code());
            }
        }
    };

    let socket = statefulmemory_core::paths::socket_path();
    let cfg = default_autospawn_config();
    if let Err(e) = crate::autospawn::ensure_running(&cfg).await {
        eprintln!("statefulmemory: {e}");
        return ExitCode::from(e.exit_code());
    }
    let channel = match channel::connect_uds(&socket) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("statefulmemory: {e}");
            return ExitCode::from(exit::DAEMON_UNREACHABLE);
        }
    };
    let client = Arc::new(Mutex::new(Client::new(channel)));
    let addr = format!("{}:{}", args.host, args.port);
    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("statefulmemory: ui bind {addr}: {e}");
            return ExitCode::from(exit::GENERAL);
        }
    };
    println!("statefulmemory UI → http://{addr}     hit Ctrl-C to stop");
    // tiny-http's blocking recv loop must not run inside the CLI's async
    // context; give it a dedicated std thread owning its own runtime so
    // `block_on` for RPCs is safe.
    let thread = std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(_) => return,
        };
        rt.block_on(serve_loop(client, server, project));
    });
    let _ = thread.join();
    ExitCode::SUCCESS
}

async fn serve_loop(client: Arc<Mutex<Client>>, server: Server, project: String) {
    loop {
        let mut request = match server.recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        let method = request.method().clone();
        let url = request.url().to_string();

        match (method, url.as_str()) {
            (Method::Get, "/") => respond_html(request, UI_HTML),
            (Method::Get, "/favicon.ico") => {
                let _ =
                    request.respond(Response::from_string("").with_status_code(StatusCode(204)));
            }
            (Method::Post, "/api/search") => {
                let q = read_json_field(&mut request, "query").unwrap_or_default();
                let body = search(Arc::clone(&client), &project, &q).await;
                respond_json(request, &body);
            }
            (Method::Post, "/api/graph") => {
                let entity = read_json_field(&mut request, "entity").unwrap_or_default();
                let hops = read_json_field(&mut request, "hops")
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(1)
                    .min(2);
                let body = graph(Arc::clone(&client), &project, &entity, hops).await;
                respond_json(request, &body);
            }
            (Method::Post, "/api/stats") => {
                let body = stats(Arc::clone(&client), &project).await;
                respond_json(request, &body);
            }
            (Method::Post, "/api/decide") => {
                let question = read_json_field(&mut request, "question").unwrap_or_default();
                let body = decide(Arc::clone(&client), &project, &question).await;
                respond_json(request, &body);
            }
            _ => {
                let _ = request
                    .respond(Response::from_string("not found").with_status_code(StatusCode(404)));
            }
        }
    }
}

fn read_body(request: &mut HReq) -> String {
    let mut s = String::new();
    request
        .as_reader()
        .read_to_string(&mut s)
        .unwrap_or_default();
    s
}

fn read_json_field(request: &mut HReq, field: &str) -> Option<String> {
    let body = read_body(request);
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    v.get(field).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn respond_html(request: HReq, html: &str) {
    let resp = Response::from_string(html)
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
        )
        .with_status_code(StatusCode(200));
    let _ = request.respond(resp);
}

fn respond_json(request: HReq, json: &str) {
    let resp = Response::from_string(json)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap())
        .with_status_code(StatusCode(200));
    let _ = request.respond(resp);
}

async fn search(client: Arc<Mutex<Client>>, project: &str, query: &str) -> String {
    if query.trim().is_empty() {
        return r#"{"hits":[],"error":"query required"}"#.into();
    }
    match client
        .lock()
        .unwrap()
        .search_observations(statefulmemory_proto::SearchObservationsRequest {
            project_name: project.into(),
            query: query.into(),
            r#type: None,
            scope: None,
            all_projects: false,
            limit: 15,
            mode: Some("hybrid".into()),
            rerank: None,
            max_tokens: None,
        })
        .await
    {
        Ok(resp) => {
            let obs = resp.into_inner().observations;
            let hits: Vec<serde_json::Value> = obs
                .iter()
                .map(|o| {
                    serde_json::json!({
                        "id": o.id,
                        "type": o.r#type,
                        "title": o.title,
                        "content": o.content.chars().take(160).collect::<String>(),
                        "created_at": o.created_at,
                        "verify_state": o.verify_state.as_deref().unwrap_or(""),
                        "code_anchor": o.code_anchor.as_deref().unwrap_or(""),
                    })
                })
                .collect();
            serde_json::json!({ "hits": hits }).to_string()
        }
        Err(e) => serde_json::json!({ "hits": [], "error": e.message() }).to_string(),
    }
}

async fn graph(client: Arc<Mutex<Client>>, project: &str, entity: &str, hops: u8) -> String {
    if entity.trim().is_empty() {
        return r#"{"entities":[],"edges":[],"error":"entity required"}"#.into();
    }
    let list = match client
        .lock()
        .unwrap()
        .list_entities(statefulmemory_proto::ListEntitiesRequest {
            project_name: project.into(),
            norm_prefix: entity.to_lowercase(),
            kind_filter: String::new(),
            limit: 20,
        })
        .await
    {
        Ok(r) => r.into_inner().entities,
        Err(e) => return serde_json::json!({ "error": e.message() }).to_string(),
    };
    let ent = list
        .iter()
        .find(|e| e.norm_name == entity.to_lowercase())
        .or_else(|| list.first());
    let Some(ent) = ent else {
        return serde_json::json!({ "entities": [], "edges": [], "error": "entity not found" })
            .to_string();
    };
    let id = ent.id;
    match client
        .lock()
        .unwrap()
        .graph_query(statefulmemory_proto::GraphQueryRequest {
            project_name: project.into(),
            entity_id: id,
            hops: hops as u32,
            edge_types: vec![],
            limit: 64,
            relation_filter: String::new(),
        })
        .await
    {
        Ok(resp) => {
            let r = resp.into_inner();
            let entities: Vec<serde_json::Value> = r
                .entities
                .iter()
                .map(|e| serde_json::json!({ "id": e.id, "kind": e.kind, "name": e.name }))
                .collect();
            let edges: Vec<serde_json::Value> = r
                .edges
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "source": e.from_id,
                        "target": e.to_id,
                        "relation": e.relation,
                        "weight": e.weight,
                    })
                })
                .collect();
            serde_json::json!({ "seed": ent.name, "entities": entities, "edges": edges })
                .to_string()
        }
        Err(e) => serde_json::json!({ "error": e.message() }).to_string(),
    }
}

async fn stats(client: Arc<Mutex<Client>>, project: &str) -> String {
    match client
        .lock()
        .unwrap()
        .dream_scan(statefulmemory_proto::DreamScanRequest {
            project_name: project.into(),
        })
        .await
    {
        Ok(r) => {
            let r = r.into_inner();
            serde_json::json!({
                "observations_scanned": r.observations_scanned,
                "consolidation_proposals": r.proposals.len(),
            })
            .to_string()
        }
        Err(e) => serde_json::json!({ "error": e.message() }).to_string(),
    }
}

async fn decide(client: Arc<Mutex<Client>>, project: &str, question: &str) -> String {
    if question.trim().is_empty() {
        return r#"{"answer":"question required"}"#.into();
    }
    match client
        .lock()
        .unwrap()
        .decide(statefulmemory_proto::DecideRequest {
            project_name: project.into(),
            question: question.into(),
            ..Default::default()
        })
        .await
    {
        Ok(r) => {
            let r = r.into_inner();
            serde_json::json!({
                "answer": r.recommendation,
                "reasoning": r.rationale,
                "confidence": r.confidence,
                "hits": r.evidence.len(),
            })
            .to_string()
        }
        Err(e) => serde_json::json!({ "error": e.message() }).to_string(),
    }
}
