use serde_json::{Value, json};
use uuid::Uuid;

pub fn normalize_model(model: Option<&str>) -> String {
    let Some(model) = model else {
        return "gpt-5.6-sol".into();
    };
    let cleaned = model.trim().to_lowercase();
    if [
        "gpt 5.6 sol",
        "gpt-5.6-sol",
        "gpt-5.6",
        "gpt 5.6",
        "opencode-default",
        "gpt-4o",
        "gpt-4",
    ]
    .contains(&cleaned.as_str())
    {
        "gpt-5.6-sol".into()
    } else {
        model.into()
    }
}

pub fn text_from_content(content: &Value) -> String {
    match content {
        Value::Null => String::new(),
        Value::String(v) => v.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| match part {
                Value::String(v) => v.clone(),
                Value::Object(o) => match o.get("type").and_then(Value::as_str) {
                    Some("text" | "input_text" | "output_text") => o
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    Some("refusal") => o
                        .get("refusal")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    _ => String::new(),
                },
                _ => String::new(),
            })
            .collect(),
        other => other.to_string(),
    }
}

pub fn validate_chat_body(body: &Value) -> Result<(), String> {
    let Some(object) = body.as_object() else {
        return Err("Request body must be a JSON object".into());
    };
    let Some(messages) = object.get("messages").and_then(Value::as_array) else {
        return Err("messages must be a non-empty array".into());
    };
    if messages.is_empty() {
        return Err("messages must be a non-empty array".into());
    }
    if !messages
        .iter()
        .all(|m| m.as_object().and_then(|o| o.get("role")).is_some())
    {
        return Err("each message must be an object with a role".into());
    }
    Ok(())
}
pub fn validate_responses_body(body: &Value) -> Result<(), String> {
    let Some(object) = body.as_object() else {
        return Err("Request body must be a JSON object".into());
    };
    if !["input", "messages", "prompt"]
        .iter()
        .any(|k| object.contains_key(*k))
    {
        return Err("one of input, messages, or prompt is required".into());
    }
    if object.get("messages").is_some_and(|v| !v.is_array()) {
        return Err("messages must be an array".into());
    }
    Ok(())
}

pub fn responses_input_to_messages(body: &Value) -> Vec<Value> {
    let mut messages = Vec::new();
    let object = body.as_object().cloned().unwrap_or_default();
    if let Some(instructions) = object.get("instructions").filter(|v| !v.is_null()) {
        messages.push(json!({"role":"system","content":text_from_content(instructions)}));
    }
    match object.get("input") {
        Some(Value::String(text)) => messages.push(json!({"role":"user","content":text})),
        Some(Value::Array(items)) => {
            for item in items {
                match item {
                Value::String(text) => messages.push(json!({"role":"user","content":text})),
                Value::Object(item) => match item.get("type").and_then(Value::as_str).unwrap_or("message") {
                    "message" => { let mut role = item.get("role").and_then(Value::as_str).unwrap_or("user"); if role == "developer" { role = "system"; } messages.push(json!({"role":role,"content":text_from_content(item.get("content").unwrap_or(&Value::Null))})); },
                    "function_call" => { let call_id = item.get("call_id").or_else(|| item.get("id")).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("call_{}", &Uuid::new_v4().simple().to_string()[..16])); let args = item.get("arguments").map(|v| if let Some(s)=v.as_str(){s.into()}else{v.to_string()}).unwrap_or_else(||"{}".into()); messages.push(json!({"role":"assistant","content":"","tool_calls":[{"id":call_id,"type":"function","function":{"name":item.get("name").and_then(Value::as_str).unwrap_or("unknown_tool"),"arguments":args}}]})); },
                    "function_call_output" => messages.push(json!({"role":"tool","tool_call_id":item.get("call_id").and_then(Value::as_str).unwrap_or(""),"content":text_from_content(item.get("output").unwrap_or(&Value::Null))})), _ => {}
                }, _ => {}
            }
            }
        }
        None => {
            if let Some(Value::Array(existing)) = object.get("messages") {
                messages.extend(existing.clone());
            } else if let Some(prompt) = object.get("prompt") {
                messages.push(json!({"role":"user","content":text_from_content(prompt)}));
            }
        }
        _ => {}
    }
    if messages.is_empty() {
        messages.push(json!({"role":"user","content":""}));
    }
    messages
}

