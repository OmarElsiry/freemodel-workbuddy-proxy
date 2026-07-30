use crate::{
    models::{ModelInfo, SessionRecord},
    sse::{ChatSseEvent, ChatSseValidator, SseDecoder},
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use unicode_segmentation::UnicodeSegmentation;

const TUI_TYPING_INTERVAL: Duration = Duration::from_millis(12);

#[derive(Clone)]
pub struct ProxyClient {
    base: String,
    client: reqwest::Client,
    api_key: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Health {
    pub status: String,
    pub service: String,
    pub upstream: String,
    pub transport: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub build_id: String,
    #[serde(default)]
    pub uptime_seconds: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Diagnostics {
    pub version: String,
    #[serde(default)]
    pub build_id: String,
    pub uptime_seconds: u64,
    pub bind_url: String,
    pub transport: String,
    pub upstream_host: String,
    pub session_store: String,
    pub runtime_dir: String,
    pub active_sidecars: usize,
    pub max_sidecars: usize,
    pub rss_bytes: Option<u64>,
}
#[derive(Clone, Debug)]
pub enum StreamEvent {
    Connected {
        request_id: u64,
    },
    Delta {
        request_id: u64,
        text: String,
        elapsed: Duration,
        source_delta: bool,
    },
    Completed {
        request_id: u64,
        total: Duration,
        deltas: usize,
        bytes: usize,
    },
    Failed {
        request_id: u64,
        message: String,
    },
    Cancelled {
        request_id: u64,
    },
}
#[derive(Clone, Debug)]
pub struct StreamRequest {
    pub request_id: u64,
    pub session_id: String,
    pub project: String,
    pub model: String,
    pub messages: Vec<Value>,
}

fn validate_health(health: &Health) -> Result<(), String> {
    if health.service != "freemodel-proxy" || health.status != "ok" {
        return Err("The configured port is not a compatible Freemodel proxy".into());
    }
    if health.build_id != crate::BUILD_ID {
        return Err(format!(
            "The configured port is running a different Freemodel proxy build (expected {}, received {})",
            crate::BUILD_ID,
            if health.build_id.is_empty() {
                "legacy build without an identity"
            } else {
                &health.build_id
            }
        ));
    }
    Ok(())
}

impl ProxyClient {
    pub fn new(
        base: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            base: base.into().trim_end_matches('/').into(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(180))
                .build()?,
            api_key: api_key.into(),
        })
    }
    pub async fn health(&self) -> Result<Health, String> {
        let h = self
            .client
            .get(format!("{}/health", self.base))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json::<Health>()
            .await
            .map_err(clean)?;
        validate_health(&h)?;
        Ok(h)
    }
    pub async fn diagnostics(&self) -> Result<Diagnostics, String> {
        self.client
            .get(format!("{}/proxy/diagnostics", self.base))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json()
            .await
            .map_err(clean)
    }
    pub async fn models(&self) -> Result<Vec<ModelInfo>, String> {
        let v = self
            .client
            .get(format!("{}/v1/models", self.base))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json::<Value>()
            .await
            .map_err(clean)?;
        serde_json::from_value(v.get("data").cloned().unwrap_or_else(|| json!([])))
            .map_err(|_| "Proxy returned an invalid model list".into())
    }
    pub async fn list_sessions(&self, project: &str) -> Result<Vec<SessionRecord>, String> {
        let v = self
            .client
            .get(format!("{}/proxy/sessions", self.base))
            .query(&[("project", project)])
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json::<Value>()
            .await
            .map_err(clean)?;
        serde_json::from_value(v.get("data").cloned().unwrap_or_else(|| json!([])))
            .map_err(|_| "Proxy returned an invalid session list".into())
    }
    pub async fn create_session(
        &self,
        project: &str,
        title: &str,
    ) -> Result<SessionRecord, String> {
        self.client
            .post(format!("{}/proxy/sessions", self.base))
            .json(&json!({"project":project,"title":title}))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json()
            .await
            .map_err(clean)
    }
    pub async fn get_session(&self, id: &str) -> Result<SessionRecord, String> {
        self.client
            .get(format!("{}/proxy/sessions/{id}", self.base))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json()
            .await
            .map_err(clean)
    }
    pub async fn rename_session(&self, id: &str, title: &str) -> Result<SessionRecord, String> {
        self.client
            .patch(format!("{}/proxy/sessions/{id}", self.base))
            .json(&json!({"title":title}))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json()
            .await
            .map_err(clean)
    }
    pub async fn delete_session(&self, id: &str) -> Result<(), String> {
        self.client
            .delete(format!("{}/proxy/sessions/{id}", self.base))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?;
        Ok(())
    }
    pub async fn append_history(
        &self,
        id: &str,
        messages: &[Value],
    ) -> Result<SessionRecord, String> {
        self.client
            .post(format!("{}/proxy/sessions/{id}/history", self.base))
            .json(&json!({"messages":messages}))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json()
            .await
            .map_err(clean)
    }
    pub async fn clear_history(&self, id: &str) -> Result<SessionRecord, String> {
        self.client
            .delete(format!("{}/proxy/sessions/{id}/history", self.base))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json()
            .await
            .map_err(clean)
    }
    pub async fn replace_history(
        &self,
        id: &str,
        messages: &[Value],
    ) -> Result<SessionRecord, String> {
        self.client
            .put(format!("{}/proxy/sessions/{id}/history", self.base))
            .json(&json!({"messages":messages}))
            .send()
            .await
            .map_err(clean)?
            .error_for_status()
            .map_err(clean)?
            .json()
            .await
            .map_err(clean)
    }
    pub fn stream_chat(
        &self,
        request: StreamRequest,
        cancel: CancellationToken,
        tx: mpsc::Sender<StreamEvent>,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            let (source_tx, source_rx) = mpsc::channel(64);
            let presenter = tokio::spawn(present_stream(
                source_rx,
                tx.clone(),
                cancel.clone(),
                TUI_TYPING_INTERVAL,
            ));
            if let Err(message) = this.run_stream(&request, &cancel, &source_tx).await {
                let _ = source_tx
                    .send(StreamEvent::Failed {
                        request_id: request.request_id,
                        message,
                    })
                    .await;
            }
            drop(source_tx);
            let _ = presenter.await;
        });
    }
    async fn run_stream(
        &self,
        r: &StreamRequest,
        cancel: &CancellationToken,
        tx: &mpsc::Sender<StreamEvent>,
    ) -> Result<(), String> {
        let start = Instant::now();
        let mut builder = self
            .client
            .post(format!("{}/v1/chat/completions", self.base))
            .header("x-workbuddy-session", &r.session_id)
            .header("x-workbuddy-project", &r.project)
            .json(&json!({"model":r.model,"messages":r.messages,"stream":true}));
        if !self.api_key.is_empty() {
            builder = builder.bearer_auth(&self.api_key);
        }
        let response = tokio::select! {_ = cancel.cancelled()=>{let _=tx.send(StreamEvent::Cancelled{request_id:r.request_id}).await;return Ok(())},v=builder.send()=>v.map_err(clean)?};
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("HTTP {status}: {}", extract_error(&text)));
        }
        let _ = tx
            .send(StreamEvent::Connected {
                request_id: r.request_id,
            })
            .await;
        let mut decoder = SseDecoder::default();
        let mut validator = ChatSseValidator::default();
        let mut body = response.bytes_stream();
        let mut deltas = 0;
        let mut bytes = 0;
        let mut done = false;
        loop {
            let next = tokio::select! {_ = cancel.cancelled()=>{let _=tx.send(StreamEvent::Cancelled{request_id:r.request_id}).await;return Ok(())},v=body.next()=>v};
            let Some(chunk) = next else { break };
            for line in decoder
                .push(&chunk.map_err(clean)?)
                .map_err(|error| error.to_string())?
            {
                if consume_line(
                    r.request_id,
                    &line,
                    start,
                    &mut validator,
                    tx,
                    &mut deltas,
                    &mut bytes,
                )
                .await?
                {
                    done = true;
                    break;
                }
            }
            if done {
                break;
            }
        }
        if !done && let Some(line) = decoder.finish().map_err(|error| error.to_string())? {
            done = consume_line(
                r.request_id,
                &line,
                start,
                &mut validator,
                tx,
                &mut deltas,
                &mut bytes,
            )
            .await?;
        }
        if !done || validator.finish().is_err() {
            return Err("Stream ended before a valid finish chunk and [DONE]".into());
        }
        tx.send(StreamEvent::Completed {
            request_id: r.request_id,
            total: start.elapsed(),
            deltas,
            bytes,
        })
        .await
        .map_err(|_| "TUI event loop closed".to_string())?;
        Ok(())
    }
}
async fn consume_line(
    id: u64,
    line: &str,
    start: Instant,
    validator: &mut ChatSseValidator,
    tx: &mpsc::Sender<StreamEvent>,
    deltas: &mut usize,
    bytes: &mut usize,
) -> Result<bool, String> {
    match validator.line(line).map_err(|e| e.to_string())? {
        Some(ChatSseEvent::Chunk(v)) => {
            if let Some(text) = v
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                *deltas += 1;
                *bytes += text.len();
                tx.send(StreamEvent::Delta {
                    request_id: id,
                    text: text.into(),
                    elapsed: start.elapsed(),
                    source_delta: true,
                })
                .await
                .map_err(|_| "TUI event loop closed".to_string())?;
            }
            Ok(false)
        }
        Some(ChatSseEvent::Completed) => Ok(true),
        None => Ok(false),
    }
}
async fn present_stream(
    mut source: mpsc::Receiver<StreamEvent>,
    output: mpsc::Sender<StreamEvent>,
    cancel: CancellationToken,
    interval: Duration,
) {
    while let Some(event) = source.recv().await {
        match event {
            StreamEvent::Delta {
                request_id,
                text,
                elapsed,
                ..
            } => {
                let graphemes = UnicodeSegmentation::graphemes(text.as_str(), true)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                for (index, grapheme) in graphemes.iter().enumerate() {
                    if cancel.is_cancelled() {
                        return;
                    }
                    if output
                        .send(StreamEvent::Delta {
                            request_id,
                            text: grapheme.clone(),
                            elapsed,
                            source_delta: index == 0,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if index + 1 < graphemes.len() && !interval.is_zero() {
                        tokio::select! {
                            _ = cancel.cancelled() => return,
                            _ = tokio::time::sleep(interval) => {}
                        }
                    }
                }
            }
            terminal => {
                if output.send(terminal).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn clean(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "Request timed out".into()
    } else if error.is_connect() {
        "Could not connect to the local proxy".into()
    } else {
        format!(
            "Proxy request failed: {}",
            error
                .status()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "network error".into())
        )
    }
}
fn extract_error(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            let clean = text
                .chars()
                .filter(|c| !c.is_control() || *c == '\n')
                .take(240)
                .collect::<String>();
            if clean.is_empty() {
                "Proxy request failed".into()
            } else {
                clean
            }
        })
}

#[cfg(test)]
mod tests {
    use super::{Health, StreamEvent, present_stream, validate_health};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn health(build_id: &str) -> Health {
        Health {
            status: "ok".into(),
            service: "freemodel-proxy".into(),
            upstream: "https://example.invalid/v1".into(),
            transport: "http".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            build_id: build_id.into(),
            uptime_seconds: 1,
        }
    }

    #[test]
    fn accepts_the_current_server_build() {
        assert!(validate_health(&health(crate::BUILD_ID)).is_ok());
    }

    #[test]
    fn rejects_legacy_server_without_build_identity() {
        let error = validate_health(&health("")).unwrap_err();
        assert!(error.contains("legacy build without an identity"));
    }

    #[test]
    fn rejects_a_different_server_build() {
        let error = validate_health(&health("older-build")).unwrap_err();
        assert!(error.contains("different Freemodel proxy build"));
        assert!(error.contains("older-build"));
    }

    #[tokio::test]
    async fn presenter_streams_unicode_graphemes_before_completion() {
        let (source_tx, source_rx) = mpsc::channel(8);
        let (output_tx, mut output_rx) = mpsc::channel(16);
        let presenter = tokio::spawn(present_stream(
            source_rx,
            output_tx,
            CancellationToken::new(),
            Duration::ZERO,
        ));
        source_tx
            .send(StreamEvent::Delta {
                request_id: 7,
                text: "A🇹🇷e\u{301}🙂".into(),
                elapsed: Duration::from_millis(3),
                source_delta: true,
            })
            .await
            .unwrap();
        source_tx
            .send(StreamEvent::Completed {
                request_id: 7,
                total: Duration::from_millis(10),
                deltas: 1,
                bytes: 14,
            })
            .await
            .unwrap();
        drop(source_tx);

        let mut text = String::new();
        let mut source_deltas = 0;
        let mut completed = false;
        while let Some(event) = output_rx.recv().await {
            match event {
                StreamEvent::Delta {
                    text: fragment,
                    source_delta,
                    ..
                } => {
                    text.push_str(&fragment);
                    source_deltas += usize::from(source_delta);
                    assert!(!completed);
                }
                StreamEvent::Completed { .. } => completed = true,
                _ => panic!("unexpected presentation event"),
            }
        }
        presenter.await.unwrap();
        assert_eq!(text, "A🇹🇷e\u{301}🙂");
        assert_eq!(source_deltas, 1);
        assert!(completed);
    }
}
