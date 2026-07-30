use axum::http::StatusCode;
use freemodel_workbuddy_proxy::{
    acp::{discover_all, serialize_messages},
    error::AcpError,
};
use serde_json::json;
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
fn classifies_http_failures() {
    let auth = AcpError::from_http_status("connection", StatusCode::FORBIDDEN);
    assert_eq!(auth.category, "authentication");
    assert!(!auth.retryable);
    let capacity = AcpError::from_http_status("prompt", StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(capacity.category, "capacity");
    assert!(capacity.retryable);
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
