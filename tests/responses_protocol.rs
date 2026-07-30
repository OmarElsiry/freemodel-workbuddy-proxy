use freemodel_workbuddy_proxy::openai::{
    build_chat_payload, chat_completion_to_response, responses_input_to_messages,
    responses_tools_to_chat,
};
use serde_json::json;

#[test]
fn instructions_and_input_convert_to_chat() {
    let messages = responses_input_to_messages(&json!({"instructions":"rules","input":"hello"}));
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
    );
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
