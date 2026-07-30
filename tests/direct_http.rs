use axum::{
    Json, Router,
    body::Body,
    extract::{State, connect_info::MockConnectInfo},
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::post,
};
use freemodel_workbuddy_proxy::{
    config::Config,
    server::{AppState, router},
    sse::MAX_SSE_BUFFER_BYTES,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const MAX_UPSTREAM_JSON_BYTES: usize = 16 * 1024 * 1024;
use tempfile::tempdir;
use tower::ServiceExt;

async fn mock_upstream(body: &'static str) -> String {
    mock_upstream_owned(body.to_string()).await
}

async fn mock_upstream_owned(body: String) -> String {
    async fn chat(State(body): State<String>) -> impl IntoResponse {
        (
            [
                ("content-type", "text/event-stream"),
                ("cache-control", "no-cache"),
            ],
            body,
        )
    }
    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .with_state(body);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}/v1")
}

fn direct_state(base_url: &str) -> (tempfile::TempDir, AppState) {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let env = HashMap::from([
        ("HOME".into(), root.path().to_string_lossy().to_string()),
        ("FREEMODEL_BASE_URL".into(), base_url.into()),
        ("FREEMODEL_TRANSPORT".into(), "http".into()),
        (
            "PROXY_DEFAULT_PROJECT".into(),
            project.to_string_lossy().to_string(),
        ),
        (
            "PROXY_SESSION_STORE".into(),
            root.path()
                .join("sessions.json")
                .to_string_lossy()
                .to_string(),
        ),
        (
            "PROXY_RUNTIME_DIR".into(),
            root.path().join("runtime").to_string_lossy().to_string(),
        ),
    ]);
    let config = Config::load_with_env(root.path(), &env).unwrap();
    let state = AppState::new(config).unwrap();
    (root, state)
}

async fn request_stream_with_headers(
    state: AppState,
    route: &str,
    body: &str,
) -> (StatusCode, HeaderMap, String) {
    let response = router(state)
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))))
        .oneshot(
            Request::post(route)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn request_stream(state: AppState, route: &str, body: &str) -> (StatusCode, String) {
    let (status, _, body) = request_stream_with_headers(state, route, body).await;
    (status, body)
}

#[derive(Clone, Default)]
struct RecordedRequest {
    authorization: Arc<Mutex<Option<String>>>,
    body: Arc<Mutex<Option<Value>>>,
}

async fn response_upstream(status: StatusCode, body: String) -> String {
    async fn respond(State((status, body)): State<(StatusCode, String)>) -> impl IntoResponse {
        (status, [("content-type", "application/json")], body)
    }
    let app = Router::new()
        .route("/v1/chat/completions", post(respond))
        .with_state((status, body));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}/v1")
}

async fn redirecting_upstream(target: String) -> String {
    async fn redirect(State(target): State<String>) -> impl IntoResponse {
        (StatusCode::TEMPORARY_REDIRECT, [("location", target)])
    }
    let app = Router::new()
        .route("/v1/chat/completions", post(redirect))
        .with_state(target);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}/v1")
}

