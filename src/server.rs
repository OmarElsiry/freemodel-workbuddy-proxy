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
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: SessionStore,
    pub sidecars: SidecarManager,
    pub client: reqwest::Client,
    pub gateways: GatewayLocks,
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
            .build()
            .map_err(|e| ProxyError::Internal(e.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
            store,
            sidecars,
            client,
            gateways: Default::default(),
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
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
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
            get(get_session).delete(delete_session),
        )
        .route("/proxy/sessions/{id}/history", post(append_history))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health(State(s): State<AppState>) -> Json<Value> {
    Json(
        json!({"status":"ok","service":"freemodel-proxy","upstream":s.config.base_url,"transport":s.config.transport}),
    )
}
async fn models(State(s): State<AppState>) -> Json<Value> {
    Json(json!({"object":"list","data":s.config.models}))
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

fn auth(headers: &HeaderMap, c: &Config) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|v| !v.is_empty() && !["sk-dummy", "dummy", "placeholder"].contains(v))
        .map(|v| format!("Bearer {v}"))
        .or_else(|| (!c.api_key.is_empty()).then(|| format!("Bearer {}", c.api_key)))
}
fn json_error(status: StatusCode, message: &str, kind: &str) -> Response {
    (
        status,
        Json(json!({"error":{"message":message,"type":kind,"code":status.as_u16()}})),
    )
        .into_response()
}
fn stream_response<S>(stream: S, session: Option<&str>) -> Response
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
    if let Some(id) = session {
        if let Ok(v) = HeaderValue::from_str(id) {
            h.insert("x-workbuddy-session", v);
        }
    }
    response
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
    let guard = s.gateways.acquire(url).await;
    let transport =
        AcpTransport::from_config(&s.config, Some(url), Some(std::path::Path::new(project)))?;
    Ok(transport.stream_chat(messages).with_gateway_guard(guard))
}
async fn first_acp(
    s: &AppState,
    url: &str,
    project: &str,
    messages: Vec<Value>,
) -> Result<(NormalizedEvent, crate::acp::AcpEventStream), AcpError> {
    let mut stream = open_acp(s, url, project, messages).await?;
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
    if let Err(e) = openai::validate_chat_body(&body) {
        return json_error(StatusCode::BAD_REQUEST, &e, "invalid_request_error");
    }
    let model = openai::normalize_model(body.get("model").and_then(Value::as_str));
    body["model"] = json!(model);
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if s.config.transport == "workbuddy_acp" {
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
                stream! {let mut completed=false;match first{NormalizedEvent::TextDelta(text)=>yield Ok(sse::data(&chat_chunk(&id,&model2,json!({"content":text}),Value::Null))),NormalizedEvent::Completed=>{completed=true;yield Ok(sse::data(&chat_chunk(&id,&model2,json!({}),json!("stop"))));yield Ok(sse::done());}}while !completed{match events.next().await{Some(Ok(NormalizedEvent::TextDelta(text)))=>yield Ok(sse::data(&chat_chunk(&id,&model2,json!({"content":text}),Value::Null))),Some(Ok(NormalizedEvent::Completed))=>{completed=true;yield Ok(sse::data(&chat_chunk(&id,&model2,json!({}),json!("stop"))));yield Ok(sse::done());},Some(Err(e))=>{yield Ok(sse::data(&json!({"error":{"message":e.to_string(),"type":"proxy_error","code":format!("workbuddy_acp_{}",e.category)}})));break},None=>{yield Ok(sse::data(&json!({"error":{"message":"WorkBuddy ACP ended without completion","type":"proxy_error","code":"upstream_stream_incomplete"}})));break}}}},
                Some(&session.id),
            );
        }
        let mut stream = match open_acp(&s, &url, &session.project, messages).await {
            Ok(v) => v,
            Err(e) => return ProxyError::Acp(e).into_response(),
        };
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(NormalizedEvent::TextDelta(v)) => text.push_str(&v),
                Ok(NormalizedEvent::Completed) => {
                    let mut r = Json(openai::chat_result(&model, &text)).into_response();
                    r.headers_mut().insert(
                        "x-workbuddy-session",
                        HeaderValue::from_str(&session.id).unwrap(),
                    );
                    return r;
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
    direct_chat(&s, &headers, body, streaming).await
}
fn chat_chunk(id: &str, model: &str, delta: Value, finish: Value) -> Value {
    json!({"id":id,"object":"chat.completion.chunk","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
}
async fn direct_chat(s: &AppState, headers: &HeaderMap, body: Value, streaming: bool) -> Response {
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
        let bytes = response.bytes().await.unwrap_or_default();
        return Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap();
    }
    if !streaming {
        return match response.json::<Value>().await {
            Ok(v) => Json(v).into_response(),
            Err(_) => json_error(
                StatusCode::BAD_GATEWAY,
                "Upstream returned invalid JSON",
                "upstream_error",
            ),
        };
    }
    stream_response(
        stream! {let mut decoder=SseDecoder::default();let mut validator=ChatSseValidator::default();let mut bytes=response.bytes_stream();let mut stopped=false;while let Some(chunk)=bytes.next().await{match chunk{Ok(c)=>for line in decoder.push(&c){match validator.line(&line){Ok(Some(ChatSseEvent::Chunk(v)))=>yield Ok(sse::data(&v)),Ok(Some(ChatSseEvent::Completed))=>{yield Ok(sse::done());stopped=true;break},Ok(None)=>{},Err(e)=>{yield Ok(sse::data(&json!({"error":{"message":e.to_string(),"type":"proxy_error","code":"upstream_stream_error"}})));stopped=true;break}}},Err(_)=>{yield Ok(sse::data(&json!({"error":{"message":"Upstream stream failed","type":"proxy_error","code":"proxy_stream_error"}})));stopped=true;}}if stopped{break}}if !stopped&&validator.finish().is_err(){yield Ok(sse::data(&json!({"error":{"message":"Upstream stream ended without a completion marker","type":"proxy_error","code":"upstream_stream_incomplete"}})));}},
        None,
    )
}

async fn responses(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(e) = openai::validate_responses_body(&body) {
        return json_error(StatusCode::BAD_REQUEST, &e, "invalid_request_error");
    }
    let model = openai::normalize_model(body.get("model").and_then(Value::as_str));
    let chat = openai::build_chat_payload(&body, &model);
    let streaming = chat.get("stream").and_then(Value::as_bool).unwrap_or(false);
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
                    Ok(NormalizedEvent::TextDelta(v)) => text.push_str(&v),
                    Ok(NormalizedEvent::Completed) => {
                        let result = openai::chat_completion_to_response(
                            &openai::chat_result(&model, &text),
                            &model,
                        );
                        let mut r = Json(result).into_response();
                        r.headers_mut().insert(
                            "x-workbuddy-session",
                            HeaderValue::from_str(&session.id).unwrap(),
                        );
                        return r;
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
        let response = direct_chat(&s, &headers, chat_non, false).await;
        if !response.status().is_success() {
            return response;
        }
        let bytes = match axum::body::to_bytes(response.into_body(), usize::MAX).await {
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
        return Json(openai::chat_completion_to_response(&value, &model)).into_response();
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
            stream! {let mut seq=0;let mut text=String::new();yield Ok(response_event("response.created",&mut seq,json!({"response":openai::base_response(&response_id,&model2,"in_progress",vec![],None)})));yield Ok(response_event("response.output_item.added",&mut seq,json!({"output_index":0,"item":{"id":message_id,"type":"message","status":"in_progress","role":"assistant","content":[]}})));let mut next=Some(Ok(first));let mut done=false;while !done{let e=if let Some(v)=next.take(){Some(v)}else{events.next().await};match e{Some(Ok(NormalizedEvent::TextDelta(v)))=>{text.push_str(&v);yield Ok(response_event("response.output_text.delta",&mut seq,json!({"item_id":message_id,"output_index":0,"content_index":0,"delta":v})));},Some(Ok(NormalizedEvent::Completed))=>{let item=openai::message_output_item(&text,Some(&message_id));yield Ok(response_event("response.output_text.done",&mut seq,json!({"item_id":message_id,"output_index":0,"content_index":0,"text":text})));yield Ok(response_event("response.output_item.done",&mut seq,json!({"output_index":0,"item":item})));yield Ok(response_event("response.completed",&mut seq,json!({"response":openai::base_response(&response_id,&model2,"completed",vec![item],None)})));done=true;},Some(Err(e))=>{let mut failed=openai::base_response(&response_id,&model2,"failed",vec![],None);failed["error"]=json!({"code":format!("workbuddy_acp_{}",e.category),"message":e.to_string()});yield Ok(response_event("response.failed",&mut seq,json!({"response":failed})));done=true;},None=>{let mut failed=openai::base_response(&response_id,&model2,"failed",vec![],None);failed["error"]=json!({"code":"upstream_stream_incomplete","message":"WorkBuddy ACP ended without completion"});yield Ok(response_event("response.failed",&mut seq,json!({"response":failed})));done=true;}}}},
            Some(&session.id),
        );
    }
    direct_responses_stream(&s, &headers, chat, &model).await
}
fn response_event(kind: &str, seq: &mut u64, payload: Value) -> bytes::Bytes {
    let mut object = payload.as_object().cloned().unwrap_or_default();
    object.insert("type".into(), json!(kind));
    object.insert("sequence_number".into(), json!(*seq));
    *seq += 1;
    sse::named(kind, &Value::Object(object))
}
async fn direct_responses_stream(
    s: &AppState,
    headers: &HeaderMap,
    chat: Value,
    model: &str,
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
        let bytes = response.bytes().await.unwrap_or_default();
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
        stream! {let mut seq=0;let mut text=String::new();let mut decoder=SseDecoder::default();let mut validator=ChatSseValidator::default();let mut bytes=response.bytes_stream();let mut failed=false;yield Ok(response_event("response.created",&mut seq,json!({"response":openai::base_response(&rid,&model,"in_progress",vec![],None)})));yield Ok(response_event("response.output_item.added",&mut seq,json!({"output_index":0,"item":{"id":mid,"type":"message","status":"in_progress","role":"assistant","content":[]}})));while let Some(chunk)=bytes.next().await{match chunk{Ok(c)=>for line in decoder.push(&c){match validator.line(&line){Ok(Some(ChatSseEvent::Chunk(v)))=>{for choice in v.get("choices").and_then(Value::as_array).into_iter().flatten(){let delta=openai::text_from_content(choice.get("delta").and_then(|d|d.get("content")).unwrap_or(&Value::Null));if !delta.is_empty(){text.push_str(&delta);yield Ok(response_event("response.output_text.delta",&mut seq,json!({"item_id":mid,"output_index":0,"content_index":0,"delta":delta})));}}},Ok(Some(ChatSseEvent::Completed))=>{},Ok(None)=>{},Err(e)=>{let mut r=openai::base_response(&rid,&model,"failed",vec![],None);r["error"]=json!({"code":"upstream_stream_error","message":e.to_string()});yield Ok(response_event("response.failed",&mut seq,json!({"response":r})));failed=true;break}}},Err(_)=>failed=true}if failed{break}}if !failed{if validator.finish().is_err(){let mut r=openai::base_response(&rid,&model,"failed",vec![],None);r["error"]=json!({"code":"upstream_stream_incomplete","message":"Upstream stream ended without a completion marker"});yield Ok(response_event("response.failed",&mut seq,json!({"response":r})));}else{let item=openai::message_output_item(&text,Some(&mid));yield Ok(response_event("response.output_text.done",&mut seq,json!({"item_id":mid,"output_index":0,"content_index":0,"text":text})));yield Ok(response_event("response.output_item.done",&mut seq,json!({"output_index":0,"item":item})));yield Ok(response_event("response.completed",&mut seq,json!({"response":openai::base_response(&rid,&model,"completed",vec![item],None)})));}}},
        None,
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
