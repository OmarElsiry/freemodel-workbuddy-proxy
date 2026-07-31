use crate::{
    acp::AcpTransport,
    config::Config,
    error::{AcpError, ProxyError},
    models::{NormalizedEvent, SessionRecord},
    openai,
    routing::{self, GatewayLocks},
    session_store::SessionStore,
    sidecar::SidecarManager,
    sse::{self, ChatSseEvent, ChatSseValidator, SseDecoder},
};
use async_stream::stream;
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
    trace::TraceLayer,
};
use uuid::Uuid;

const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_UPSTREAM_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_STREAM_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_STREAM_TOOL_CALLS: usize = 1024;
const MAX_STREAM_TOOL_METADATA_BYTES: usize = 4096;

fn append_stream_output(target: &mut String, fragment: &str) -> Result<(), ProxyError> {
    if target.len().saturating_add(fragment.len()) > MAX_STREAM_OUTPUT_BYTES {
        return Err(ProxyError::Internal(
            "Upstream streamed output exceeded the maximum size".into(),
        ));
    }
    target.push_str(fragment);
    Ok(())
}

fn has_requested_tools(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
}

fn unsupported_acp_tools() -> Response {
    json_error(
        StatusCode::BAD_REQUEST,
        "Function tools are not supported by the WorkBuddy ACP transport",
        "unsupported_feature_error",
    )
}

