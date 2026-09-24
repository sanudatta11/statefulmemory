use std::io::Read;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, ResponseBox, StatusCode};

use crate::backend::{normalize_name, BackendError};
use crate::{
    AuthFailure, UiState, MAX_BODY_BYTES, MAX_HEADER_BYTES, MAX_URL_BYTES, SESSION_COOKIE,
};

const CSP: &[u8] = b"default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'";
const MAX_TEXT_CHARS: usize = 4096;
const MAX_ENTITY_CHARS: usize = 256;
const SEARCH_DEFAULT_LIMIT: i32 = 15;
const SEARCH_MAX_LIMIT: i32 = 50;
const DECIDE_DEFAULT_LIMIT: i32 = 12;
const DECIDE_MAX_LIMIT: i32 = 30;

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
    Observation,
    RetrievalExplain,
    Jobs,
    Sync,
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
        Route::Search
        | Route::Graph
        | Route::Stats
        | Route::Decide
        | Route::Observation
        | Route::RetrievalExplain
        | Route::Jobs
        | Route::Sync
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
                Route::Visualization(kind) => visualization_route(&state, &body, kind).await,
                Route::Root
                | Route::Favicon
                | Route::AppScript
                | Route::AppStyle
                | Route::Projects
                | Route::ProjectMemories => unreachable!(),
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
        "/api/projects" => (&Method::Get, Route::Projects),
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
    match state
        .backend
        .search(&state.project_name, &query, limit)
        .await
    {
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
    match state
        .backend
        .graph(&state.project_name, &entity, hops)
        .await
    {
        Ok(value) => json_response(200, value),
        Err(error) => backend_error(error),
    }
}

async fn stats_route(state: &UiState, body: &[u8]) -> ResponseBox {
    if let Err(error) = parse_empty_object(body) {
        return error.response();
    }
    match state.backend.stats(&state.project_name).await {
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
    match state
        .backend
        .decide(&state.project_name, &question, limit)
        .await
    {
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
    match state
        .backend
        .observation_detail(&state.project_name, request.observation_id)
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
    match state
        .backend
        .explain(&state.project_name, &query, limit, &mode, &rerank)
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
    match state
        .backend
        .jobs(
            &state.project_name,
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
    if let Err(error) = parse_empty_object(body) {
        return error.response();
    }
    match state.backend.sync_state(&state.project_name).await {
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
    if let Err(error) = parse_empty_object(body) {
        return error.response();
    }
    let result = match kind {
        VisualizationRoute::Overview => tokio::try_join!(
            state.backend.project_summary(&state.project_name),
            state.backend.health(&state.project_name),
            state.backend.graph_stats(&state.project_name),
            state.backend.sync_state(&state.project_name),
            state.backend.jobs(&state.project_name, "", "", 50),
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
            .graph_stats(&state.project_name)
            .await
            .map(|value| json!({ "graph": value })),
        VisualizationRoute::Graph => state
            .backend
            .graph_stats(&state.project_name)
            .await
            .map(|value| json!({ "graph": value })),
        VisualizationRoute::Timeline => state
            .backend
            .jobs(&state.project_name, "", "", 50)
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchBody {
    query: String,
    limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphBody {
    entity: String,
    hops: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideBody {
    question: String,
    limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationBody {
    observation_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetrievalExplainBody {
    query: String,
    limit: Option<i32>,
    mode: Option<String>,
    rerank: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobsBody {
    status: Option<String>,
    kind: Option<String>,
    limit: Option<i32>,
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
    }

    fn test_state() -> (Arc<UiState>, Arc<FakeBackend>) {
        let backend = Arc::new(FakeBackend {
            searches: AtomicUsize::new(0),
        });
        let state = Arc::new(UiState {
            config: UiConfig::new("127.0.0.1", 4687).unwrap(),
            project_name: "test-project".to_string(),
            auth: AuthState::new().unwrap(),
            backend: backend.clone(),
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
}
