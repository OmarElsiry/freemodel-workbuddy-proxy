use freemodel_workbuddy_proxy::openai::{
    build_chat_payload, chat_completion_to_response, chat_content_from_responses, convert_usage,
    responses_input_to_messages, responses_tools_to_chat, text_from_content,
    validate_responses_body,
};
use serde_json::json;

#[test]
fn instructions_and_input_convert_to_chat() {
    let messages =
        responses_input_to_messages(&json!({"instructions":"rules","input":"hello"})).unwrap();
    assert_eq!(messages[0], json!({"role":"system","content":"rules"}));
    assert_eq!(messages[1], json!({"role":"user","content":"hello"}));
}
#[test]
fn function_definitions_and_calls_translate() {
    let tools = responses_tools_to_chat(Some(
        &json!([{"type":"function","name":"lookup","parameters":{"type":"object"}}]),
    ));
    assert_eq!(tools[0]["function"]["name"], "lookup");
    let payload = build_chat_payload(
        &json!({"input":[{"type":"function_call","call_id":"c1","name":"lookup","arguments":"{}"},{"type":"function_call_output","call_id":"c1","output":"ok"}],"stream":true}),
        "gpt-5.6-sol",
    )
    .unwrap();
    assert_eq!(payload["messages"][0]["tool_calls"][0]["id"], "c1");
    assert_eq!(payload["messages"][1]["role"], "tool");
}
#[test]
fn nonstreaming_has_native_response_shape() {
    let v = chat_completion_to_response(
        &json!({"choices":[{"message":{"role":"assistant","content":"hello"}}]}),
        "gpt-5.6-sol",
    );
    assert_eq!(v["object"], "response");
    assert_eq!(v["status"], "completed");
    assert_eq!(v["output"][0]["content"][0]["text"], "hello");
}

#[test]
fn responses_validation_rejects_missing_and_wrong_messages_type() {
    for invalid in [
        json!(null),
        json!([]),
        json!({}),
        json!({"messages":"nope"}),
    ] {
        assert!(validate_responses_body(&invalid).is_err());
    }
    for valid in [
        json!({"input":"hello"}),
        json!({"messages":[]}),
        json!({"prompt":"hello"}),
    ] {
        assert!(validate_responses_body(&valid).is_ok());
    }
}

#[test]
fn content_blocks_and_refusals_flatten_in_order() {
    assert_eq!(
        text_from_content(&json!([
            "A",
            {"type":"input_text","text":"B"},
            {"type":"output_text","text":"C"},
            {"type":"refusal","refusal":"D"},
            {"type":"image","url":"ignored"}
        ])),
        "ABCD"
    );
}

#[test]
fn multimodal_input_preserves_text_and_supported_images_in_order() {
    let converted = chat_content_from_responses(&json!([
        {"type":"input_text","text":"before"},
        {"type":"input_image","image_url":"https://example.test/image.png","detail":"high"},
        {"type":"input_text","text":"middle"},
        {"type":"input_image","image_url":"data:image/png;base64,AA=="},
        {"type":"input_text","text":"after"}
    ]))
    .unwrap();
    assert_eq!(converted[0], json!({"type":"text","text":"before"}));
    assert_eq!(converted[1]["type"], "image_url");
    assert_eq!(
        converted[1]["image_url"]["url"],
        "https://example.test/image.png"
    );
    assert_eq!(converted[1]["image_url"]["detail"], "high");
    assert_eq!(converted[2], json!({"type":"text","text":"middle"}));
    assert_eq!(
        converted[3]["image_url"]["url"],
        "data:image/png;base64,AA=="
    );
    assert_eq!(converted[4], json!({"type":"text","text":"after"}));
}

#[test]
fn local_and_malformed_image_references_fail_instead_of_disappearing() {
    for content in [
        json!([{"type":"input_image","image_url":"/tmp/image.png"}]),
        json!([{"type":"input_image","image_url":"file:///tmp/image.png"}]),
        json!([{"type":"input_image","file_id":"file-123"}]),
        json!([{"type":"input_image","image_url":"data:text/plain;base64,QQ=="}]),
        json!([{"type":"input_image"}]),
        json!([{"type":"unknown","value":"x"}]),
    ] {
        let error = chat_content_from_responses(&content).unwrap_err();
        assert!(!error.is_empty());
    }
}

#[test]
fn developer_role_maps_to_system_and_prompt_fallback_is_preserved() {
    let converted = responses_input_to_messages(&json!({
        "input":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}]
    }))
    .unwrap();
    assert_eq!(
        converted,
        vec![json!({"role":"system","content":[{"type":"text","text":"rules"}]})]
    );

    assert_eq!(
        responses_input_to_messages(&json!({"prompt":"fallback"})).unwrap(),
        vec![json!({"role":"user","content":"fallback"})]
    );
}