pub fn responses_tools_to_chat(tools: Option<&Value>) -> Vec<Value> {
    tools
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let object = tool.as_object()?;
            if object.get("type")?.as_str()? != "function" {
                return None;
            }
            if object.get("function").is_some_and(Value::is_object) {
                return Some(tool.clone());
            }
            let mut function = serde_json::Map::new();
            for key in ["name", "description", "parameters", "strict"] {
                if let Some(v) = object.get(key) {
                    function.insert(key.into(), v.clone());
                }
            }
            function
                .contains_key("name")
                .then(|| json!({"type":"function","function":function}))
        })
        .collect()
}

pub fn build_chat_payload(body: &Value, model: &str) -> Value {
    let mut payload = json!({"model":model,"messages":responses_input_to_messages(body),"stream":body.get("stream").and_then(Value::as_bool).unwrap_or(false)});
    let tools = responses_tools_to_chat(body.get("tools"));
    if !tools.is_empty() {
        payload["tools"] = Value::Array(tools);
        if let Some(v) = body.get("tool_choice") {
            payload["tool_choice"] = v.clone();
        }
        if let Some(v) = body.get("parallel_tool_calls") {
            payload["parallel_tool_calls"] = v.clone();
        }
    }
    for (source, target) in [
        ("max_output_tokens", "max_tokens"),
        ("temperature", "temperature"),
        ("top_p", "top_p"),
    ] {
        if let Some(v) = body.get(source) {
            payload[target] = v.clone();
        }
    }
    payload
}

pub fn chat_result(model: &str, text: &str) -> Value {
    json!({"id":format!("chatcmpl-{}",&Uuid::new_v4().simple().to_string()[..24]),"object":"chat.completion","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})
}
pub fn message_output_item(text: &str, id: Option<&str>) -> Value {
    json!({"id":id.map(str::to_string).unwrap_or_else(||format!("msg_{}",&Uuid::new_v4().simple().to_string()[..24])),"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":text,"annotations":[]}]})
}

pub fn response_output_items(message: &Value) -> Vec<Value> {
    let mut output = Vec::new();
    let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
    if !text.is_empty() || message.get("tool_calls").is_none() {
        output.push(message_output_item(&text, None));
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let Some(function) = tool_call.get("function").and_then(Value::as_object) else {
                continue;
            };
            let Some(name) = function.get("name").and_then(Value::as_str) else {
                continue;
            };
            let call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{}", &Uuid::new_v4().simple().to_string()[..16]));
            let arguments = function
                .get("arguments")
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string())
                })
                .unwrap_or_else(|| "{}".into());
            output.push(json!({
                "id": format!("fc_{}", &Uuid::new_v4().simple().to_string()[..24]),
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "arguments": arguments
            }));
        }
    }
    output
}
pub fn base_response(
    id: &str,
    model: &str,
    status: &str,
    output: Vec<Value>,
    usage: Option<&Value>,
) -> Value {
    let mut v = json!({"id":id,"object":"response","created_at":chrono::Utc::now().timestamp(),"status":status,"model":model,"output":output,"parallel_tool_calls":true});
    if let Some(u) = usage {
        v["usage"] = convert_usage(u);
    }
    v
}
pub fn convert_usage(usage: &Value) -> Value {
    let input = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    json!({"input_tokens":input,"input_tokens_details":{"cached_tokens":usage.pointer("/prompt_tokens_details/cached_tokens").or_else(||usage.pointer("/input_tokens_details/cached_tokens")).and_then(Value::as_i64).unwrap_or(0)},"output_tokens":output,"output_tokens_details":{"reasoning_tokens":usage.pointer("/completion_tokens_details/reasoning_tokens").or_else(||usage.pointer("/output_tokens_details/reasoning_tokens")).and_then(Value::as_i64).unwrap_or(0)},"total_tokens":usage.get("total_tokens").and_then(Value::as_i64).unwrap_or(input+output)})
}
pub fn chat_completion_to_response(chat: &Value, model: &str) -> Value {
    let id = format!("resp_{}", &Uuid::new_v4().simple().to_string()[..24]);
    let message = chat
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let output = response_output_items(&message);
    base_response(&id, model, "completed", output, chat.get("usage"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases() {
        assert_eq!(normalize_model(Some("gpt-4o")), "gpt-5.6-sol");
    }
    #[test]
    fn block_text() {
        assert_eq!(
            text_from_content(
                &json!([{"type":"input_text","text":"a"},{"type":"output_text","text":"b"}])
            ),
            "ab"
        );
    }
}
