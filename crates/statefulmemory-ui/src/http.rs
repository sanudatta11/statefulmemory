use std::io::Read;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, ResponseBox, StatusCode};

use crate::backend::{normalize_name, BackendError, ObsRef, SaveObservationInput, UpdateObservationInput};
use crate::{
    AuthFailure, UiState, MAX_BODY_BYTES, MAX_HEADER_BYTES, MAX_URL_BYTES, SESSION_COOKIE,
};

const CSP: &[u8] = b"default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'";
const MAX_TEXT_CHARS: usize = 4096;
const MAX_CONTENT_CHARS: usize = 16 * 1024;
const MAX_ENTITY_CHARS: usize = 256;
const SEARCH_DEFAULT_LIMIT: i32 = 15;
const SEARCH_MAX_LIMIT: i32 = 50;
const DECIDE_DEFAULT_LIMIT: i32 = 12;
const DECIDE_MAX_LIMIT: i32 = 30;
const CONTEXT_DEFAULT_LIMIT: i32 = 10;
const CONTEXT_MAX_LIMIT: i32 = 50;
const RECENT_DEFAULT_LIMIT: i32 = 20;
const RECENT_MAX_LIMIT: i32 = 100;
const SESSION_DEFAULT_LIMIT: i32 = 50;
const SESSION_MAX_LIMIT: i32 = 200;
const TIMELINE_DEFAULT_SPAN: i32 = 5;
const TIMELINE_MAX_SPAN: i32 = 50;
const MAX_ANCHORS: usize = 32;

#[derive(Debug)]
struct ApiError {
    status: u16,
    code: &'static str,
    message: String,
    allow: Option<&'static str>,
}

