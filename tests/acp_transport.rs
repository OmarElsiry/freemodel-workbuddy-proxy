use axum::http::StatusCode;
use freemodel_workbuddy_proxy::{
    acp::{AcpTransport, discover_all, prompt_stop_error_for_test, serialize_messages},
    error::AcpError,
    models::NormalizedEvent,
};
use futures_util::StreamExt;
use serde_json::json;
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::tempdir;

#[test]
fn serializes_history_tools_and_omits_runtime_instructions() {
    let text = serialize_messages(&[
        json!({"role":"system","content":"secret"}),
        json!({"role":"developer","content":"dev"}),
        json!({"role":"user","content":"hello"}),
        json!({"role":"assistant","content":"","tool_calls":[{"id":"c1"}]}),
        json!({"role":"tool","tool_call_id":"c1","content":"result"}),
    ]);
    assert!(!text.contains("secret"));
    assert!(!text.contains("dev"));
    assert!(text.contains("USER:\nhello"));
    assert!(text.contains("TOOL_CALLS:"));
    assert!(text.contains("TOOL[c1]:\nresult"));
    assert!(text.ends_with("ASSISTANT:"));
}
#[test]
fn surfaces_provider_quota_from_refusal_metadata() {
    let provider = json!({
        "code": -32003,
        "message": "Quota exceeded",
        "data": {
            "details": "429 Credits exhausted. Purchase add-on packs.",
            "statusCode": 429,
            "category": "quota"
        }
    });
    let event = json!({
        "result": {
            "stopReason": "refusal",
            "_meta": {"codebuddy.ai/errorMessage": provider.to_string()}
        }
    });
    let error = prompt_stop_error_for_test(&event, "refusal");
    assert_eq!(error.category, "capacity");
    assert_eq!(error.status_code, Some(StatusCode::TOO_MANY_REQUESTS));
    assert!(!error.retryable);
    assert!(error.message.contains("Credits exhausted"));
}

#[test]
fn classifies_http_failures() {
    let cases = [
        (StatusCode::FORBIDDEN, "authentication", false),
        (StatusCode::UNAUTHORIZED, "authentication", false),
        (StatusCode::TOO_MANY_REQUESTS, "capacity", true),
        (StatusCode::REQUEST_TIMEOUT, "upstream", true),
        (StatusCode::BAD_GATEWAY, "upstream", true),
        (StatusCode::SERVICE_UNAVAILABLE, "capacity", true),
        (StatusCode::GATEWAY_TIMEOUT, "upstream", true),
        (StatusCode::BAD_REQUEST, "upstream", false),
    ];
    for (status, category, retryable) in cases {
        let error = AcpError::from_http_status("prompt", status);
        assert_eq!(error.category, category, "{status}");
        assert_eq!(error.retryable, retryable, "{status}");
        assert_eq!(error.status_code, Some(status));
    }
}
#[test]
fn discovery_skips_stale_processes() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sessions")).unwrap();
    std::fs::write(
        dir.path().join("sessions/stale.json"),
        r#"{"pid":99999999,"url":"http://127.0.0.1:1"}"#,
    )
    .unwrap();
    assert!(discover_all(Some(dir.path())).is_empty());
}

