use freemodel_workbuddy_proxy::sse::SseDecoder;

#[test]
fn tui_parser_preserves_fragmented_sse_lines() {
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(b"data: {\"choices\":[{\"delta\":{").is_empty());
    let lines = decoder.push(b"\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n");
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("hello"));
    assert_eq!(lines[2], "data: [DONE]");
}