impl ApiError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            allow: None,
        }
    }

    fn method(allow: &'static str) -> Self {
        Self {
            status: 405,
            code: "method_not_allowed",
            message: "method not allowed".to_string(),
            allow: Some(allow),
        }
    }

    fn response(self) -> ResponseBox {
        let mut response = error_response(self.status, self.code, &self.message);
        if let Some(allow) = self.allow {
            response = response.with_header(make_header(b"Allow", allow.as_bytes()));
        }
        response
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Route {
    Root,
    Favicon,
    AppScript,
    AppStyle,
    Search,
    Graph,
    Stats,
    Decide,
    Projects,
    ProjectMemories,
    Meta,
    EvalScorecard,
    Observation,
    RetrievalExplain,
    Jobs,
    Sync,
    // Mutations
    ObsSave,
    ObsUpdate,
    ObsDelete,
    // Retrieval / lifecycle
    ObsRecent,
    ObsFacts,
    ObsHistory,
    Context,
    Timeline,
    Verify,
    // Sessions
    SessionList,
    SessionGet,
    SessionSummary,
    SessionDelete,
    SessionStart,
    SessionEnd,
    // Project ops
    ProjectList,
    ProjectCurrent,
    ProjectMerge,
    ProjectDelete,
    ProjectConsolidate,
    ProjectPrune,
    // Daemon ops
    OpsDoctor,
    OpsStats,
    OpsDaemonStatus,
    OpsReextract,
    OpsReindex,
    Visualization(VisualizationRoute),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VisualizationRoute {
    Overview,
    Entities,
    Graph,
    Timeline,
}

const VISUALIZATION_ROUTES: &[(&str, VisualizationRoute)] = &[
    ("/api/visualization/overview", VisualizationRoute::Overview),
    ("/api/visualization/entities", VisualizationRoute::Entities),
    ("/api/visualization/graph", VisualizationRoute::Graph),
    ("/api/visualization/timeline", VisualizationRoute::Timeline),
];

struct RequestParts {
    method: Method,
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
    content_length: Option<usize>,
    remote_loopback: bool,
}

pub(crate) async fn handle_request(state: Arc<UiState>, request: &mut Request) -> ResponseBox {
    let mut parts = match inspect_request(request) {
        Ok(parts) => parts,
        Err(error) => return error.response(),
    };
    let content_length = match validate_transport(&parts, &state) {
        Ok(content_length) => content_length,
        Err(error) => return error.response(),
    };
    parts.content_length = content_length;
    let route = match classify_route(&parts.method, &parts.path) {
        Ok(route) => route,
        Err(error) => return error.response(),
    };

    match route {
        Route::Root => handle_root(&state, &parts),
        Route::Favicon => {
            if parts.query.is_some() {
                return ApiError::new(400, "invalid_request", "query is not allowed").response();
            }
            base_response(204, "", Vec::new())
        }
        Route::AppScript => {
            if parts.query.is_some() {
                return ApiError::new(400, "invalid_request", "query is not allowed").response();
            }
            if state.script.is_empty() {
                return ApiError::new(404, "not_found", "asset not found").response();
            }
            base_response(
                200,
                "text/javascript; charset=utf-8",
                state.script.as_bytes().to_vec(),
            )
        }
        Route::AppStyle => {
            if parts.query.is_some() {
                return ApiError::new(400, "invalid_request", "query is not allowed").response();
            }
            if state.style.is_empty() {
                return ApiError::new(404, "not_found", "asset not found").response();
            }
            base_response(
                200,
                "text/css; charset=utf-8",
                state.style.as_bytes().to_vec(),
            )
        }
        Route::Projects => {
            if let Err(error) = require_session(&state, &parts) {
                return error.response();
            }
            projects_route(&state).await
        }
        Route::ProjectMemories => {
            if let Err(error) = require_session(&state, &parts) {
                return error.response();
            }
            memories_route(&state, &parts).await
        }
        Route::Meta => {
            if parts.query.is_some() {
                return ApiError::new(400, "invalid_request", "query is not allowed").response();
            }
            if let Err(error) = require_session(&state, &parts) {
                return error.response();
            }
            meta_route(&state)
        }
        Route::EvalScorecard => {
            if parts.query.is_some() {
                return ApiError::new(400, "invalid_request", "query is not allowed").response();
            }
            if let Err(error) = require_session(&state, &parts) {
                return error.response();
            }
            let cwd = std::env::current_dir().unwrap_or_default();
            eval_scorecard_route(&cwd)
        }
        Route::Search
        | Route::Graph
        | Route::Stats
        | Route::Decide
        | Route::Observation
        | Route::RetrievalExplain
        | Route::Jobs
        | Route::Sync
        | Route::ObsSave
        | Route::ObsUpdate
        | Route::ObsDelete
        | Route::ObsRecent
        | Route::ObsFacts
        | Route::ObsHistory
        | Route::Context
        | Route::Timeline
        | Route::Verify
        | Route::SessionList
        | Route::SessionGet
        | Route::SessionSummary
        | Route::SessionDelete
        | Route::SessionStart
        | Route::SessionEnd
        | Route::ProjectList
        | Route::ProjectCurrent
        | Route::ProjectMerge
        | Route::ProjectDelete
        | Route::ProjectConsolidate
        | Route::ProjectPrune
        | Route::OpsDoctor
        | Route::OpsStats
        | Route::OpsDaemonStatus
        | Route::OpsReextract
        | Route::OpsReindex
        | Route::Visualization(_) => {
            if parts.query.is_some() {
                return ApiError::new(400, "invalid_request", "query is not allowed").response();
            }
            if let Err(error) = require_session(&state, &parts) {
                return error.response();
            }
            if let Err(error) = require_json_content_type(&parts) {
                return error.response();
            }
            let body = match read_body(request, &parts) {
                Ok(body) => body,
                Err(error) => return error.response(),
            };
            match route {
                Route::Search => search_route(&state, &body).await,
                Route::Graph => graph_route(&state, &body).await,
                Route::Stats => stats_route(&state, &body).await,
                Route::Decide => decide_route(&state, &body).await,
                Route::Observation => observation_route(&state, &body).await,
                Route::RetrievalExplain => retrieval_explain_route(&state, &body).await,
                Route::Jobs => jobs_route(&state, &body).await,
                Route::Sync => sync_route(&state, &body).await,
                Route::ObsSave => obs_save_route(&state, &body).await,
                Route::ObsUpdate => obs_update_route(&state, &body).await,
                Route::ObsDelete => obs_delete_route(&state, &body).await,
                Route::ObsRecent => recent_route(&state, &body).await,
                Route::ObsFacts => facts_route(&state, &body).await,
                Route::ObsHistory => history_route(&state, &body).await,
                Route::Context => context_route(&state, &body).await,
                Route::Timeline => timeline_route(&state, &body).await,
                Route::Verify => verify_route(&state, &body).await,
                Route::SessionList => session_list_route(&state, &body).await,
                Route::SessionGet => session_get_route(&state, &body).await,
                Route::SessionSummary => session_summary_route(&state, &body).await,
                Route::SessionDelete => session_delete_route(&state, &body).await,
                Route::SessionStart => session_start_route(&state, &body).await,
                Route::SessionEnd => session_end_route(&state, &body).await,
                Route::ProjectList => project_list_route(&state, &body).await,
                Route::ProjectCurrent => current_project_route(&state, &body).await,
                Route::ProjectMerge => merge_route(&state, &body).await,
                Route::ProjectDelete => project_delete_route(&state, &body).await,
                Route::ProjectConsolidate => consolidate_route(&state, &body).await,
                Route::ProjectPrune => prune_route(&state, &body).await,
                Route::OpsDoctor => doctor_route(&state, &body).await,
                Route::OpsStats => ops_stats_route(&state, &body).await,
                Route::OpsDaemonStatus => daemon_status_route(&state, &body).await,
                Route::OpsReextract => reextract_route(&state, &body).await,
                Route::OpsReindex => reindex_route(&state, &body).await,
                Route::Visualization(kind) => visualization_route(&state, &body, kind).await,
                Route::Root
                | Route::Favicon
                | Route::AppScript
                | Route::AppStyle
                | Route::Projects
                | Route::ProjectMemories
                | Route::Meta
                | Route::EvalScorecard => unreachable!(),
            }
        }
    }
}

fn inspect_request(request: &Request) -> Result<RequestParts, ApiError> {
    let url = request.url();
    if url.len() > MAX_URL_BYTES {
        return Err(ApiError::new(
            414,
            "uri_too_long",
            "request URI is too long",
        ));
    }
    if url.contains('#') {
        return Err(ApiError::new(400, "invalid_request", "invalid request URI"));
    }
    let mut pieces = url.splitn(2, '?');
    let path = pieces.next().unwrap_or_default().to_string();
    let query = pieces.next().map(str::to_string);
    if !path.starts_with('/') || path.is_empty() {
        return Err(ApiError::new(
            400,
            "invalid_request",
            "invalid request path",
        ));
    }

    let mut headers = Vec::new();
    let mut header_bytes = 0_usize;
    for header in request.headers() {
        let field: String = header.field.as_str().as_str().to_ascii_lowercase();
        let value = header.value.as_str().to_owned();
        header_bytes = header_bytes
            .saturating_add(field.len())
            .saturating_add(value.len());
        headers.push((field, value));
    }
    if header_bytes > MAX_HEADER_BYTES {
        return Err(ApiError::new(
            431,
            "headers_too_large",
            "request headers are too large",
        ));
    }
    Ok(RequestParts {
        method: request.method().clone(),
        path,
        query,
        headers,
        content_length: None,
        remote_loopback: request
            .remote_addr()
            .map(|address| address.ip().is_loopback())
            .unwrap_or(false),
    })
}

fn validate_transport(parts: &RequestParts, state: &UiState) -> Result<Option<usize>, ApiError> {
    if !parts.remote_loopback {
        return Err(ApiError::new(403, "forbidden", "loopback client required"));
    }
    let host = single_header(parts, "host")?
        .ok_or_else(|| ApiError::new(400, "missing_host", "Host header required"))?;
    if host != state.config.authority() {
        return Err(ApiError::new(
            403,
            "forbidden",
            "Host header is not allowed",
        ));
    }
    if header_present(parts, "transfer-encoding") {
        return Err(ApiError::new(
            400,
            "invalid_request",
            "transfer encoding is not supported",
        ));
    }
    if header_present(parts, "expect") {
        return Err(ApiError::new(
            417,
            "expectation_failed",
            "Expect is not supported",
        ));
    }
    if header_present(parts, "content-encoding") {
        return Err(ApiError::new(
            415,
            "unsupported_media_type",
            "content encoding is not supported",
        ));
    }
    if let Some(connection) = single_header(parts, "connection")? {
        if connection
            .split(',')
            .any(|value| value.trim().eq_ignore_ascii_case("upgrade"))
        {
            return Err(ApiError::new(
                400,
                "invalid_request",
                "connection upgrade is not allowed",
            ));
        }
    }
    let content_length = match single_header(parts, "content-length")? {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| ApiError::new(400, "invalid_request", "invalid Content-Length"))?;
            if parsed > MAX_BODY_BYTES {
                return Err(ApiError::new(
                    413,
                    "body_too_large",
                    "request body is too large",
                ));
            }
            Some(parsed)
        }
        None => None,
    };
    let origin = single_header(parts, "origin")?;
    if let Some(origin) = origin {
        if origin != state.config.origin() {
            return Err(ApiError::new(403, "forbidden", "Origin is not allowed"));
        }
    } else if parts.path.starts_with("/api/") && parts.method != Method::Get {
        return Err(ApiError::new(
            403,
            "origin_required",
            "Origin header required",
        ));
    }
    let _ = single_header(parts, "content-type")?;
    let _ = single_header(parts, "cookie")?;
    let _ = single_header(parts, "host")?;
    Ok(content_length)
}

