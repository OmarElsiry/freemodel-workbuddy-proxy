use freemodel_workbuddy_proxy::{
    openai::{normalize_model, validate_chat_body},
    sse::{ChatSseEvent, ChatSseValidator, SseDecoder, SseError},
};
use serde_json::json;

#[test]
fn validates_requests_and_aliases() {
    assert!(validate_chat_body(&json!({"messages":[{"role":"user","content":"x"}]})).is_ok());
    assert!(validate_chat_body(&json!({"messages":[]})).is_err());
    assert_eq!(normalize_model(Some("gpt-4o")), "gpt-5.6-sol");
}
#[test]
fn forwards_incremental_chunks_then_exact_completion() {
    let mut v = ChatSseValidator::default();
    assert!(matches!(
        v.line("data: {\"choices\":[{\"delta\":{\"content\":\"a\"},\"finish_reason\":null}]}")
            .unwrap(),
        Some(ChatSseEvent::Chunk(_))
    ));
    assert!(matches!(
        v.line("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}")
            .unwrap(),
        Some(ChatSseEvent::Chunk(_))
    ));
    assert_eq!(
        v.line("data: [DONE]").unwrap(),
        Some(ChatSseEvent::Completed)
    );
    assert!(v.finish().is_ok());
}
#[test]
fn rejects_done_without_finish_and_malformed_json() {
    let mut v = ChatSseValidator::default();
    assert_eq!(v.line("data: [DONE]"), Err(SseError::MissingFinish));
    let mut v = ChatSseValidator::default();
    assert_eq!(v.line("data: nope"), Err(SseError::MalformedJson));
}
#[test]
fn decoder_handles_arbitrary_network_boundaries() {
    let mut d = SseDecoder::default();
    assert!(d.push(b"data: {\"choices\":").is_empty());
    assert_eq!(d.push(b"[]}\r\n\r\n"), vec!["data: {\"choices\":[]}", ""]);
}