async fn recording_upstream() -> (String, RecordedRequest) {
    async fn chat(
        State(recorded): State<RecordedRequest>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        *recorded.authorization.lock().unwrap() = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        *recorded.body.lock().unwrap() = Some(body);
        Json(json!({
            "id":"chatcmpl-test",
            "object":"chat.completion",
            "model":"gpt-5.6-sol",
            "choices":[{"index":0,"message":{"role":"assistant","content":"recorded"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}
        }))
    }
    let recorded = RecordedRequest::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .with_state(recorded.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/v1"), recorded)
}

#[tokio::test]
async fn direct_nonstreaming_forwards_auth_and_converts_responses() {
    let (base, recorded) = recording_upstream().await;
    let (_root, state) = direct_state(&base);
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));
    let response = app
        .oneshot(
            Request::post("/v1/responses")
                .header("content-type", "application/json")
                .header("authorization", "Bearer client-key")
                .body(Body::from(r#"{"model":"gpt-4o","input":"hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("x-workbuddy-session"));
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["output"][0]["content"][0]["text"], "recorded");
    assert_eq!(
        recorded.authorization.lock().unwrap().as_deref(),
        Some("Bearer client-key")
    );
    let upstream = recorded.body.lock().unwrap().clone().unwrap();
    assert_eq!(upstream["model"], "gpt-5.6-sol");
    assert_eq!(upstream["messages"][0]["content"], "hello");
    assert_eq!(upstream["stream"], false);
}

#[tokio::test]
async fn malformed_direct_streams_have_exact_failure_semantics() {
    let base = mock_upstream("data: nope\n\n").await;
    let (_root, chat_state) = direct_state(&base);
    let (status, chat) = request_stream(
        chat_state,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"hello"}],"stream":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(chat.matches("\"error\"").count(), 1, "{chat}");
    assert_eq!(chat.matches("data: [DONE]").count(), 0, "{chat}");

    let (_root, responses_state) = direct_state(&base);
    let (status, responses) = request_stream(
        responses_state,
        "/v1/responses",
        r#"{"input":"hello","stream":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        responses.matches("event: response.failed").count(),
        1,
        "{responses}"
    );
    assert_eq!(
        responses.matches("event: response.completed").count(),
        0,
        "{responses}"
    );
}

#[tokio::test]
async fn proxy_api_key_protects_public_openai_routes() {
    let base = mock_upstream(
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_root, mut state) = direct_state(&base);
    let mut config = (*state.config).clone();
    config.proxy_api_key = "local-secret".into();
    state.config = std::sync::Arc::new(config);
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));

    for authorization in [None, Some("Bearer wrong")] {
        let mut request = Request::get("/v1/models");
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = app
        .oneshot(
            Request::get("/v1/models")
                .header("authorization", "Bearer local-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn oversized_request_body_is_rejected_before_upstream() {
    let (base, recorded) = recording_upstream().await;
    let (_root, state) = direct_state(&base);
    let oversized = format!(
        "{{\"messages\":[{{\"role\":\"user\",\"content\":\"{}\"}}]}}",
        "x".repeat(16 * 1024 * 1024)
    );
    let (status, _) = request_stream(state, "/v1/chat/completions", &oversized).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(recorded.body.lock().unwrap().is_none());
}

#[tokio::test]
async fn oversized_sse_event_fails_chat_and_responses_explicitly() {
    let oversized = format!("data: {}", "x".repeat(MAX_SSE_BUFFER_BYTES + 1));
    let base = mock_upstream_owned(oversized).await;

    let (_root, chat_state) = direct_state(&base);
    let (status, chat) = request_stream(
        chat_state,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"hello"}],"stream":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(chat.matches("\"error\"").count(), 1, "{chat}");
    assert_eq!(chat.matches("data: [DONE]").count(), 0, "{chat}");
    assert!(chat.contains("maximum buffer size"), "{chat}");

    let (_root, responses_state) = direct_state(&base);
    let (status, responses) = request_stream(
        responses_state,
        "/v1/responses",
        r#"{"input":"hello","stream":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(responses.matches("event: response.failed").count(), 1);
    assert_eq!(responses.matches("event: response.completed").count(), 0);
    assert!(responses.contains("maximum buffer size"), "{responses}");
}

#[tokio::test]
async fn direct_http_does_not_follow_upstream_redirects() {
    let (target, recorded) = recording_upstream().await;
    let redirect = redirecting_upstream(format!("{target}/chat/completions")).await;
    let (_root, state) = direct_state(&redirect);
    let (status, body) = request_stream(
        state,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"hello"}]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT, "{body}");
    assert!(recorded.body.lock().unwrap().is_none());
}

#[tokio::test]
async fn direct_http_bounds_success_and_error_response_bodies() {
    for status in [StatusCode::OK, StatusCode::BAD_GATEWAY] {
        let base = response_upstream(status, "x".repeat(MAX_UPSTREAM_JSON_BYTES + 1)).await;
        let (_root, state) = direct_state(&base);
        let (actual, body) = request_stream(
            state,
            "/v1/chat/completions",
            r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        )
        .await;
        assert_eq!(actual, StatusCode::BAD_GATEWAY, "{status}: {body}");
        assert!(body.contains("maximum size"), "{status}: {body}");
    }
}

#[tokio::test]
async fn direct_chat_consumes_unterminated_final_sse_line() {
    let base = mock_upstream(
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]",
    )
    .await;
    let (_root, state) = direct_state(&base);
    let (status, headers, body) = request_stream_with_headers(
        state,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"hello"}],"stream":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-workbuddy-session"));
    assert_eq!(body.matches("data: [DONE]").count(), 1, "{body}");
    assert!(!body.contains("\"error\""), "{body}");
}