fn classify_route(method: &Method, path: &str) -> Result<Route, ApiError> {
    let (expected, route): (&Method, Route) = match path {
        "/" => (&Method::Get, Route::Root),
        "/favicon.ico" => (&Method::Get, Route::Favicon),
        "/assets/app.js" => (&Method::Get, Route::AppScript),
        "/assets/app.css" => (&Method::Get, Route::AppStyle),
        "/api/search" => (&Method::Post, Route::Search),
        "/api/graph" => (&Method::Post, Route::Graph),
        "/api/stats" => (&Method::Post, Route::Stats),
        "/api/decide" => (&Method::Post, Route::Decide),
        "/api/observation" => (&Method::Post, Route::Observation),
        "/api/retrieval/explain" => (&Method::Post, Route::RetrievalExplain),
        "/api/jobs" => (&Method::Post, Route::Jobs),
        "/api/sync" => (&Method::Post, Route::Sync),
        "/api/observations/save" => (&Method::Post, Route::ObsSave),
        "/api/observations/update" => (&Method::Post, Route::ObsUpdate),
        "/api/observations/delete" => (&Method::Post, Route::ObsDelete),
        "/api/observations/recent" => (&Method::Post, Route::ObsRecent),
        "/api/observations/facts" => (&Method::Post, Route::ObsFacts),
        "/api/observations/history" => (&Method::Post, Route::ObsHistory),
        "/api/context" => (&Method::Post, Route::Context),
        "/api/timeline" => (&Method::Post, Route::Timeline),
        "/api/verify" => (&Method::Post, Route::Verify),
        "/api/sessions/list" => (&Method::Post, Route::SessionList),
        "/api/sessions/get" => (&Method::Post, Route::SessionGet),
        "/api/sessions/summary" => (&Method::Post, Route::SessionSummary),
        "/api/sessions/delete" => (&Method::Post, Route::SessionDelete),
        "/api/sessions/start" => (&Method::Post, Route::SessionStart),
        "/api/sessions/end" => (&Method::Post, Route::SessionEnd),
        "/api/projects/list" => (&Method::Post, Route::ProjectList),
        "/api/projects/current" => (&Method::Post, Route::ProjectCurrent),
        "/api/projects/merge" => (&Method::Post, Route::ProjectMerge),
        "/api/projects/delete" => (&Method::Post, Route::ProjectDelete),
        "/api/projects/consolidate" => (&Method::Post, Route::ProjectConsolidate),
        "/api/projects/prune" => (&Method::Post, Route::ProjectPrune),
        "/api/ops/doctor" => (&Method::Post, Route::OpsDoctor),
        "/api/ops/stats" => (&Method::Post, Route::OpsStats),
        "/api/ops/daemon-status" => (&Method::Post, Route::OpsDaemonStatus),
        "/api/ops/reextract" => (&Method::Post, Route::OpsReextract),
        "/api/ops/reindex" => (&Method::Post, Route::OpsReindex),
        "/api/projects" => (&Method::Get, Route::Projects),
        "/api/meta" => (&Method::Get, Route::Meta),
        "/api/eval/scorecard" => (&Method::Get, Route::EvalScorecard),
        _ if is_project_memories_path(path) => (&Method::Get, Route::ProjectMemories),
        _ => {
            if let Some((_, kind)) = VISUALIZATION_ROUTES
                .iter()
                .find(|(candidate, _)| *candidate == path)
            {
                (&Method::Post, Route::Visualization(*kind))
            } else {
                return Err(ApiError::new(404, "not_found", "route not found"));
            }
        }
    };
    if method != expected {
        return Err(ApiError::method(match *expected {
            Method::Get => "GET",
            Method::Post => "POST",
            _ => "GET, POST",
        }));
    }
    Ok(route)
}

fn is_project_memories_path(path: &str) -> bool {
    let Some(project) = path.strip_prefix("/api/projects/") else {
        return false;
    };
    let Some(project) = project.strip_suffix("/memories") else {
        return false;
    };
    !project.is_empty() && !project.contains('/')
}

fn handle_root(state: &UiState, parts: &RequestParts) -> ResponseBox {
    let launch = match parse_launch_query(parts.query.as_deref()) {
        Ok(launch) => launch,
        Err(error) => return error.response(),
    };
    if let Some(launch) = launch {
        return match state.auth.redeem_launch(&launch.token) {
            Ok(session) => {
                let cookie = format!(
                    "{SESSION_COOKIE}={session}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict",
                    state.auth.session_ttl_secs()
                );
                let location = launch
                    .project
                    .as_deref()
                    .map(|project| format!("/?project={}", encode_query_component(project)))
                    .unwrap_or_else(|| "/".to_string());
                base_response(303, "text/plain; charset=utf-8", Vec::new())
                    .with_header(make_header(b"Location", location.as_bytes()))
                    .with_header(make_header(b"Set-Cookie", cookie.as_bytes()))
            }
            Err(AuthFailure::InvalidLaunch) => {
                ApiError::new(401, "unauthorized", "launch capability is invalid").response()
            }
            Err(AuthFailure::LaunchUsed) => {
                ApiError::new(401, "unauthorized", "launch capability was already used").response()
            }
        };
    }
    match session_cookie(parts) {
        Ok(Some(cookie)) if state.auth.session_valid(&cookie) => base_response(
            200,
            "text/html; charset=utf-8",
            state.html.as_bytes().to_vec(),
        ),
        Ok(_) => ApiError::new(401, "unauthorized", "valid session token required").response(),
        Err(error) => error.response(),
    }
}

fn require_session(state: &UiState, parts: &RequestParts) -> Result<(), ApiError> {
    let cookie = session_cookie(parts)?;
    if cookie.is_some_and(|value| state.auth.session_valid(&value)) {
        Ok(())
    } else {
        Err(ApiError::new(
            401,
            "unauthorized",
            "valid session token required",
        ))
    }
}

fn session_cookie(parts: &RequestParts) -> Result<Option<String>, ApiError> {
    let Some(raw) = single_header(parts, "cookie")? else {
        return Ok(None);
    };
    let mut found = None;
    for item in raw.split(';') {
        let Some((name, value)) = item.split_once('=') else {
            continue;
        };
        if name.trim() == SESSION_COOKIE {
            if found.is_some() {
                return Err(ApiError::new(
                    400,
                    "invalid_request",
                    "duplicate session cookie",
                ));
            }
            let value = value.trim();
            if value.is_empty()
                || value.len() > 256
                || value.bytes().any(|byte| !(0x21..=0x7e).contains(&byte))
            {
                return Err(ApiError::new(
                    400,
                    "invalid_request",
                    "invalid session cookie",
                ));
            }
            found = Some(value.to_string());
        }
    }
    Ok(found)
}

struct LaunchQuery {
    token: String,
    project: Option<String>,
}

fn parse_launch_query(query: Option<&str>) -> Result<Option<LaunchQuery>, ApiError> {
    let Some(query) = query else {
        return Ok(None);
    };
    if query.is_empty() {
        return Err(ApiError::new(
            400,
            "invalid_request",
            "invalid launch query",
        ));
    }
    let mut token = None;
    let mut project = None;
    for item in query.split('&') {
        if item.is_empty() {
            return Err(ApiError::new(
                400,
                "invalid_request",
                "invalid launch query",
            ));
        }
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| ApiError::new(400, "invalid_request", "invalid launch query"))?;
        let decoded = decode_component(value)
            .ok_or_else(|| ApiError::new(400, "invalid_request", "invalid launch query"))?;
        match key {
            "token" | "capability" => {
                if token.is_some() || decoded.is_empty() || decoded.len() > 256 {
                    return Err(ApiError::new(
                        400,
                        "invalid_request",
                        "invalid launch capability",
                    ));
                }
                token = Some(decoded);
            }
            "project" => {
                if project.is_some() || decoded.is_empty() || decoded.len() > 120 {
                    return Err(ApiError::new(
                        400,
                        "invalid_request",
                        "invalid launch project",
                    ));
                }
                project = Some(decoded);
            }
            _ => {
                return Err(ApiError::new(
                    400,
                    "invalid_request",
                    "invalid launch query",
                ));
            }
        }
    }
    Ok(token.map(|token| LaunchQuery { token, project }))
}

fn encode_query_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn decode_component(value: &str) -> Option<String> {
    if !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn require_json_content_type(parts: &RequestParts) -> Result<(), ApiError> {
    let Some(content_type) = single_header(parts, "content-type")? else {
        return Err(ApiError::new(
            415,
            "unsupported_media_type",
            "Content-Type: application/json required",
        ));
    };
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    if !media_type.eq_ignore_ascii_case("application/json") {
        return Err(ApiError::new(
            415,
            "unsupported_media_type",
            "Content-Type: application/json required",
        ));
    }
    Ok(())
}

fn read_body(request: &mut Request, parts: &RequestParts) -> Result<Vec<u8>, ApiError> {
    let mut body = Vec::new();
    let mut reader = request.as_reader().take((MAX_BODY_BYTES + 1) as u64);
    reader
        .read_to_end(&mut body)
        .map_err(|_| ApiError::new(400, "invalid_request", "could not read request body"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::new(
            413,
            "body_too_large",
            "request body is too large",
        ));
    }
    if let Some(expected) = parts.content_length {
        if body.len() != expected {
            return Err(ApiError::new(
                400,
                "invalid_request",
                "request body length mismatch",
            ));
        }
    }
    Ok(body)
}

fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body)
        .map_err(|_| ApiError::new(400, "invalid_json", "request body must be valid JSON"))
}

