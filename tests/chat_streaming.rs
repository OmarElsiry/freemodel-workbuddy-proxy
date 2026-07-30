use freemodel_workbuddy_proxy::{
    openai::{normalize_model, validate_chat_body},
    sse::{ChatSseEvent, ChatSseValidator, SseDecoder, SseError},
};
use serde_json::json;

const CONTENT: &str =
    "data: {\"choices\":[{\"delta\":{\"content\":\"你好🙂\"},\"finish_reason\":null}]}";
const FINISH: &str = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}";

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
    assert!(d.push(b"data: {\"choices\":").unwrap().is_empty());
    assert_eq!(
        d.push(b"[]}\r\n\r\n").unwrap(),
        vec!["data: {\"choices\":[]}", ""]
    );
}

#[test]
fn decoder_preserves_split_utf8_and_unterminated_final_line() {
    let bytes = CONTENT.as_bytes();
    let split = bytes
        .windows("你".len())
        .position(|window| window == "你".as_bytes())
        .expect("Unicode marker exists")
        + 1;
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
    assert!(decoder.push(&bytes[split..]).unwrap().is_empty());
    assert_eq!(decoder.finish().unwrap().as_deref(), Some(CONTENT));
}

#[test]
fn ignores_comments_empty_data_and_non_data_fields() {
    let mut validator = ChatSseValidator::default();
    for line in ["", ": keepalive", "event: message", "id: 7", "data:"] {
        assert_eq!(validator.line(line), Ok(None));
    }
    assert_eq!(validator.finish(), Err(SseError::Incomplete));
}

#[test]
fn structured_error_is_sanitized_and_terminal() {
    let mut validator = ChatSseValidator::default();
    assert_eq!(
        validator.line("data: {\"error\":{\"message\":\"capacity exhausted\"}}"),
        Err(SseError::Upstream("capacity exhausted".into()))
    );
    assert_eq!(validator.finish(), Err(SseError::Incomplete));
}

#[test]
fn duplicate_done_or_data_after_done_is_rejected() {
    let mut validator = ChatSseValidator::default();
    assert!(matches!(
        validator.line(CONTENT),
        Ok(Some(ChatSseEvent::Chunk(_)))
    ));
    assert!(matches!(
        validator.line(FINISH),
        Ok(Some(ChatSseEvent::Chunk(_)))
    ));
    assert_eq!(
        validator.line("data: [DONE]"),
        Ok(Some(ChatSseEvent::Completed))
    );
    assert_eq!(
        validator.line("data: [DONE]"),
        Err(SseError::InvalidPayload)
    );
    assert_eq!(validator.line(CONTENT), Err(SseError::InvalidPayload));
}

#[test]
fn eof_without_done_is_incomplete_even_after_finish_reason() {
    let mut missing_done = ChatSseValidator::default();
    missing_done.line(CONTENT).expect("content is valid");
    missing_done.line(FINISH).expect("finish is valid");
    assert_eq!(missing_done.finish(), Err(SseError::Incomplete));

    let mut truncated = ChatSseValidator::default();
    truncated.line(CONTENT).expect("content is valid");
    assert_eq!(truncated.finish(), Err(SseError::Incomplete));
}

#[test]
fn rejects_non_object_json_payloads() {
    for payload in ["null", "[]", "\"text\"", "7"] {
        let mut validator = ChatSseValidator::default();
        assert_eq!(
            validator.line(&format!("data: {payload}")),
            Err(SseError::InvalidPayload)
        );
    }
}

#[test]
fn accepts_usage_only_terminal_metadata_chunk() {
    let mut validator = ChatSseValidator::default();
    assert!(matches!(
        validator.line(r#"data: {"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#),
        Ok(Some(ChatSseEvent::Chunk(_)))
    ));
    assert_eq!(validator.finish(), Err(SseError::Incomplete));
}

#[test]
fn rejects_missing_empty_or_malformed_choices() {
    for payload in [
        r#"{}"#,
        r#"{"choices":null}"#,
        r#"{"choices":[]}"#,
        r#"{"choices":[],"usage":null}"#,
        r#"{"choices":[null]}"#,
        r#"{"choices":[{"delta":"text","finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":7}]}"#,
    ] {
        let mut validator = ChatSseValidator::default();
        assert_eq!(
            validator.line(&format!("data: {payload}")),
            Err(SseError::InvalidPayload),
            "{payload}"
        );
    }
}
