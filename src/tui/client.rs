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
    pub uptime_seconds: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Diagnostics {
    pub version: String,
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
        if h.service != "freemodel-proxy" || h.status != "ok" {
            return Err("The configured port is not a compatible Freemodel proxy".into());
        }
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
            if let Err(message) = this.run_stream(&request, &cancel, &tx).await {
                let _ = tx
                    .send(StreamEvent::Failed {
                        request_id: request.request_id,
                        message,
                    })
                    .await;
            }
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