async fn bounded_upstream_bytes(response: reqwest::Response) -> Result<bytes::Bytes, ProxyError> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| ProxyError::Internal("Unable to read upstream response".into()))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_UPSTREAM_JSON_BYTES {
            return Err(ProxyError::Internal(
                "Upstream response exceeded the maximum size".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(bytes))
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: SessionStore,
    pub sidecars: SidecarManager,
    pub client: reqwest::Client,
    pub gateways: GatewayLocks,
    pub started_at: Instant,
}
impl AppState {
    pub fn new(config: Config) -> Result<Self, ProxyError> {
        let store = SessionStore::new(&config.session_store, config.max_history_turns);
        let sidecars = SidecarManager::new(
            store.clone(),
            &config.workbuddy_cli_path,
            &config.runtime_dir,
            config.sidecar_startup_timeout,
            config.sidecar_idle_timeout,
            config.max_sidecars,
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ProxyError::Internal(e.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
            store,
            sidecars,
            client,
            gateways: Default::default(),
            started_at: Instant::now(),
        })
    }
}

pub fn router(state: AppState) -> Router {
    let origins: Vec<HeaderValue> = state
        .config
        .cors_origins
        .iter()
        .filter_map(|v| v.parse().ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(tower_http::cors::Any);
    Router::new()
        .route("/", get(health))
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/responses", post(responses))
        .route("/proxy/sessions", get(list_sessions).post(create_session))
        .route(
            "/proxy/sessions/{id}",
            get(get_session)
                .patch(update_session)
                .delete(delete_session),
        )
        .route(
            "/proxy/sessions/{id}/history",
            post(append_history)
                .put(replace_history)
                .delete(clear_history),
        )
        .route("/proxy/diagnostics", get(diagnostics))
        .layer(RequestBodyLimitLayer::new(MAX_REQUEST_BODY_BYTES))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health(State(s): State<AppState>) -> Json<Value> {
    let upstream_mode = if s.config.transport == "workbuddy_acp" {
        "official_local_acp"
    } else {
        "direct_http"
    };
    Json(
        json!({"status":"ok","service":"freemodel-proxy","version":env!("CARGO_PKG_VERSION"),"build_id":crate::BUILD_ID,"uptime_seconds":s.started_at.elapsed().as_secs(),"logical_service":s.config.base_url,"transport":s.config.transport,"upstream_mode":upstream_mode}),
    )
}
async fn models(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if !proxy_auth(&headers, &s.config) {
        return proxy_auth_error();
    }
    let models = &s.config.models;
    Json(json!({
        "object": "list",
        "data": models,
        "models": models,
    }))
    .into_response()
}
fn local(addr: SocketAddr) -> Result<(), ProxyError> {
    if !addr.ip().is_loopback() {
        return Err(ProxyError::Permission(
            "Proxy session management is loopback-only".into(),
        ));
    }
    Ok(())
}
#[derive(Deserialize)]
struct ProjectQuery {
    project: Option<String>,
}
async fn list_sessions(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<Value>, ProxyError> {
    local(addr)?;
    Ok(Json(
        json!({"object":"list","data":s.store.list(q.project.as_deref()).await?}),
    ))
}
async fn create_session(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, ProxyError> {
    local(addr)?;
    let o = body
        .as_object()
        .ok_or_else(|| ProxyError::Invalid("Request body must be a JSON object".into()))?;
    let session = s
        .store
        .create(
            o.get("project").and_then(Value::as_str).unwrap_or(""),
            o.get("title").and_then(Value::as_str).unwrap_or(""),
            None,
            false,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(session)))
}
async fn get_session(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionRecord>, ProxyError> {
    local(addr)?;
    Ok(Json(s.store.get(&id).await?.ok_or_else(|| {
        ProxyError::NotFound("Unknown proxy session".into())
    })?))
}
async fn append_history(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<SessionRecord>, ProxyError> {
    local(addr)?;
    let messages = body
        .as_object()
        .and_then(|o| o.get("messages"))
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| ProxyError::Invalid("messages must be an array".into()))?;
    Ok(Json(s.store.append_history(&id, messages).await?))
}
async fn update_session(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<SessionRecord>, ProxyError> {
    local(addr)?;
    let object = body
        .as_object()
        .ok_or_else(|| ProxyError::Invalid("Request body must be a JSON object".into()))?;
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::Invalid("title must be a non-empty string".into()))?;
    Ok(Json(s.store.update(&id, Some(title), None, None).await?))
}
async fn replace_history(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<SessionRecord>, ProxyError> {
    local(addr)?;
    let messages = body
        .as_object()
        .and_then(|object| object.get("messages"))
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| ProxyError::Invalid("messages must be an array".into()))?;
    Ok(Json(s.store.update(&id, None, Some(messages), None).await?))
}

async fn clear_history(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionRecord>, ProxyError> {
    local(addr)?;
    Ok(Json(
        s.store.update(&id, None, Some(Vec::new()), None).await?,
    ))
}
async fn diagnostics(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ProxyError> {
    local(addr)?;
    let active_sidecars = s
        .store
        .list(None)
        .await?
        .iter()
        .filter(|session| !session.sidecar.is_empty())
        .count();
    let rss_bytes = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|value| {
            value
                .split_whitespace()
                .nth(1)?
                .parse::<u64>()
                .ok()
                .map(|pages| pages * 4096)
        });
    let upstream_host = url::Url::parse(&s.config.base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default();
    let direct = s.config.transport == "http";
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build_id": crate::BUILD_ID,
        "uptime_seconds": s.started_at.elapsed().as_secs(),
        "bind_url": format!("http://{}:{}", s.config.host, s.config.port),
        "transport": s.config.transport,
        "upstream_host": upstream_host,
        "session_store": s.config.session_store,
        "runtime_dir": s.config.runtime_dir,
        "default_project": s.config.default_project,
        "active_sidecars": active_sidecars,
        "max_sidecars": s.config.max_sidecars,
        "capabilities": {
            "responses_api": true,
            "client_function_tools": direct,
            "skills_execution": if direct { "client" } else { "sidecar_only_not_transparent" },
            "vision_input": if direct { "http_https_or_data_image_url" } else { "unsupported" },
            "local_image_paths": false,
            "image_generation": false
        },
        "rss_bytes": rss_bytes
    })))
}
async fn delete_session(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ProxyError> {
    local(addr)?;
    match s.sidecars.stop(&id).await {
        Ok(_) | Err(ProxyError::NotFound(_)) => {}
        Err(e) => return Err(e),
    }
    if !s.store.delete(&id).await? {
        return Err(ProxyError::NotFound("Unknown proxy session".into()));
    }
    Ok(Json(json!({"deleted":true,"id":id})))
}

fn proxy_auth(headers: &HeaderMap, config: &Config) -> bool {
    if config.proxy_api_key.is_empty() {
        return true;
    }
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();
    supplied
        .as_bytes()
        .ct_eq(config.proxy_api_key.as_bytes())
        .into()
}

fn proxy_auth_error() -> Response {
    json_error(
        StatusCode::UNAUTHORIZED,
        "Invalid proxy API key",
        "authentication_error",
    )
}

fn auth(_headers: &HeaderMap, c: &Config) -> Option<String> {
    (!c.api_key.is_empty()).then(|| format!("Bearer {}", c.api_key))
}
fn json_error(status: StatusCode, message: &str, kind: &str) -> Response {
    (
        status,
        Json(json!({"error":{"message":message,"type":kind,"code":status.as_u16()}})),
    )
        .into_response()
}
fn attach_routing_headers(response: &mut Response, session: Option<&SessionRecord>) {
    let Some(session) = session else { return };
    if let Ok(value) = HeaderValue::from_str(&session.id) {
        response.headers_mut().insert("x-workbuddy-session", value);
    }
    if let Ok(value) = HeaderValue::from_str(&session.project) {
        response.headers_mut().insert("x-workbuddy-project", value);
    }
}

fn stream_response<S>(stream: S, session: Option<&SessionRecord>) -> Response
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, Infallible>> + Send + 'static,
{
    let mut response = Response::new(Body::from_stream(stream));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    attach_routing_headers(&mut response, session);
    response
}

async fn resolve_session(
    s: &AppState,
    headers: &HeaderMap,
    messages: &[Value],
) -> Result<SessionRecord, ProxyError> {
    routing::resolve_session(
        headers,
        messages,
        &s.store,
        s.config.default_project.to_string_lossy().as_ref(),
    )
    .await
}

async fn resolve_acp(
    s: &AppState,
    headers: &HeaderMap,
    messages: &[Value],
) -> Result<(SessionRecord, String), ProxyError> {
    routing::resolve(
        headers,
        messages,
        &s.store,
        &s.sidecars,
        s.config.default_project.to_string_lossy().as_ref(),
    )
    .await
}
async fn open_acp(
    s: &AppState,
    url: &str,
    project: &str,
    messages: Vec<Value>,
) -> Result<crate::acp::AcpEventStream, AcpError> {
    open_acp_with_attempts(
        s,
        url,
        project,
        messages,
        s.config.workbuddy_acp_max_attempts,
    )
    .await
}

async fn open_acp_with_attempts(
    s: &AppState,
    url: &str,
    project: &str,
    messages: Vec<Value>,
    attempts: usize,
) -> Result<crate::acp::AcpEventStream, AcpError> {
    let guard = s.gateways.acquire(url).await;
    let transport =
        AcpTransport::from_config(&s.config, Some(url), Some(std::path::Path::new(project)))?;
    Ok(transport
        .stream_chat_with_attempts(messages, attempts)
        .with_gateway_guard(guard))
}
async fn first_acp(
    s: &AppState,
    url: &str,
    project: &str,
    messages: Vec<Value>,
) -> Result<(NormalizedEvent, crate::acp::AcpEventStream), AcpError> {
    let mut stream = open_acp_with_attempts(
        s,
        url,
        project,
        messages,
        s.config.workbuddy_acp_max_attempts,
    )
    .await?;
    match stream.next().await {
        Some(Ok(e)) => Ok((e, stream)),
        Some(Err(e)) => Err(e),
        None => Err(AcpError::new(
            "WorkBuddy ACP ended without completion",
            "protocol",
        )),
    }
}

async fn chat(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    if !proxy_auth(&headers, &s.config) {
        return proxy_auth_error();
    }
    if let Err(e) = openai::validate_chat_body(&body) {
        return json_error(StatusCode::BAD_REQUEST, &e, "invalid_request_error");
    }
    let model = openai::normalize_model(body.get("model").and_then(Value::as_str));
    body["model"] = json!(model);
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if s.config.transport == "workbuddy_acp" {
        if has_requested_tools(&body) {
            return unsupported_acp_tools();
        }
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let (session, url) = match resolve_acp(&s, &headers, &messages).await {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        if streaming {
            let (first, mut events) = match first_acp(&s, &url, &session.project, messages).await {
                Ok(v) => v,
                Err(e) => return ProxyError::Acp(e).into_response(),
            };
            let id = format!("chatcmpl-{}", &Uuid::new_v4().simple().to_string()[..24]);
            let model2 = model.clone();
            return stream_response(
                stream! {let mut completed=false;let mut output_bytes=0usize;match first{NormalizedEvent::TextDelta(text)=>{output_bytes=text.len();yield Ok(sse::data(&chat_chunk(&id,&model2,json!({"content":text}),Value::Null)))},NormalizedEvent::Completed=>{completed=true;yield Ok(sse::data(&chat_chunk(&id,&model2,json!({}),json!("stop"))));yield Ok(sse::done());}}while !completed{match events.next().await{Some(Ok(NormalizedEvent::TextDelta(text)))=>{if output_bytes.saturating_add(text.len())>MAX_STREAM_OUTPUT_BYTES{yield Ok(sse::data(&json!({"error":{"message":"WorkBuddy ACP response exceeded the maximum size","type":"proxy_error","code":"upstream_stream_error"}})));break}output_bytes+=text.len();yield Ok(sse::data(&chat_chunk(&id,&model2,json!({"content":text}),Value::Null)))}, Some(Ok(NormalizedEvent::Completed))=>{completed=true;yield Ok(sse::data(&chat_chunk(&id,&model2,json!({}),json!("stop"))));yield Ok(sse::done());},Some(Err(e))=>{yield Ok(sse::data(&json!({"error":{"message":e.to_string(),"type":"proxy_error","code":format!("workbuddy_acp_{}",e.category)}})));break},None=>{yield Ok(sse::data(&json!({"error":{"message":"WorkBuddy ACP ended without completion","type":"proxy_error","code":"upstream_stream_incomplete"}})));break}}}},
                Some(&session),
            );
        }
        let mut stream = match open_acp(&s, &url, &session.project, messages).await {
            Ok(v) => v,
            Err(e) => return ProxyError::Acp(e).into_response(),
        };
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(NormalizedEvent::TextDelta(v)) => {
                    if append_stream_output(&mut text, &v).is_err() {
                        return json_error(
                            StatusCode::BAD_GATEWAY,
                            "WorkBuddy ACP response exceeded the maximum size",
                            "workbuddy_acp_error",
                        );
                    }
                }
                Ok(NormalizedEvent::Completed) => {
                    let mut response = Json(openai::chat_result(&model, &text)).into_response();
                    attach_routing_headers(&mut response, Some(&session));
                    return response;
                }
                Err(e) => return ProxyError::Acp(e).into_response(),
            }
        }
        return ProxyError::Acp(AcpError::new(
            "WorkBuddy ACP ended without completion",
            "protocol",
        ))
        .into_response();
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let session = match resolve_session(&s, &headers, &messages).await {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    direct_chat(&s, &headers, body, streaming, Some(&session)).await
}
fn chat_chunk(id: &str, model: &str, delta: Value, finish: Value) -> Value {
    json!({"id":id,"object":"chat.completion.chunk","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
}
async fn direct_chat(
    s: &AppState,
    headers: &HeaderMap,
    body: Value,
    streaming: bool,
    session: Option<&SessionRecord>,
) -> Response {
    let url = format!(
        "{}/chat/completions",
        s.config.base_url.trim_end_matches('/')
    );
    let mut req = s.client.post(url).json(&body);
    if let Some(a) = auth(headers, &s.config) {
        req = req.header(header::AUTHORIZATION, a);
    }
    let response = match req.send().await {
        Ok(v) => v,
        Err(e) => {
            return json_error(
                if e.is_timeout() {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                },
                if e.is_timeout() {
                    "Upstream request timed out"
                } else {
                    "Unable to connect to upstream"
                },
                "proxy_error",
            );
        }
    };
    let status = response.status();
    if !status.is_success() {
        let bytes = match bounded_upstream_bytes(response).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    "Upstream error response exceeded the maximum size",
                    "upstream_error",
                );
            }
        };
        return Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap();
    }
    if !streaming {
        let bytes = match bounded_upstream_bytes(response).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    "Upstream response exceeded the maximum size",
                    "upstream_error",
                );
            }
        };
        return match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => {
                let mut response = Json(value).into_response();
                attach_routing_headers(&mut response, session);
                response
            }
            Err(_) => json_error(
                StatusCode::BAD_GATEWAY,
                "Upstream returned invalid JSON",
                "upstream_error",
            ),
        };
    }
    stream_response(
        stream! {
            let mut decoder = SseDecoder::default();
            let mut validator = ChatSseValidator::default();
            let mut bytes = response.bytes_stream();
            let mut stopped = false;
            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Ok(chunk) => {
                        let lines = match decoder.push(&chunk) {
                            Ok(lines) => lines,
                            Err(error) => {
                                yield Ok(sse::data(&json!({"error":{"message":error.to_string(),"type":"proxy_error","code":"upstream_stream_error"}})));
                                stopped = true;
                                break;
                            }
                        };
                        for line in lines {
                            match validator.line(&line) {
                                Ok(Some(ChatSseEvent::Chunk(value))) => {
                                    yield Ok(sse::data(&value));
                                }
                                Ok(Some(ChatSseEvent::Completed)) => {
                                    yield Ok(sse::done());
                                    stopped = true;
                                    break;
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    yield Ok(sse::data(&json!({"error":{"message":error.to_string(),"type":"proxy_error","code":"upstream_stream_error"}})));
                                    stopped = true;
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        yield Ok(sse::data(&json!({"error":{"message":"Upstream stream failed","type":"proxy_error","code":"proxy_stream_error"}})));
                        stopped = true;
                    }
                }
                if stopped {
                    break;
                }
            }
            if !stopped {
                let final_line = match decoder.finish() {
                    Ok(line) => line,
                    Err(error) => {
                        yield Ok(sse::data(&json!({"error":{"message":error.to_string(),"type":"proxy_error","code":"upstream_stream_error"}})));
                        stopped = true;
                        None
                    }
                };
                if let Some(line) = final_line {
                match validator.line(&line) {
                    Ok(Some(ChatSseEvent::Chunk(value))) => yield Ok(sse::data(&value)),
                    Ok(Some(ChatSseEvent::Completed)) => {
                        yield Ok(sse::done());
                        stopped = true;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        yield Ok(sse::data(&json!({"error":{"message":error.to_string(),"type":"proxy_error","code":"upstream_stream_error"}})));
                        stopped = true;
                    }
                }
                }
            }
            if !stopped && validator.finish().is_err() {
                yield Ok(sse::data(&json!({"error":{"message":"Upstream stream ended without a completion marker","type":"proxy_error","code":"upstream_stream_incomplete"}})));
            }
        },
        session,
    )
}

async fn responses(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !proxy_auth(&headers, &s.config) {
        return proxy_auth_error();
    }
    if let Err(e) = openai::validate_responses_body(&body) {
        return json_error(StatusCode::BAD_REQUEST, &e, "invalid_request_error");
    }
    let model = openai::normalize_model(body.get("model").and_then(Value::as_str));
    let chat = match openai::build_chat_payload(&body, &model) {
        Ok(chat) => chat,
        Err(error) => return json_error(StatusCode::BAD_REQUEST, &error, "invalid_request_error"),
    };
    let streaming = chat.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if s.config.transport == "workbuddy_acp" && has_requested_tools(&body) {
        return unsupported_acp_tools();
    }
    if !streaming {
        let mut chat_non = chat.clone();
        chat_non["stream"] = json!(false);
        if s.config.transport == "workbuddy_acp" {
            let messages = chat_non["messages"].as_array().cloned().unwrap_or_default();
            let (session, url) = match resolve_acp(&s, &headers, &messages).await {
                Ok(v) => v,
                Err(e) => return e.into_response(),
            };
            let mut events = match open_acp(&s, &url, &session.project, messages).await {
                Ok(v) => v,
                Err(e) => return ProxyError::Acp(e).into_response(),
            };
            let mut text = String::new();
            while let Some(e) = events.next().await {
                match e {
                    Ok(NormalizedEvent::TextDelta(v)) => {
                        if append_stream_output(&mut text, &v).is_err() {
                            return json_error(
                                StatusCode::BAD_GATEWAY,
                                "WorkBuddy ACP response exceeded the maximum size",
                                "workbuddy_acp_error",
                            );
                        }
                    }
                    Ok(NormalizedEvent::Completed) => {
                        let result = openai::chat_completion_to_response(
                            &openai::chat_result(&model, &text),
                            &model,
                        );
                        let mut response = Json(result).into_response();
                        attach_routing_headers(&mut response, Some(&session));
                        return response;
                    }
                    Err(e) => return ProxyError::Acp(e).into_response(),
                }
            }
            return json_error(
                StatusCode::BAD_GATEWAY,
                "WorkBuddy ACP ended without completion",
                "workbuddy_acp_error",
            );
        }
        let messages = chat_non["messages"].as_array().cloned().unwrap_or_default();
        let session = match resolve_session(&s, &headers, &messages).await {
            Ok(session) => session,
            Err(error) => return error.into_response(),
        };
        let response = direct_chat(&s, &headers, chat_non, false, Some(&session)).await;
        if !response.status().is_success() {
            return response;
        }
        let bytes = match axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_JSON_BYTES).await
        {
            Ok(v) => v,
            Err(_) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    "Upstream returned invalid JSON",
                    "upstream_error",
                );
            }
        };
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    "Upstream returned invalid JSON",
                    "upstream_error",
                );
            }
        };
        let mut response =
            Json(openai::chat_completion_to_response(&value, &model)).into_response();
        attach_routing_headers(&mut response, Some(&session));
        return response;
    }
    if s.config.transport == "workbuddy_acp" {
        let messages = chat["messages"].as_array().cloned().unwrap_or_default();
        let (session, url) = match resolve_acp(&s, &headers, &messages).await {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        let (first, mut events) = match first_acp(&s, &url, &session.project, messages).await {
            Ok(v) => v,
            Err(e) => return ProxyError::Acp(e).into_response(),
        };
        let response_id = format!("resp_{}", &Uuid::new_v4().simple().to_string()[..24]);
        let message_id = format!("msg_{}", &Uuid::new_v4().simple().to_string()[..24]);
        let model2 = model.clone();
        return stream_response(
            stream! {let mut seq=0;let mut text=String::new();yield Ok(response_event("response.created",&mut seq,json!({"response":openai::base_response(&response_id,&model2,"in_progress",vec![],None)})));yield Ok(response_event("response.output_item.added",&mut seq,json!({"output_index":0,"item":{"id":message_id,"type":"message","status":"in_progress","role":"assistant","content":[]}})));let mut next=Some(Ok(first));let mut done=false;while !done{let e=if let Some(v)=next.take(){Some(v)}else{events.next().await};match e{Some(Ok(NormalizedEvent::TextDelta(v)))=>{if append_stream_output(&mut text,&v).is_err(){let mut failed=openai::base_response(&response_id,&model2,"failed",vec![],None);failed["error"]=json!({"code":"upstream_stream_error","message":"WorkBuddy ACP response exceeded the maximum size"});yield Ok(response_event("response.failed",&mut seq,json!({"response":failed})));done=true;continue;}yield Ok(response_event("response.output_text.delta",&mut seq,json!({"item_id":message_id,"output_index":0,"content_index":0,"delta":v})));},Some(Ok(NormalizedEvent::Completed))=>{let item=openai::message_output_item(&text,Some(&message_id));yield Ok(response_event("response.output_text.done",&mut seq,json!({"item_id":message_id,"output_index":0,"content_index":0,"text":text})));yield Ok(response_event("response.output_item.done",&mut seq,json!({"output_index":0,"item":item})));yield Ok(response_event("response.completed",&mut seq,json!({"response":openai::base_response(&response_id,&model2,"completed",vec![item],None)})));done=true;},Some(Err(e))=>{let mut failed=openai::base_response(&response_id,&model2,"failed",vec![],None);failed["error"]=json!({"code":format!("workbuddy_acp_{}",e.category),"message":e.to_string()});yield Ok(response_event("response.failed",&mut seq,json!({"response":failed})));done=true;},None=>{let mut failed=openai::base_response(&response_id,&model2,"failed",vec![],None);failed["error"]=json!({"code":"upstream_stream_incomplete","message":"WorkBuddy ACP ended without completion"});yield Ok(response_event("response.failed",&mut seq,json!({"response":failed})));done=true;}}}},
            Some(&session),
        );
    }
    let messages = chat["messages"].as_array().cloned().unwrap_or_default();
    let session = match resolve_session(&s, &headers, &messages).await {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    direct_responses_stream(&s, &headers, chat, &model, Some(&session)).await
}
fn response_event(kind: &str, seq: &mut u64, payload: Value) -> bytes::Bytes {
    let mut object = payload.as_object().cloned().unwrap_or_default();
    object.insert("type".into(), json!(kind));
    object.insert("sequence_number".into(), json!(*seq));
    *seq += 1;
    sse::named(kind, &Value::Object(object))
}

#[derive(Debug)]
struct StreamingToolCall {
    chat_index: u64,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    pending_argument_deltas: Vec<String>,
    output_index: Option<usize>,
}

#[derive(Debug)]
struct ResponsesStreamState {
    message_id: String,
    message_output_index: Option<usize>,
    text: String,
    tools: Vec<StreamingToolCall>,
    next_output_index: usize,
}

impl ResponsesStreamState {
    fn new(message_id: String) -> Self {
        Self {
            message_id,
            message_output_index: None,
            text: String::new(),
            tools: Vec::new(),
            next_output_index: 0,
        }
    }

    fn consume_chunk(&mut self, value: &Value) -> Result<Vec<(&'static str, Value)>, String> {
        let mut events = Vec::new();
        for choice in value
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(delta) = choice.get("delta").and_then(Value::as_object) else {
                continue;
            };
            let text_delta =
                openai::text_from_content(delta.get("content").unwrap_or(&Value::Null));
            if !text_delta.is_empty() {
                if self.text.len().saturating_add(text_delta.len()) > MAX_STREAM_OUTPUT_BYTES {
                    return Err("Upstream streamed output exceeded the maximum size".into());
                }
                let output_index = *self.message_output_index.get_or_insert_with(|| {
                    let index = self.next_output_index;
                    self.next_output_index += 1;
                    index
                });
                if self.text.is_empty() {
                    events.push((
                        "response.output_item.added",
                        json!({"output_index":output_index,"item":{"id":self.message_id,"type":"message","status":"in_progress","role":"assistant","content":[]}}),
                    ));
                }
                self.text.push_str(&text_delta);
                events.push((
                    "response.output_text.delta",
                    json!({"item_id":self.message_id,"output_index":output_index,"content_index":0,"delta":text_delta}),
                ));
            }

            let Some(tool_deltas) = delta.get("tool_calls") else {
                continue;
            };
            let tool_deltas = tool_deltas
                .as_array()
                .ok_or_else(|| "Upstream tool_calls delta was not an array".to_string())?;
            for tool_delta in tool_deltas {
                let chat_index = tool_delta
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "Upstream tool call delta omitted its index".to_string())?;
                let existing = self
                    .tools
                    .iter()
                    .position(|tool| tool.chat_index == chat_index);
                if existing.is_none() && self.tools.len() >= MAX_STREAM_TOOL_CALLS {
                    return Err("Upstream stream exceeded the maximum tool-call count".into());
                }
                let position = existing.unwrap_or_else(|| {
                    self.tools.push(StreamingToolCall {
                        chat_index,
                        item_id: format!("fc_{}", &Uuid::new_v4().simple().to_string()[..24]),
                        call_id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                        pending_argument_deltas: Vec::new(),
                        output_index: None,
                    });
                    self.tools.len() - 1
                });
                let tool = &mut self.tools[position];
                if let Some(id) = tool_delta.get("id") {
                    let fragment = id
                        .as_str()
                        .ok_or_else(|| "Upstream tool call id was not a string".to_string())?;
                    if tool.call_id.len().saturating_add(fragment.len())
                        > MAX_STREAM_TOOL_METADATA_BYTES
                    {
                        return Err("Upstream tool call id exceeded the maximum size".into());
                    }
                    tool.call_id.push_str(fragment);
                }
                if let Some(function) = tool_delta.get("function") {
                    let function = function.as_object().ok_or_else(|| {
                        "Upstream tool call function delta was not an object".to_string()
                    })?;
                    if let Some(name) = function.get("name") {
                        let fragment = name
                            .as_str()
                            .ok_or_else(|| "Upstream tool name was not a string".to_string())?;
                        if tool.name.len().saturating_add(fragment.len())
                            > MAX_STREAM_TOOL_METADATA_BYTES
                        {
                            return Err("Upstream tool name exceeded the maximum size".into());
                        }
                        tool.name.push_str(fragment);
                    }
                    if let Some(arguments) = function.get("arguments") {
                        let fragment = arguments.as_str().ok_or_else(|| {
                            "Upstream tool arguments were not a string".to_string()
                        })?;
                        if tool.arguments.len().saturating_add(fragment.len())
                            > MAX_STREAM_OUTPUT_BYTES
                        {
                            return Err(
                                "Upstream streamed tool arguments exceeded the maximum size".into(),
                            );
                        }
                        tool.arguments.push_str(fragment);
                        tool.pending_argument_deltas.push(fragment.to_string());
                    }
                }
                if tool.output_index.is_none() && !tool.name.is_empty() {
                    let output_index = self.next_output_index;
                    self.next_output_index += 1;
                    tool.output_index = Some(output_index);
                    if tool.call_id.is_empty() {
                        tool.call_id =
                            format!("call_{}", &Uuid::new_v4().simple().to_string()[..16]);
                    }
                    events.push((
                        "response.output_item.added",
                        json!({"output_index":output_index,"item":{"id":tool.item_id,"type":"function_call","status":"in_progress","call_id":tool.call_id,"name":tool.name,"arguments":""}}),
                    ));
                }
                if let Some(output_index) = tool.output_index {
                    for fragment in tool.pending_argument_deltas.drain(..) {
                        events.push((
                            "response.function_call_arguments.delta",
                            json!({"item_id":tool.item_id,"output_index":output_index,"delta":fragment}),
                        ));
                    }
                }
            }
        }
        Ok(events)
    }

    fn completed_events(self) -> Result<Vec<(&'static str, Value)>, String> {
        let mut events = Vec::new();
        let output_count = self.next_output_index.max(1);
        let mut output = vec![None; output_count];
        if let Some(output_index) = self.message_output_index {
            let item = openai::message_output_item(&self.text, Some(&self.message_id));
            events.push((
                "response.output_text.done",
                json!({"item_id":self.message_id,"output_index":output_index,"content_index":0,"text":self.text}),
            ));
            events.push((
                "response.output_item.done",
                json!({"output_index":output_index,"item":item}),
            ));
            output[output_index] = Some(item);
        } else if self.tools.is_empty() {
            let item = openai::message_output_item("", Some(&self.message_id));
            events.push((
                "response.output_item.added",
                json!({"output_index":0,"item":{"id":self.message_id,"type":"message","status":"in_progress","role":"assistant","content":[]}}),
            ));
            events.push((
                "response.output_text.done",
                json!({"item_id":self.message_id,"output_index":0,"content_index":0,"text":""}),
            ));
            events.push((
                "response.output_item.done",
                json!({"output_index":0,"item":item}),
            ));
            output[0] = Some(item);
        }
        for tool in self.tools {
            let output_index = tool.output_index.ok_or_else(|| {
                format!(
                    "Upstream tool call {} omitted its function name",
                    tool.chat_index
                )
            })?;
            let item = json!({
                "id":tool.item_id,
                "type":"function_call",
                "status":"completed",
                "call_id":tool.call_id,
                "name":tool.name,
                "arguments":tool.arguments
            });
            events.push((
                "response.function_call_arguments.done",
                json!({"item_id":tool.item_id,"output_index":output_index,"arguments":tool.arguments}),
            ));
            events.push((
                "response.output_item.done",
                json!({"output_index":output_index,"item":item}),
            ));
            output[output_index] = Some(item);
        }
        events.push((
            "response.completed",
            json!({"output":output.into_iter().flatten().collect::<Vec<_>>() }),
        ));
        Ok(events)
    }
}