#[test]
fn discovery_accepts_live_registrations_deduplicates_and_skips_malformed() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let pid = std::process::id();
    std::fs::write(
        sessions.join("one.json"),
        format!(r#"{{"pid":{pid},"url":"http://127.0.0.1:1234/"}}"#),
    )
    .unwrap();
    std::fs::write(
        sessions.join("two.json"),
        format!(r#"{{"pid":{pid},"endpoint":"http://127.0.0.1:1234"}}"#),
    )
    .unwrap();
    std::fs::write(sessions.join("bad.json"), "not-json").unwrap();
    std::fs::write(sessions.join("missing.json"), format!(r#"{{"pid":{pid}}}"#)).unwrap();
    std::fs::write(sessions.join("ignored.txt"), "anything").unwrap();
    assert_eq!(
        discover_all(Some(dir.path())),
        vec!["http://127.0.0.1:1234"]
    );
}

async fn retry_gateway(refuse_attempts: usize) -> (SocketAddr, Arc<AtomicUsize>) {
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::get,
    };
    use serde_json::Value;

    #[derive(Clone)]
    struct GatewayState {
        prompts: Arc<AtomicUsize>,
        refuse_attempts: usize,
    }

    async fn connect() -> impl IntoResponse {
        (
            [
                ("content-type", "text/event-stream"),
                ("acp-connection-id", "test-connection"),
                ("acp-session-token", "test-token"),
            ],
            ":ok\n\n",
        )
    }

    async fn rpc(State(state): State<GatewayState>, Json(body): Json<Value>) -> Response {
        let id = body["id"].as_i64().unwrap();
        let method = body["method"].as_str().unwrap();
        let events = match method {
            "initialize" => vec![json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1}})],
            "session/new" => {
                vec![json!({"jsonrpc":"2.0","id":id,"result":{"sessionId":"session-1"}})]
            }
            "session/prompt" => {
                let attempt = state.prompts.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt <= state.refuse_attempts {
                    vec![json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"refusal"}})]
                } else {
                    vec![
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"RETRY_OK"}}}}),
                        json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                    ]
                }
            }
            "session/cancel" => vec![json!({"jsonrpc":"2.0","id":id,"result":{}})],
            _ => vec![json!({"jsonrpc":"2.0","id":id,"error":{"message":"unknown method"}})],
        };
        let body = events
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            body,
        )
            .into_response()
    }

    async fn close(_headers: HeaderMap) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    let prompts = Arc::new(AtomicUsize::new(0));
    let state = GatewayState {
        prompts: prompts.clone(),
        refuse_attempts,
    };
    let app = Router::new()
        .route("/api/v1/acp", get(connect).post(rpc).delete(close))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, prompts)
}

fn retry_transport(address: SocketAddr) -> AcpTransport {
    AcpTransport {
        base_url: format!("http://{address}"),
        password: String::new(),
        cwd: PathBuf::from("/tmp"),
        timeout: Duration::from_secs(5),
    }
}

#[tokio::test]
async fn retries_retryable_failure_before_first_delta() {
    let (address, prompts) = retry_gateway(1).await;
    let mut stream = retry_transport(address)
        .stream_chat_with_attempts(vec![json!({"role":"user","content":"hello"})], 2);
    assert_eq!(
        stream.next().await.unwrap().unwrap(),
        NormalizedEvent::TextDelta("RETRY_OK".into())
    );
    assert_eq!(
        stream.next().await.unwrap().unwrap(),
        NormalizedEvent::Completed
    );
    assert!(stream.next().await.is_none());
    assert_eq!(prompts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn reports_exhausted_pre_content_retries() {
    let (address, prompts) = retry_gateway(3).await;
    let mut stream = retry_transport(address)
        .stream_chat_with_attempts(vec![json!({"role":"user","content":"hello"})], 2);
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.category, "refusal");
    assert!(!error.retryable);
    assert!(error.message.contains("failed after 2 attempts"));
    assert_eq!(prompts.load(Ordering::SeqCst), 2);
    assert!(stream.next().await.is_none());
}

#[test]
fn serialization_preserves_unicode_multiline_and_content_blocks() {
    let serialized = serialize_messages(&[
        json!({"role":"user","content":[{"type":"input_text","text":"第一行\nsecond🙂"}]}),
        json!({"role":"assistant","content":"答案"}),
    ]);
    assert!(serialized.contains("第一行\nsecond🙂"));
    assert!(serialized.contains("ASSISTANT:\n答案"));
    assert_eq!(serialized.matches("ASSISTANT:").count(), 2);
}