fn response_events(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

#[tokio::test]
async fn direct_responses_streams_fragmented_interleaved_function_calls() {
    let base = mock_upstream(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"type\":\"function\",\"function\":{\"name\":\"forecast\",\"arguments\":\"{\\\"city\\\":\\\"\"}},{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\"}}]},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"weather\\\"}\"}},{\"index\":1,\"function\":{\"arguments\":\"Paris\\\"}\"}}]},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_root, state) = direct_state(&base);
    let (status, _, body) =
        request_stream_with_headers(state, "/v1/responses", r#"{"input":"hello","stream":true}"#)
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let events = response_events(&body);
    let kinds: Vec<_> = events
        .iter()
        .filter_map(|event| event["type"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "response.created",
            "response.output_item.added",
            "response.function_call_arguments.delta",
            "response.output_item.added",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.done",
            "response.output_item.done",
            "response.function_call_arguments.done",
            "response.output_item.done",
            "response.completed",
        ]
    );
    for (sequence, event) in events.iter().enumerate() {
        assert_eq!(event["sequence_number"], sequence as u64);
    }
    let completed = events.last().unwrap();
    assert_eq!(completed["response"]["output"][0]["call_id"], "call_b");
    assert_eq!(completed["response"]["output"][0]["name"], "forecast");
    assert_eq!(
        completed["response"]["output"][0]["arguments"],
        r#"{"city":"Paris"}"#
    );
    assert_eq!(completed["response"]["output"][1]["call_id"], "call_a");
    assert_eq!(completed["response"]["output"][1]["name"], "lookup");
    assert_eq!(
        completed["response"]["output"][1]["arguments"],
        r#"{"q":"weather"}"#
    );
    assert_eq!(body.matches("event: response.completed").count(), 1);
    assert_eq!(body.matches("event: response.failed").count(), 0);
}

#[tokio::test]
async fn direct_responses_preserves_text_and_tool_output_order() {
    let base = mock_upstream(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Checking.\",\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_root, state) = direct_state(&base);
    let (status, _, body) =
        request_stream_with_headers(state, "/v1/responses", r#"{"input":"hello","stream":true}"#)
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let events = response_events(&body);
    let completed = events.last().unwrap();
    assert_eq!(completed["type"], "response.completed");
    assert_eq!(completed["response"]["output"][0]["type"], "message");
    assert_eq!(
        completed["response"]["output"][0]["content"][0]["text"],
        "Checking."
    );
    assert_eq!(completed["response"]["output"][1]["type"], "function_call");
    assert_eq!(completed["response"]["output"][1]["call_id"], "call_a");
}

#[tokio::test]
async fn malformed_streaming_function_call_fails_without_completion() {
    let base = mock_upstream(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_root, state) = direct_state(&base);
    let (status, body) =
        request_stream(state, "/v1/responses", r#"{"input":"hello","stream":true}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.matches("event: response.failed").count(), 1, "{body}");
    assert_eq!(
        body.matches("event: response.completed").count(),
        0,
        "{body}"
    );
    assert!(body.contains("omitted its function name"), "{body}");
}

#[tokio::test]
async fn direct_responses_consumes_unterminated_done_line() {
    let base = mock_upstream(
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]",
    )
    .await;
    let (_root, state) = direct_state(&base);
    let (status, headers, body) =
        request_stream_with_headers(state, "/v1/responses", r#"{"input":"hello","stream":true}"#)
            .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-workbuddy-session"));
    assert_eq!(
        body.matches("event: response.completed").count(),
        1,
        "{body}"
    );
    assert_eq!(body.matches("event: response.failed").count(), 0, "{body}");
}