async fn direct_responses_stream(
    s: &AppState,
    headers: &HeaderMap,
    chat: Value,
    model: &str,
    session: Option<&SessionRecord>,
) -> Response {
    let url = format!(
        "{}/chat/completions",
        s.config.base_url.trim_end_matches('/')
    );
    let mut req = s.client.post(url).json(&chat);
    if let Some(a) = auth(headers, &s.config) {
        req = req.header(header::AUTHORIZATION, a);
    }
    let response = match req.send().await {
        Ok(v) => v,
        Err(_) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "Unable to connect to upstream stream",
                "proxy_error",
            );
        }
    };
    if !response.status().is_success() {
        let status = response.status();
        let bytes = match bounded_upstream_bytes(response).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    "Upstream error response exceeded the maximum size",
                    "upstream_error",
                );
            }
        };
        return Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap();
    }
    let rid = format!("resp_{}", &Uuid::new_v4().simple().to_string()[..24]);
    let mid = format!("msg_{}", &Uuid::new_v4().simple().to_string()[..24]);
    let model = model.to_string();
    stream_response(
        stream! {
            let mut seq = 0;
            let mut stream_state = ResponsesStreamState::new(mid);
            let mut decoder = SseDecoder::default();
            let mut validator = ChatSseValidator::default();
            let mut bytes = response.bytes_stream();
            let mut failed = false;
            yield Ok(response_event("response.created", &mut seq, json!({"response":openai::base_response(&rid,&model,"in_progress",vec![],None)})));
            while let Some(chunk) = bytes.next().await {
                let lines = match chunk {
                    Ok(chunk) => match decoder.push(&chunk) {
                        Ok(lines) => lines,
                        Err(error) => {
                            let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                            response["error"] = json!({"code":"upstream_stream_error","message":error.to_string()});
                            yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                            failed = true;
                            break;
                        }
                    },
                    Err(_) => {
                        let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                        response["error"] = json!({"code":"proxy_stream_error","message":"Upstream stream failed"});
                        yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                        failed = true;
                        break;
                    }
                };
                for line in lines {
                    match validator.line(&line) {
                        Ok(Some(ChatSseEvent::Chunk(value))) => {
                            match stream_state.consume_chunk(&value) {
                                Ok(events) => {
                                    for (kind, payload) in events {
                                        yield Ok(response_event(kind, &mut seq, payload));
                                    }
                                }
                                Err(message) => {
                                    let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                                    response["error"] = json!({"code":"upstream_stream_error","message":message});
                                    yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                                    failed = true;
                                    break;
                                }
                            }
                        }
                        Ok(Some(ChatSseEvent::Completed)) | Ok(None) => {}
                        Err(error) => {
                            let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                            response["error"] = json!({"code":"upstream_stream_error","message":error.to_string()});
                            yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                            failed = true;
                            break;
                        }
                    }
                }
                if failed {
                    break;
                }
            }
            if !failed {
                let final_line = match decoder.finish() {
                    Ok(line) => line,
                    Err(error) => {
                        let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                        response["error"] = json!({"code":"upstream_stream_error","message":error.to_string()});
                        yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                        failed = true;
                        None
                    }
                };
                if let Some(line) = final_line {
                    match validator.line(&line) {
                        Ok(Some(ChatSseEvent::Chunk(value))) => {
                            match stream_state.consume_chunk(&value) {
                                Ok(events) => {
                                    for (kind, payload) in events {
                                        yield Ok(response_event(kind, &mut seq, payload));
                                    }
                                }
                                Err(message) => {
                                    let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                                    response["error"] = json!({"code":"upstream_stream_error","message":message});
                                    yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                                    failed = true;
                                }
                            }
                        }
                        Ok(Some(ChatSseEvent::Completed)) | Ok(None) => {}
                        Err(error) => {
                            let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                            response["error"] = json!({"code":"upstream_stream_error","message":error.to_string()});
                            yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                            failed = true;
                        }
                    }
                }
                if !failed {
                    if validator.finish().is_err() {
                        let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                        response["error"] = json!({"code":"upstream_stream_incomplete","message":"Upstream stream ended without a completion marker"});
                        yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                    } else {
                        match stream_state.completed_events() {
                            Ok(events) => {
                                for (kind, payload) in events {
                                    if kind == "response.completed" {
                                        let output = payload.get("output").and_then(Value::as_array).cloned().unwrap_or_default();
                                        yield Ok(response_event(kind, &mut seq, json!({"response":openai::base_response(&rid,&model,"completed",output,None)})));
                                    } else {
                                        yield Ok(response_event(kind, &mut seq, payload));
                                    }
                                }
                            }
                            Err(message) => {
                                let mut response = openai::base_response(&rid, &model, "failed", vec![], None);
                                response["error"] = json!({"code":"upstream_stream_error","message":message});
                                yield Ok(response_event("response.failed", &mut seq, json!({"response":response})));
                            }
                        }
                    }
                }
            }
        },
        session,
    )
}

