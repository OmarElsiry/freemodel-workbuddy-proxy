use bytes::Bytes;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SseError {
    #[error("Malformed JSON in SSE stream")]
    MalformedJson,
    #[error("Invalid SSE JSON payload")]
    InvalidPayload,
    #[error("Upstream stream ended without a finish reason")]
    MissingFinish,
    #[error("Upstream stream ended without a completion marker")]
    Incomplete,
    #[error("{0}")]
    Upstream(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatSseEvent {
    Chunk(Value),
    Completed,
}

#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}
impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let mut raw: Vec<u8> = self.buffer.drain(..=pos).collect();
            raw.pop();
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            lines.push(String::from_utf8_lossy(&raw).into_owned());
        }
        lines
    }
    pub fn finish(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(std::mem::take(&mut self.buffer).as_slice()).into_owned())
        }
    }
}

#[derive(Default)]
pub struct ChatSseValidator {
    saw_event: bool,
    saw_terminal: bool,
    completed: bool,
}
impl ChatSseValidator {
    pub fn line(&mut self, line: &str) -> Result<Option<ChatSseEvent>, SseError> {
        let line = line.trim();
        if line.is_empty() || line.starts_with(':') || !line.starts_with("data:") {
            return Ok(None);
        }
        let data = line[5..].trim();
        if data.is_empty() {
            return Ok(None);
        }
        if data == "[DONE]" {
            if !self.saw_terminal {
                return Err(SseError::MissingFinish);
            }
            self.completed = true;
            return Ok(Some(ChatSseEvent::Completed));
        }
        let value: Value = serde_json::from_str(data).map_err(|_| SseError::MalformedJson)?;
        let object = value.as_object().ok_or(SseError::InvalidPayload)?;
        if let Some(error) = object.get("error") {
            return Err(SseError::Upstream(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Upstream stream failed")
                    .to_string(),
            ));
        }
        self.saw_event = true;
        self.saw_terminal |= object
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|choice| choice.get("finish_reason").is_some_and(|v| !v.is_null()));
        Ok(Some(ChatSseEvent::Chunk(value)))
    }
    pub fn finish(&self) -> Result<(), SseError> {
        if self.completed || (self.saw_event && self.saw_terminal) {
            Ok(())
        } else {
            Err(SseError::Incomplete)
        }
    }
}

pub fn data(payload: &Value) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        serde_json::to_string(payload).expect("serializable JSON")
    ))
}
pub fn named(event_type: &str, payload: &Value) -> Bytes {
    Bytes::from(format!(
        "event: {event_type}\ndata: {}\n\n",
        serde_json::to_string(payload).expect("serializable JSON")
    ))
}
pub fn done() -> Bytes {
    Bytes::from_static(b"data: [DONE]\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_lines() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"data: {\"a\":").is_empty());
        assert_eq!(d.push(b"1}\n\n"), vec!["data: {\"a\":1}", ""]);
    }
    #[test]
    fn done_requires_finish() {
        let mut v = ChatSseValidator::default();
        assert_eq!(v.line("data: [DONE]"), Err(SseError::MissingFinish));
    }
}