fn parse_empty_object(body: &[u8]) -> Result<(), ApiError> {
    let value: Value = parse_json(body)?;
    let Some(object) = value.as_object() else {
        return Err(ApiError::new(
            400,
            "invalid_request",
            "JSON object required",
        ));
    };
    if !object.is_empty() {
        return Err(ApiError::new(
            400,
            "invalid_request",
            "request object must be empty",
        ));
    }
    Ok(())
}

async fn search_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SearchBody = match parse_json(body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let query = match required_text(request.query, "query", MAX_TEXT_CHARS) {
        Ok(query) => query,
        Err(error) => return error.response(),
    };
    let limit = match bounded_limit(request.limit, SEARCH_DEFAULT_LIMIT, SEARCH_MAX_LIMIT) {
        Ok(limit) => limit,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state.backend.search(&project, &query, limit).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn graph_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: GraphBody = match parse_json(body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let entity = match required_text(request.entity, "entity", MAX_ENTITY_CHARS) {
        Ok(entity) => entity,
        Err(error) => return error.response(),
    };
    if normalize_name(&entity).is_empty() {
        return ApiError::new(
            400,
            "invalid_request",
            "entity has no searchable characters",
        )
        .response();
    }
    let hops = request.hops.unwrap_or(1).clamp(1, 2);
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state.backend.graph(&project, &entity, hops).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn stats_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ProjectBody = match parse_json(body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state.backend.stats(&project).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn decide_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: DecideBody = match parse_json(body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let question = match required_text(request.question, "question", MAX_TEXT_CHARS) {
        Ok(question) => question,
        Err(error) => return error.response(),
    };
    let limit = match bounded_limit(request.limit, DECIDE_DEFAULT_LIMIT, DECIDE_MAX_LIMIT) {
        Ok(limit) => limit,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state.backend.decide(&project, &question, limit).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn projects_route(state: &UiState) -> ResponseBox {
    match state.backend.project_summaries().await {
        Ok(projects) => json_response(200, json!({ "projects": projects })),
        Err(error) => backend_error(error),
    }
}

/// Read-only transport metadata so the dashboard can label the active source
/// (local UDS vs remote team daemon) and the launch project. No backend call.
fn meta_route(state: &UiState) -> ResponseBox {
    json_response(
        200,
        json!({
            "remote": state.remote,
            "project": state.project_name,
            "authority": state.config.authority(),
        }),
    )
}

/// Serve the most recently written committed eval scorecard JSON (Phase
/// 5.3b), so the dashboard's Eval view can render accuracy/MRR/recall@k/
/// superseded_served_pct/latency without inventing data or requiring a
/// manual file upload. Looks under `<root>/eval/` for `*.json` files
/// (produced by `statefulmemory eval --save-scorecard` and the CI eval gate,
/// see .github/workflows/eval.yml) and `eval/baselines/*.json` (the gate's
/// seeded baseline). No backend/gRPC call — this is a plain local file read,
/// scoped to a fixed relative subdirectory (never user-supplied input, so
/// there is no path-traversal surface). Returns `{"available": false}`
/// (200, not an error) when no scorecard exists yet. `root` is the process
/// cwd in production; tests pass a temp dir so no test touches global
/// process state.
fn eval_scorecard_route(root: &std::path::Path) -> ResponseBox {
    let candidates = [root.join("eval"), root.join("eval").join("baselines")];

    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for dir in &candidates {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            if newest.as_ref().map_or(true, |(t, _)| modified > *t) {
                newest = Some((modified, path));
            }
        }
    }

    let Some((_, path)) = newest else {
        return json_response(200, json!({ "available": false }));
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return json_response(200, json!({ "available": false })),
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return json_response(200, json!({ "available": false })),
    };
    json_response(
        200,
        json!({
            "available": true,
            "source": path.file_name().and_then(|n| n.to_str()).unwrap_or("scorecard.json"),
            "scorecard": parsed,
        }),
    )
}

async fn memories_route(state: &UiState, parts: &RequestParts) -> ResponseBox {
    let project = match project_from_memories_path(&parts.path) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    let limit = match query_limit(parts.query.as_deref(), 50, 50) {
        Ok(limit) => limit,
        Err(error) => return error.response(),
    };
    match state.backend.list_memories(&project, limit).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn observation_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ObservationBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    if request.observation_id <= 0 {
        return ApiError::new(400, "invalid_request", "observation_id must be positive").response();
    }
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .observation_detail(&project, request.observation_id)
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn retrieval_explain_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: RetrievalExplainBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let query = match required_text(request.query, "query", MAX_TEXT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let limit = match bounded_limit(request.limit, SEARCH_DEFAULT_LIMIT, SEARCH_MAX_LIMIT) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let mode = request.mode.unwrap_or_else(|| "hybrid".into());
    if mode != "hybrid" && mode != "bm25" {
        return ApiError::new(400, "invalid_request", "mode must be hybrid or bm25").response();
    }
    let rerank = request.rerank.unwrap_or_default();
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .explain(&project, &query, limit, &mode, &rerank)
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn jobs_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: JobsBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let limit = match bounded_limit(request.limit, 50, 200) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .jobs(
            &project,
            &request.status.unwrap_or_default(),
            &request.kind.unwrap_or_default(),
            limit,
        )
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn sync_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ProjectBody = match parse_json(body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    match state.backend.sync_state(&project).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn obs_save_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SaveBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let obs_type = match required_text(request.r#type, "type", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let title = match required_text(request.title, "title", MAX_TEXT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let content = match required_text(request.content, "content", MAX_CONTENT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let scope = match optional_text(request.scope, "scope", MAX_ENTITY_CHARS) {
        Ok(value) => value.unwrap_or_else(|| "project".to_string()),
        Err(error) => return error.response(),
    };
    let topic_key = match optional_text(request.topic_key, "topic_key", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let tool_name = match optional_text(request.tool_name, "tool_name", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let created_by = match optional_text(request.created_by, "created_by", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let session_id = match optional_text(request.session_id, "session_id", MAX_ENTITY_CHARS) {
        Ok(value) => value.unwrap_or_default(),
        Err(error) => return error.response(),
    };
    let anchors = match validate_anchors(request.anchors) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let input = SaveObservationInput {
        session_id,
        r#type: obs_type,
        title,
        content,
        scope,
        topic_key,
        tool_name,
        created_by,
        anchors,
    };
    match state.backend.save_observation(&project, input).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn obs_update_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: UpdateBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let key = match observation_ref(request.id, request.sync_id) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let title = match optional_text(request.title, "title", MAX_TEXT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let content = match optional_text(request.content, "content", MAX_CONTENT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let topic_key = match optional_text(request.topic_key, "topic_key", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let scope = match optional_text(request.scope, "scope", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let obs_type = match optional_text(request.r#type, "type", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let code_anchor = match optional_text(request.code_anchor, "code_anchor", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    if title.is_none()
        && content.is_none()
        && topic_key.is_none()
        && scope.is_none()
        && obs_type.is_none()
        && code_anchor.is_none()
    {
        return ApiError::new(400, "invalid_request", "no fields to update").response();
    }
    let patch = UpdateObservationInput {
        title,
        content,
        topic_key,
        scope,
        r#type: obs_type,
        code_anchor,
    };
    match state.backend.update_observation(&project, key, patch).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn obs_delete_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: DeleteObsBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let key = match observation_ref(request.id, request.sync_id) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .delete_observation(&project, key, request.hard.unwrap_or(false))
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn recent_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: RecentBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let limit = match bounded_limit(request.limit, RECENT_DEFAULT_LIMIT, RECENT_MAX_LIMIT) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let scope = match optional_text(request.scope, "scope", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .recent_observations(&project, limit, scope)
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn facts_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ObservationIdBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    if request.observation_id <= 0 {
        return ApiError::new(400, "invalid_request", "observation_id must be positive").response();
    }
    match state.backend.get_facts(&project, request.observation_id).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn history_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ObservationIdBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    if request.observation_id <= 0 {
        return ApiError::new(400, "invalid_request", "observation_id must be positive").response();
    }
    match state
        .backend
        .observation_history(&project, request.observation_id)
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn context_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ContextBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let query = match optional_text(request.query, "query", MAX_TEXT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let limit = match bounded_limit(request.limit, CONTEXT_DEFAULT_LIMIT, CONTEXT_MAX_LIMIT) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let mode = request.mode.unwrap_or_else(|| "hybrid".into());
    if mode != "hybrid" && mode != "bm25" {
        return ApiError::new(400, "invalid_request", "mode must be hybrid or bm25").response();
    }
    let max_tokens = match request.max_tokens {
        Some(value) if value < 0 => {
            return ApiError::new(400, "invalid_request", "max_tokens is invalid").response();
        }
        other => other,
    };
    match state
        .backend
        .context(
            &project,
            query,
            limit,
            mode,
            request.include_stale.unwrap_or(false),
            max_tokens,
        )
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn timeline_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: TimelineBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let anchor = match observation_ref(request.id, request.sync_id) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let before = match bounded_span(request.before, TIMELINE_DEFAULT_SPAN, TIMELINE_MAX_SPAN) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let after = match bounded_span(request.after, TIMELINE_DEFAULT_SPAN, TIMELINE_MAX_SPAN) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.timeline(&project, anchor, before, after).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn verify_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: VerifyBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    if let Some(id) = request.observation_id {
        if id <= 0 {
            return ApiError::new(400, "invalid_request", "observation_id must be positive")
                .response();
        }
    }
    match state
        .backend
        .verify_anchors(&project, request.observation_id)
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn session_list_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SessionListBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let limit = match bounded_limit(request.limit, SESSION_DEFAULT_LIMIT, SESSION_MAX_LIMIT) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.list_sessions(&project, limit).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn session_get_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SessionIdBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let id = match required_text(request.id, "id", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.get_session(&project, &id).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn session_summary_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SessionSummaryBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let id = match required_text(request.id, "id", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let summary = match required_text(request.summary, "summary", MAX_CONTENT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .save_session_summary(&project, &id, &summary)
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn session_delete_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SessionIdBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let id = match required_text(request.id, "id", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.delete_session(&project, &id).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn session_start_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SessionStartBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let id = match required_text(request.id, "id", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let directory = match optional_text(request.directory, "directory", MAX_TEXT_CHARS) {
        Ok(value) => value.unwrap_or_default(),
        Err(error) => return error.response(),
    };
    match state.backend.start_session(&project, &id, &directory).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn session_end_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: SessionEndBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let id = match required_text(request.id, "id", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let summary = match optional_text(request.summary, "summary", MAX_CONTENT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.end_session(&project, &id, summary).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn project_list_route(state: &UiState, body: &[u8]) -> ResponseBox {
    if let Err(error) = parse_empty_object(body) {
        return error.response();
    }
    match state.backend.list_projects().await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn current_project_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: CurrentProjectBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let directory = match required_text(request.directory, "directory", MAX_TEXT_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.current_project(&directory).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn merge_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: MergeBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let from = match validate_project_name(request.from) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let to = match validate_project_name(request.to) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.merge_projects(&from, &to).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn project_delete_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: DeleteProjectBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .delete_project(&project, request.hard.unwrap_or(false))
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn consolidate_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: DryRunBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .consolidate_projects(request.dry_run.unwrap_or(true))
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn prune_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: DryRunBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .prune_projects(request.dry_run.unwrap_or(true))
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn doctor_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: DoctorBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match optional_project(request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .doctor(project, request.auto_repair.unwrap_or(false))
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn ops_stats_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: StatsOpsBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match optional_project(request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state.backend.daemon_stats(project).await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn daemon_status_route(state: &UiState, body: &[u8]) -> ResponseBox {
    if let Err(error) = parse_empty_object(body) {
        return error.response();
    }
    match state.backend.daemon_status().await {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn reextract_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ReextractBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let since = match optional_text(request.since, "since", MAX_ENTITY_CHARS) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .reextract_observations(&project, since, request.only_missing.unwrap_or(true))
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn reindex_route(state: &UiState, body: &[u8]) -> ResponseBox {
    let request: ReindexBody = match parse_json(body) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(value) => value,
        Err(error) => return error.response(),
    };
    match state
        .backend
        .reindex_observations(&project, request.force.unwrap_or(false))
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

fn project_from_memories_path(path: &str) -> Result<String, ApiError> {
    let encoded = path
        .strip_prefix("/api/projects/")
        .and_then(|value| value.strip_suffix("/memories"))
        .ok_or_else(|| ApiError::new(404, "not_found", "route not found"))?;
    let project = decode_component(encoded)
        .ok_or_else(|| ApiError::new(400, "invalid_request", "invalid project name"))?;
    if project.contains('/') || project.contains('\\') || project == "." || project == ".." {
        return Err(ApiError::new(
            400,
            "invalid_request",
            "invalid project name",
        ));
    }
    required_text(project, "project", MAX_ENTITY_CHARS)
}

fn query_limit(query: Option<&str>, default: i32, maximum: i32) -> Result<i32, ApiError> {
    let Some(query) = query else {
        return Ok(default);
    };
    let mut limit = default;
    for item in query.split('&') {
        let Some((key, value)) = item.split_once('=') else {
            return Err(ApiError::new(400, "invalid_request", "invalid query"));
        };
        if key != "limit" {
            return Err(ApiError::new(400, "invalid_request", "invalid query"));
        }
        limit = value
            .parse::<i32>()
            .map_err(|_| ApiError::new(400, "invalid_request", "invalid limit"))?;
    }
    bounded_limit(Some(limit), default, maximum)
}

async fn visualization_route(
    state: &UiState,
    body: &[u8],
    kind: VisualizationRoute,
) -> ResponseBox {
    let request: ProjectBody = match parse_json(body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let project = match resolve_project(state, request.project) {
        Ok(project) => project,
        Err(error) => return error.response(),
    };
    let result = match kind {
        VisualizationRoute::Overview => tokio::try_join!(
            state.backend.project_summary(&project),
            state.backend.health(&project),
            state.backend.graph_stats(&project),
            state.backend.sync_state(&project),
            state.backend.jobs(&project, "", "", 50),
        )
        .map(|values| {
            json!({
                "summary": values.0,
                "health": values.1,
                "graph": values.2,
                "sync": values.3,
                "jobs": values.4,
            })
        }),
        VisualizationRoute::Entities => state
            .backend
            .graph_stats(&project)
            .await
            .map(|value| json!({ "graph": value })),
        VisualizationRoute::Graph => state
            .backend
            .graph_stats(&project)
            .await
            .map(|value| json!({ "graph": value })),
        VisualizationRoute::Timeline => state
            .backend
            .jobs(&project, "", "", 50)
            .await
            .map(|value| json!({ "jobs": value })),
    };
    match result {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

fn required_text(value: String, field: &'static str, max_chars: usize) -> Result<String, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::new(
            400,
            "invalid_request",
            format!("{field} is required"),
        ));
    }
    if value.chars().count() > max_chars {
        return Err(ApiError::new(
            400,
            "invalid_request",
            format!("{field} is too long"),
        ));
    }
    if value.contains('\0') {
        return Err(ApiError::new(
            400,
            "invalid_request",
            format!("{field} is invalid"),
        ));
    }
    Ok(trimmed.to_string())
}

fn bounded_limit(value: Option<i32>, default: i32, maximum: i32) -> Result<i32, ApiError> {
    match value {
        None => Ok(default),
        Some(value) if value < 1 => Err(ApiError::new(400, "invalid_request", "limit is invalid")),
        Some(value) => Ok(value.min(maximum)),
    }
}

fn bounded_span(value: Option<i32>, default: i32, maximum: i32) -> Result<i32, ApiError> {
    match value {
        None => Ok(default),
        Some(value) if value < 0 => Err(ApiError::new(400, "invalid_request", "span is invalid")),
        Some(value) => Ok(value.min(maximum)),
    }
}

/// Resolve the project a per-project route operates on: an explicit non-empty
/// `project` in the request body, else the launch-time project. Mirrors how the
/// GET memories route accepts a per-request project while POST routes default to
/// the launch project.
fn resolve_project(state: &UiState, project: Option<String>) -> Result<String, ApiError> {
    match project {
        Some(value) if !value.trim().is_empty() => validate_project_name(value),
        _ => Ok(state.project_name.clone()),
    }
}

/// Like [`resolve_project`] but returns `None` when omitted, for RPCs where an
/// absent project means "all projects" (e.g. Stats, Doctor).
fn optional_project(project: Option<String>) -> Result<Option<String>, ApiError> {
    match project {
        Some(value) if !value.trim().is_empty() => Ok(Some(validate_project_name(value)?)),
        _ => Ok(None),
    }
}

fn validate_project_name(project: String) -> Result<String, ApiError> {
    let trimmed = project.trim();
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed == "." || trimmed == ".." {
        return Err(ApiError::new(400, "invalid_request", "invalid project name"));
    }
    required_text(project, "project", MAX_ENTITY_CHARS)
}

/// Validate an optional free-text field: `None`/blank → `None`, otherwise apply
/// the same checks [`required_text`] performs.
fn optional_text(
    value: Option<String>,
    field: &'static str,
    max_chars: usize,
) -> Result<Option<String>, ApiError> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(Some(required_text(value, field, max_chars)?)),
        _ => Ok(None),
    }
}

/// Build an observation key from a request body, requiring exactly one of
/// numeric `id` or `sync_id` (mirrors the proto `oneof key`).
fn observation_ref(id: Option<i64>, sync_id: Option<String>) -> Result<ObsRef, ApiError> {
    match (id, sync_id) {
        (Some(_), Some(_)) => Err(ApiError::new(
            400,
            "invalid_request",
            "provide either id or sync_id, not both",
        )),
        (Some(id), None) => {
            if id <= 0 {
                return Err(ApiError::new(400, "invalid_request", "id must be positive"));
            }
            Ok(ObsRef::Id(id))
        }
        (None, Some(sync_id)) => Ok(ObsRef::SyncId(required_text(
            sync_id,
            "sync_id",
            MAX_ENTITY_CHARS,
        )?)),
        (None, None) => Err(ApiError::new(
            400,
            "invalid_request",
            "id or sync_id is required",
        )),
    }
}

fn validate_anchors(anchors: Option<Vec<String>>) -> Result<Vec<String>, ApiError> {
    let anchors = anchors.unwrap_or_default();
    if anchors.len() > MAX_ANCHORS {
        return Err(ApiError::new(400, "invalid_request", "too many anchors"));
    }
    let mut validated = Vec::with_capacity(anchors.len());
    for anchor in anchors {
        validated.push(required_text(anchor, "anchor", MAX_ENTITY_CHARS)?);
    }
    Ok(validated)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchBody {
    query: String,
    limit: Option<i32>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphBody {
    entity: String,
    hops: Option<u32>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideBody {
    question: String,
    limit: Option<i32>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationBody {
    observation_id: i64,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetrievalExplainBody {
    query: String,
    limit: Option<i32>,
    mode: Option<String>,
    rerank: Option<String>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobsBody {
    status: Option<String>,
    kind: Option<String>,
    limit: Option<i32>,
    project: Option<String>,
}

/// Body for routes that otherwise take no parameters but must still re-scope on
/// the active project (stats / sync / visualization). An empty `{}` keeps the
/// launch-time project, so existing callers are unaffected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectBody {
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveBody {
    project: Option<String>,
    session_id: Option<String>,
    r#type: String,
    title: String,
    content: String,
    scope: Option<String>,
    topic_key: Option<String>,
    tool_name: Option<String>,
    created_by: Option<String>,
    anchors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    project: Option<String>,
    id: Option<i64>,
    sync_id: Option<String>,
    title: Option<String>,
    content: Option<String>,
    topic_key: Option<String>,
    scope: Option<String>,
    r#type: Option<String>,
    code_anchor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteObsBody {
    project: Option<String>,
    id: Option<i64>,
    sync_id: Option<String>,
    hard: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentBody {
    project: Option<String>,
    limit: Option<i32>,
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationIdBody {
    project: Option<String>,
    observation_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextBody {
    project: Option<String>,
    query: Option<String>,
    limit: Option<i32>,
    mode: Option<String>,
    include_stale: Option<bool>,
    max_tokens: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineBody {
    project: Option<String>,
    id: Option<i64>,
    sync_id: Option<String>,
    before: Option<i32>,
    after: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyBody {
    project: Option<String>,
    observation_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionListBody {
    project: Option<String>,
    limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionIdBody {
    project: Option<String>,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionSummaryBody {
    project: Option<String>,
    id: String,
    summary: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionStartBody {
    project: Option<String>,
    id: String,
    directory: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionEndBody {
    project: Option<String>,
    id: String,
    summary: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentProjectBody {
    directory: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeBody {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteProjectBody {
    project: Option<String>,
    hard: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DryRunBody {
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DoctorBody {
    project: Option<String>,
    auto_repair: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatsOpsBody {
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReextractBody {
    project: Option<String>,
    since: Option<String>,
    only_missing: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReindexBody {
    project: Option<String>,
    force: Option<bool>,
}

fn single_header<'a>(parts: &'a RequestParts, name: &str) -> Result<Option<&'a str>, ApiError> {
    let mut found = None;
    for (field, value) in &parts.headers {
        if field == name {
            if found.is_some() {
                return Err(ApiError::new(
                    400,
                    "invalid_request",
                    "duplicate request header",
                ));
            }
            found = Some(value.as_str());
        }
    }
    Ok(found)
}

fn header_present(parts: &RequestParts, name: &str) -> bool {
    parts.headers.iter().any(|(field, _)| field == name)
}

fn backend_error(error: BackendError) -> ResponseBox {
    let status = error.status.clamp(400, 599);
    error_response(status, &error.code, &error.message)
}

fn json_response(status: u16, value: Value) -> ResponseBox {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| {
        br#"{"error":"internal_error","message":"could not encode response"}"#.to_vec()
    });
    base_response(status, "application/json; charset=utf-8", body)
}

fn error_response(status: u16, code: &str, message: &str) -> ResponseBox {
    let body = serde_json::to_vec(&json!({
        "error": code,
        "message": message,
    }))
    .unwrap_or_else(|_| br#"{"error":"internal_error"}"#.to_vec());
    base_response(status, "application/json; charset=utf-8", body)
}

fn base_response(status: u16, content_type: &str, body: Vec<u8>) -> ResponseBox {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    if !content_type.is_empty() {
        response = response.with_header(make_header(b"Content-Type", content_type.as_bytes()));
    }
    response
        .with_header(make_header(b"Cache-Control", b"no-store"))
        .with_header(make_header(b"Content-Security-Policy", CSP))
        .with_header(make_header(b"X-Content-Type-Options", b"nosniff"))
        .with_header(make_header(b"X-Frame-Options", b"DENY"))
        .with_header(make_header(b"Referrer-Policy", b"no-referrer"))
        .with_header(make_header(
            b"Permissions-Policy",
            b"camera=(), microphone=(), geolocation=()",
        ))
        .with_header(make_header(b"Cross-Origin-Opener-Policy", b"same-origin"))
        .with_header(make_header(b"Cross-Origin-Resource-Policy", b"same-origin"))
        .boxed()
}

fn make_header(name: &'static [u8], value: &[u8]) -> Header {
    Header::from_bytes(name, value.to_vec()).expect("static response header must be ASCII")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;
    use tiny_http::{Header, TestRequest};

    use super::*;
    use crate::backend::UiBackend;
    use crate::{AuthState, UiConfig, UiState};

    struct FakeBackend {
        searches: AtomicUsize,
        saves: AtomicUsize,
    }

    #[async_trait]
    impl UiBackend for FakeBackend {
        async fn search(
            &self,
            _project_name: &str,
            query: &str,
            _limit: i32,
        ) -> Result<Value, BackendError> {
            self.searches.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"hits": [{"query": query}]}))
        }

        async fn graph(
            &self,
            _project_name: &str,
            _entity: &str,
            _hops: u32,
        ) -> Result<Value, BackendError> {
            Ok(json!({"entities": [], "edges": []}))
        }

        async fn stats(&self, _project_name: &str) -> Result<Value, BackendError> {
            Ok(json!({"observations_scanned": 0}))
        }

        async fn decide(
            &self,
            _project_name: &str,
            _question: &str,
            _limit: i32,
        ) -> Result<Value, BackendError> {
            Ok(json!({"answer": "ok"}))
        }

        async fn save_observation(
            &self,
            project_name: &str,
            input: SaveObservationInput,
        ) -> Result<Value, BackendError> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            Ok(json!({
                "observation": {"project": project_name, "title": input.title},
            }))
        }
    }

    fn test_state() -> (Arc<UiState>, Arc<FakeBackend>) {
        let backend = Arc::new(FakeBackend {
            searches: AtomicUsize::new(0),
            saves: AtomicUsize::new(0),
        });
        let state = Arc::new(UiState {
            config: UiConfig::new("127.0.0.1", 4687).unwrap(),
            project_name: "test-project".to_string(),
            auth: AuthState::new().unwrap(),
            backend: backend.clone(),
            remote: false,
            html: "<!doctype html><title>test</title>",
            script: "",
            style: "",
        });
        (state, backend)
    }

    fn header(name: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Header {
        Header::from_bytes(name.as_ref().to_vec(), value.as_ref().to_vec()).unwrap()
    }

    fn response_header<'a>(response: &'a ResponseBox, name: &str) -> Option<&'a str> {
        response
            .headers()
            .iter()
            .find(|candidate| candidate.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|candidate| candidate.value.as_str())
    }

    fn response_body(response: ResponseBox) -> String {
        let mut body = String::new();
        response.into_reader().read_to_string(&mut body).unwrap();
        body
    }

    fn session_cookie_from(response: &ResponseBox) -> String {
        response_header(response, "Set-Cookie")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1
            .to_string()
    }

    fn path_request(path: impl Into<String>) -> TestRequest {
        let path = path.into();
        TestRequest::new().with_path(&path)
    }

    async fn dispatch_test(state: Arc<UiState>, request: TestRequest) -> ResponseBox {
        let mut request: Request = request.into();
        handle_request(state, &mut request).await
    }

    #[tokio::test]
    async fn host_and_origin_checks_are_exact() {
        let (state, _backend) = test_state();
        let request = TestRequest::new()
            .with_path("/")
            .with_header(header("Host", "localhost:4687"));

        let response = dispatch_test(Arc::clone(&state), request).await;
        assert_eq!(response.status_code().0, 403);

        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();
        let request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/search")
            .with_body(r#"{"query":"x"}"#)
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Origin", "http://127.0.0.1:4688"))
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Cookie", format!("{SESSION_COOKIE}={session}")));

        let response = dispatch_test(state, request).await;
        assert_eq!(response.status_code().0, 403);
    }

    #[tokio::test]
    async fn launch_capability_is_single_use_and_sets_session_cookie() {
        let (state, _backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let request =
            path_request(format!("/?token={launch}")).with_header(header("Host", "127.0.0.1:4687"));

        let response = dispatch_test(Arc::clone(&state), request).await;
        assert_eq!(response.status_code().0, 303);
        let cookie = session_cookie_from(&response);
        assert!(cookie.len() >= 64);
        assert!(response_header(&response, "Location").is_some());

        let request =
            path_request(format!("/?token={launch}")).with_header(header("Host", "127.0.0.1:4687"));

        let response = dispatch_test(state, request).await;
        assert_eq!(response.status_code().0, 401);
    }

    #[tokio::test]
    async fn session_api_route_maps_to_backend_with_security_headers() {
        let (state, backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();
        let request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/search")
            .with_body(r#"{"query":"x","limit":9999}"#)
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Origin", "http://127.0.0.1:4687"))
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Cookie", format!("{SESSION_COOKIE}={session}")));

        let response = dispatch_test(state, request).await;
        assert_eq!(response.status_code().0, 200);
        assert_eq!(response_header(&response, "X-Frame-Options"), Some("DENY"));
        assert_eq!(
            response_header(&response, "X-Content-Type-Options"),
            Some("nosniff")
        );
        assert!(response_header(&response, "Content-Security-Policy").is_some());
        assert_eq!(backend.searches.load(Ordering::SeqCst), 1);
        assert!(response_body(response).contains("query"));
    }

    #[tokio::test]
    async fn body_limit_and_route_allowlist_are_bounded() {
        let (state, _backend) = test_state();
        let request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/stats")
            .with_body("{}")
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Origin", "http://127.0.0.1:4687"))
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Content-Length", "65537"));

        let response = dispatch_test(state.clone(), request).await;
        assert_eq!(response.status_code().0, 413);

        let request = TestRequest::new()
            .with_path("/not-a-route")
            .with_header(header("Host", "127.0.0.1:4687"));

        let response = dispatch_test(state, request).await;
        assert_eq!(response.status_code().0, 404);
    }

    #[tokio::test]
    async fn visualization_routes_are_reserved_without_backend_calls() {
        let (state, _backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();
        for path in [
            "/api/visualization/overview",
            "/api/visualization/entities",
            "/api/visualization/graph",
            "/api/visualization/timeline",
        ] {
            let request = TestRequest::new()
                .with_method(Method::Post)
                .with_path(path)
                .with_body("{}")
                .with_header(header("Host", "127.0.0.1:4687"))
                .with_header(header("Origin", "http://127.0.0.1:4687"))
                .with_header(header("Content-Type", "application/json"))
                .with_header(header("Cookie", format!("{SESSION_COOKIE}={session}")));

            let response = dispatch_test(state.clone(), request).await;
            assert_eq!(response.status_code().0, 501);
        }
    }

    fn authed_post(path: &str, body: &'static str, session: &str) -> TestRequest {
        TestRequest::new()
            .with_method(Method::Post)
            .with_path(path)
            .with_body(body)
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Origin", "http://127.0.0.1:4687"))
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Cookie", format!("{SESSION_COOKIE}={session}")))
    }

    #[tokio::test]
    async fn full_duplex_routes_are_registered_and_reach_backend() {
        // Routes whose fields are all optional (or accept an empty body) reach
        // the FakeBackend, which has no override and returns the trait default
        // 501. A 501 (not 404/405) proves the route is registered, authed, and
        // dispatched to the backend.
        let (state, _backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();
        for path in [
            "/api/observations/recent",
            "/api/context",
            "/api/verify",
            "/api/sessions/list",
            "/api/projects/list",
            "/api/projects/delete",
            "/api/projects/consolidate",
            "/api/projects/prune",
            "/api/ops/doctor",
            "/api/ops/stats",
            "/api/ops/daemon-status",
            "/api/ops/reextract",
            "/api/ops/reindex",
        ] {
            let request = authed_post(path, "{}", &session);
            let response = dispatch_test(state.clone(), request).await;
            assert_eq!(response.status_code().0, 501, "unexpected status for {path}");
        }
    }

    #[tokio::test]
    async fn mutation_route_requires_session_and_origin() {
        let (state, backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();

        // No session cookie → 401, backend untouched.
        let request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/observations/save")
            .with_body(r#"{"type":"note","title":"t","content":"c"}"#)
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Origin", "http://127.0.0.1:4687"))
            .with_header(header("Content-Type", "application/json"));
        let response = dispatch_test(state.clone(), request).await;
        assert_eq!(response.status_code().0, 401);

        // Cross-origin → 403.
        let request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/observations/save")
            .with_body(r#"{"type":"note","title":"t","content":"c"}"#)
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Origin", "http://127.0.0.1:4688"))
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Cookie", format!("{SESSION_COOKIE}={session}")));
        let response = dispatch_test(state.clone(), request).await;
        assert_eq!(response.status_code().0, 403);

        assert_eq!(backend.saves.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn save_route_validates_body_then_proxies() {
        let (state, backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();

        // Missing required fields → 400 before touching the backend.
        let request = authed_post("/api/observations/save", "{}", &session);
        let response = dispatch_test(state.clone(), request).await;
        assert_eq!(response.status_code().0, 400);
        assert_eq!(backend.saves.load(Ordering::SeqCst), 0);

        // Well-formed body → proxied, backend invoked once.
        let request = authed_post(
            "/api/observations/save",
            r#"{"type":"note","title":"hello","content":"world"}"#,
            &session,
        );
        let response = dispatch_test(state.clone(), request).await;
        assert_eq!(response.status_code().0, 200);
        assert_eq!(backend.saves.load(Ordering::SeqCst), 1);
        assert!(response_body(response).contains("hello"));
    }

    #[tokio::test]
    async fn key_routes_require_an_observation_reference() {
        let (state, _backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();

        // Update with neither id nor sync_id → 400.
        let request = authed_post("/api/observations/update", r#"{"title":"x"}"#, &session);
        let response = dispatch_test(state.clone(), request).await;
        assert_eq!(response.status_code().0, 400);

        // Both id and sync_id → 400.
        let request = authed_post(
            "/api/observations/delete",
            r#"{"id":1,"sync_id":"abc"}"#,
            &session,
        );
        let response = dispatch_test(state.clone(), request).await;
        assert_eq!(response.status_code().0, 400);
    }

    #[tokio::test]
    async fn full_duplex_routes_reject_get() {
        let (state, _backend) = test_state();
        let launch = state.auth.launch_token.clone();
        let session = state.auth.redeem_launch(&launch).unwrap();
        let request = TestRequest::new()
            .with_path("/api/observations/save")
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Cookie", format!("{SESSION_COOKIE}={session}")));
        let response = dispatch_test(state, request).await;
        assert_eq!(response.status_code().0, 405);
    }

    fn authed_get(path: &str, session: &str) -> TestRequest {
        TestRequest::new()
            .with_method(Method::Get)
            .with_path(path)
            .with_header(header("Host", "127.0.0.1:4687"))
            .with_header(header("Cookie", format!("{SESSION_COOKIE}={session}")))
    }

    #[test]
    fn eval_scorecard_route_reports_unavailable_with_no_eval_dir() {
        let dir = tempfile::tempdir().unwrap();
        let response = eval_scorecard_route(dir.path());
        assert_eq!(response.status_code().0, 200);
        let body: Value = serde_json::from_str(&response_body(response)).unwrap();
        assert_eq!(body["available"], false);
    }

    #[test]
    fn eval_scorecard_route_serves_newest_json_under_eval() {
        let dir = tempfile::tempdir().unwrap();
        let eval_dir = dir.path().join("eval");
        let baselines_dir = eval_dir.join("baselines");
        std::fs::create_dir_all(&baselines_dir).unwrap();

        // Older file in eval/baselines/.
        let older = baselines_dir.join("gate-locomo.json");
        std::fs::write(&older, r#"{"accuracy_pct": 60.0}"#).unwrap();
        // Force a distinguishable, newer mtime on the top-level file.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let newer = eval_dir.join("scorecard.json");
        std::fs::write(&newer, r#"{"accuracy_pct": 67.6, "mrr": 0.65}"#).unwrap();

        let response = eval_scorecard_route(dir.path());
        assert_eq!(response.status_code().0, 200);
        let body: Value = serde_json::from_str(&response_body(response)).unwrap();
        assert_eq!(body["available"], true);
        assert_eq!(body["source"], "scorecard.json", "must pick the newest file");
        assert_eq!(body["scorecard"]["accuracy_pct"], 67.6);
        assert_eq!(body["scorecard"]["mrr"], 0.65);
    }

    #[test]
    fn eval_scorecard_route_ignores_non_json_files() {
        let dir = tempfile::tempdir().unwrap();
        let eval_dir = dir.path().join("eval");
        std::fs::create_dir_all(&eval_dir).unwrap();
        std::fs::write(eval_dir.join("notes.txt"), "not json").unwrap();
        let response = eval_scorecard_route(dir.path());
        let body: Value = serde_json::from_str(&response_body(response)).unwrap();
        assert_eq!(body["available"], false);
    }

    #[tokio::test]
    async fn eval_scorecard_route_is_wired_and_session_gated() {
        let (state, _backend) = test_state();
        let launch = state.auth.launch_token.clone();

        // No session cookie → 401 (same gate as every other GET route).
        let anon = path_request("/api/eval/scorecard").with_header(header("Host", "127.0.0.1:4687"));
        let response = dispatch_test(state.clone(), anon).await;
        assert_eq!(response.status_code().0, 401);

        // Authed GET reaches the route and returns a well-formed response
        // (available:false in the test process's cwd, which has no eval/ dir
        // relative to where `cargo test` runs this binary from — the route
        // itself is proven directly against a temp dir in the tests above).
        let session = state.auth.redeem_launch(&launch).unwrap();
        let request = authed_get("/api/eval/scorecard", &session);
        let response = dispatch_test(state, request).await;
        assert_eq!(response.status_code().0, 200);
        let body: Value = serde_json::from_str(&response_body(response)).unwrap();
        assert!(body.get("available").is_some());
    }
}