#[test]
fn payload_forwards_generation_and_tool_controls() {
    let payload = build_chat_payload(
        &json!({
            "input":"hello",
            "stream":true,
            "max_output_tokens":17,
            "temperature":0.2,
            "top_p":0.8,
            "tools":[{"type":"function","name":"lookup","description":"find","parameters":{"type":"object"},"strict":true}],
            "tool_choice":"required",
            "parallel_tool_calls":false
        }),
        "gpt-5.6-sol",
    )
    .unwrap();
    assert_eq!(payload["model"], "gpt-5.6-sol");
    assert_eq!(payload["stream"], true);
    assert_eq!(payload["max_tokens"], 17);
    assert_eq!(payload["temperature"], 0.2);
    assert_eq!(payload["top_p"], 0.8);
    assert_eq!(payload["tool_choice"], "required");
    assert_eq!(payload["parallel_tool_calls"], false);
    assert_eq!(payload["tools"][0]["function"]["name"], "lookup");
    assert_eq!(payload["tools"][0]["function"]["strict"], true);
}

#[test]
fn malformed_or_unsupported_tools_are_ignored_without_panicking() {
    for tools in [
        json!(null),
        json!({}),
        json!([null, 7, {"type":"other"}, {"type":"function"}]),
    ] {
        assert!(responses_tools_to_chat(Some(&tools)).is_empty());
    }
}

#[test]
fn function_call_arguments_and_outputs_preserve_values() {
    let messages = responses_input_to_messages(&json!({"input":[
        {"type":"function_call","call_id":"c1","name":"lookup","arguments":{"q":"你好"}},
        {"type":"function_call_output","call_id":"c1","output":{"answer":42}}
    ]}))
    .unwrap();
    assert_eq!(messages[0]["tool_calls"][0]["id"], "c1");
    assert_eq!(
        messages[0]["tool_calls"][0]["function"]["arguments"],
        "{\"q\":\"你好\"}"
    );
    assert_eq!(messages[1]["tool_call_id"], "c1");
    assert_eq!(messages[1]["content"], "{\"answer\":42}");
}

#[test]
fn usage_conversion_preserves_token_details_and_total() {
    let usage = convert_usage(&json!({
        "prompt_tokens":10,
        "completion_tokens":4,
        "prompt_tokens_details":{"cached_tokens":3},
        "completion_tokens_details":{"reasoning_tokens":2}
    }));
    assert_eq!(usage["input_tokens"], 10);
    assert_eq!(usage["output_tokens"], 4);
    assert_eq!(usage["total_tokens"], 14);
    assert_eq!(usage["input_tokens_details"]["cached_tokens"], 3);
    assert_eq!(usage["output_tokens_details"]["reasoning_tokens"], 2);
}

#[test]
fn nonstreaming_response_preserves_function_calls() {
    let value = chat_completion_to_response(
        &json!({
            "choices":[{
                "message":{
                    "role":"assistant",
                    "content":null,
                    "tool_calls":[{
                        "id":"call_lookup_1",
                        "type":"function",
                        "function":{"name":"lookup","arguments":"{\"q\":\"weather\"}"}
                    }]
                },
                "finish_reason":"tool_calls"
            }]
        }),
        "gpt-5.6-sol",
    );
    assert_eq!(value["output"].as_array().unwrap().len(), 1);
    assert_eq!(value["output"][0]["type"], "function_call");
    assert_eq!(value["output"][0]["call_id"], "call_lookup_1");
    assert_eq!(value["output"][0]["name"], "lookup");
    assert_eq!(value["output"][0]["arguments"], "{\"q\":\"weather\"}");
}

#[test]
fn nonstreaming_response_preserves_text_and_multiple_function_calls() {
    let value = chat_completion_to_response(
        &json!({
            "choices":[{"message":{
                "role":"assistant",
                "content":"I will check.",
                "tool_calls":[
                    {"id":"call_a","type":"function","function":{"name":"a","arguments":"{}"}},
                    {"id":"call_b","type":"function","function":{"name":"b","arguments":{"x":1}}}
                ]
            }}]
        }),
        "gpt-5.6-sol",
    );
    assert_eq!(value["output"].as_array().unwrap().len(), 3);
    assert_eq!(value["output"][0]["type"], "message");
    assert_eq!(value["output"][0]["content"][0]["text"], "I will check.");
    assert_eq!(value["output"][1]["call_id"], "call_a");
    assert_eq!(value["output"][2]["call_id"], "call_b");
    assert_eq!(value["output"][2]["arguments"], "{\"x\":1}");
}

#[test]
fn nonstreaming_response_includes_converted_usage() {
    let value = chat_completion_to_response(
        &json!({
            "choices":[{"message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}],
            "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}
        }),
        "gpt-5.6-sol",
    );
    assert_eq!(value["output"][0]["content"][0]["text"], "hello");
    assert_eq!(value["usage"]["input_tokens"], 2);
    assert_eq!(value["usage"]["output_tokens"], 1);
    assert_eq!(value["usage"]["total_tokens"], 3);
}
