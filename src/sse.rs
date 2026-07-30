use bytes::Bytes;
use serde_json::Value;
use thiserror::Error;

pub const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024;

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
    #[error("SSE event exceeded the maximum buffer size")]
    BufferLimit,
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
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, SseError> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            if pos > MAX_SSE_BUFFER_BYTES {
                self.buffer.clear();
                return Err(SseError::BufferLimit);
            }
            let mut raw: Vec<u8> = self.buffer.drain(..=pos).collect();
            raw.pop();
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            lines.push(String::from_utf8_lossy(&raw).into_owned());
        }
        if self.buffer.len() > MAX_SSE_BUFFER_BYTES {
            self.buffer.clear();
            return Err(SseError::BufferLimit);
        }
        Ok(lines)
    }
    pub fn finish(&mut self) -> Result<Option<String>, SseError> {
        if self.buffer.len() > MAX_SSE_BUFFER_BYTES {
            self.buffer.clear();
            return Err(SseError::BufferLimit);
        }
        if self.buffer.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                String::from_utf8_lossy(std::mem::take(&mut self.buffer).as_slice()).into_owned(),
            ))
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
            if self.completed {
                return Err(SseError::InvalidPayload);
            }
            if !self.saw_terminal {
                return Err(SseError::MissingFinish);
            }
            self.completed = true;
            return Ok(Some(ChatSseEvent::Completed));
        }
        if self.completed {
            return Err(SseError::InvalidPayload);
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
        let choices = object
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(SseError::InvalidPayload)?;
        if choices.is_empty() {
            if !object.get("usage").is_some_and(Value::is_object) {
                return Err(SseError::InvalidPayload);
            }
        } else if choices.iter().any(|choice| {
            let Some(choice) = choice.as_object() else {
                return true;
            };
            choice.get("delta").is_some_and(|delta| !delta.is_object())
                || choice
                    .get("finish_reason")
                    .is_some_and(|reason| !reason.is_null() && !reason.is_string())
        }) {
            return Err(SseError::InvalidPayload);
        }
        self.saw_event = true;
        self.saw_terminal |= choices
            .iter()
            .any(|choice| choice.get("finish_reason").is_some_and(|v| !v.is_null()));
        Ok(Some(ChatSseEvent::Chunk(value)))
    }
    pub fn finish(&self) -> Result<(), SseError> {
        if self.completed {
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
        assert!(d.push(b"data: {\"a\":").unwrap().is_empty());
        assert_eq!(d.push(b"1}\n\n").unwrap(), vec!["data: {\"a\":1}", ""]);
    }
    #[test]
    fn rejects_oversized_unterminated_event() {
        let mut decoder = SseDecoder::default();
        let oversized = vec![b'a'; MAX_SSE_BUFFER_BYTES + 1];
        assert_eq!(decoder.push(&oversized), Err(SseError::BufferLimit));
        assert_eq!(decoder.finish(), Ok(None));
    }

    #[test]
    fn rejects_oversized_terminated_event() {
        let mut decoder = SseDecoder::default();
        let mut oversized = vec![b'a'; MAX_SSE_BUFFER_BYTES + 1];
        oversized.push(b'\n');
        assert_eq!(decoder.push(&oversized), Err(SseError::BufferLimit));
    }

    #[test]
    fn accepts_event_at_exact_buffer_limit() {
        let mut decoder = SseDecoder::default();
        let exact = vec![b'a'; MAX_SSE_BUFFER_BYTES];
        assert!(decoder.push(&exact).unwrap().is_empty());
        assert_eq!(
            decoder.finish().unwrap().unwrap().len(),
            MAX_SSE_BUFFER_BYTES
        );
    }

    #[test]
    fn done_requires_finish() {
        let mut v = ChatSseValidator::default();
        assert_eq!(v.line("data: [DONE]"), Err(SseError::MissingFinish));
    }
}