pub async fn serve(state: AppState) -> Result<(), ProxyError> {
    state.store.clear_stale_runtime().await?;
    let addr: SocketAddr = format!("{}:{}", state.config.host, state.config.port)
        .parse()
        .map_err(|e| ProxyError::Invalid(format!("Invalid proxy address: {e}")))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| ProxyError::Internal(format!("Unable to bind proxy: {e}")))?;
    let cleanup = state.clone();
    let reaper = state.clone();
    let idle = tokio::spawn(async move {
        let every = Duration::from_secs_f64(reaper.config.sidecar_idle_timeout.clamp(1.0, 30.0));
        loop {
            tokio::time::sleep(every).await;
            let _ = reaper.sidecars.reap_idle().await;
        }
    });
    let result = axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await;
    idle.abort();
    cleanup.sidecars.stop_all().await;
    result.map_err(|e| ProxyError::Internal(e.to_string()))
}
async fn shutdown() {
    let ctrl = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {_=ctrl=>{},_=term=>{}}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_delta(index: u64, id: Value, name: Value, arguments: Value) -> Value {
        let mut function = serde_json::Map::new();
        if !name.is_null() {
            function.insert("name".into(), name);
        }
        if !arguments.is_null() {
            function.insert("arguments".into(), arguments);
        }
        let mut tool = serde_json::Map::from_iter([
            ("index".into(), json!(index)),
            ("function".into(), Value::Object(function)),
        ]);
        if !id.is_null() {
            tool.insert("id".into(), id);
        }
        json!({
            "choices":[{
                "delta":{"tool_calls":[Value::Object(tool)]},
                "finish_reason":null
            }]
        })
    }

    #[test]
    fn response_stream_state_bounds_text_and_tool_arguments() {
        let mut text = ResponsesStreamState::new("msg_test".into());
        text.text = "x".repeat(MAX_STREAM_OUTPUT_BYTES);
        let error = text
            .consume_chunk(&json!({
                "choices":[{"delta":{"content":"x"},"finish_reason":null}]
            }))
            .unwrap_err();
        assert!(error.contains("maximum size"), "{error}");

        let mut arguments = ResponsesStreamState::new("msg_test".into());
        arguments.tools.push(StreamingToolCall {
            chat_index: 0,
            item_id: "fc_test".into(),
            call_id: "call_test".into(),
            name: "lookup".into(),
            arguments: "x".repeat(MAX_STREAM_OUTPUT_BYTES),
            pending_argument_deltas: Vec::new(),
            output_index: Some(0),
        });
        arguments.next_output_index = 1;
        let error = arguments
            .consume_chunk(&tool_delta(0, Value::Null, Value::Null, json!("x")))
            .unwrap_err();
        assert!(error.contains("tool arguments"), "{error}");
    }

    #[test]
    fn response_stream_state_bounds_tool_count_and_metadata() {
        let mut tools = ResponsesStreamState::new("msg_test".into());
        for index in 0..MAX_STREAM_TOOL_CALLS as u64 {
            tools.tools.push(StreamingToolCall {
                chat_index: index,
                item_id: format!("fc_{index}"),
                call_id: String::new(),
                name: String::new(),
                arguments: String::new(),
                pending_argument_deltas: Vec::new(),
                output_index: None,
            });
        }
        let error = tools
            .consume_chunk(&tool_delta(
                MAX_STREAM_TOOL_CALLS as u64,
                json!("call_extra"),
                json!("lookup"),
                json!("{}"),
            ))
            .unwrap_err();
        assert!(error.contains("tool-call count"), "{error}");

        for (id, name, arguments, expected) in [
            (
                json!("x".repeat(MAX_STREAM_TOOL_METADATA_BYTES + 1)),
                Value::Null,
                Value::Null,
                "call id",
            ),
            (
                Value::Null,
                json!("x".repeat(MAX_STREAM_TOOL_METADATA_BYTES + 1)),
                Value::Null,
                "tool name",
            ),
            (json!(7), Value::Null, Value::Null, "call id"),
            (Value::Null, json!(7), Value::Null, "tool name"),
            (Value::Null, Value::Null, json!({}), "tool arguments"),
        ] {
            let mut state = ResponsesStreamState::new("msg_test".into());
            let error = state
                .consume_chunk(&tool_delta(0, id, name, arguments))
                .unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }
}
