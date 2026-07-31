use crate::{config::Config, error::AcpError, models::NormalizedEvent, sse::SseDecoder};
use futures_util::{Stream, StreamExt};
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{OwnedMutexGuard, mpsc};

const ACP_EVENT_CHANNEL_CAPACITY: usize = 64;

pub struct AcpEventStream {
    receiver: mpsc::Receiver<Result<NormalizedEvent, AcpError>>,
    gateway_guard: Option<OwnedMutexGuard<()>>,
}
impl AcpEventStream {
    pub fn with_gateway_guard(mut self, guard: OwnedMutexGuard<()>) -> Self {
        self.gateway_guard = Some(guard);
        self
    }
}
impl Stream for AcpEventStream {
    type Item = Result<NormalizedEvent, AcpError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

#[derive(Clone)]
pub struct AcpTransport {
    pub base_url: String,
    pub password: String,
    pub cwd: PathBuf,
    pub timeout: Duration,
}
impl AcpTransport {
    pub fn from_config(
        config: &Config,
        base_url: Option<&str>,
        cwd: Option<&Path>,
    ) -> Result<Self, AcpError> {
        let candidates = candidate_urls(config);
        let selected=base_url.map(str::to_string).or_else(||candidates.first().cloned()).ok_or_else(||AcpError::new("No active WorkBuddy ACP gateway was found. Start WorkBuddy or set WORKBUDDY_ACP_URL.","configuration"))?;
        Ok(Self {
            base_url: selected.trim_end_matches('/').into(),
            password: if config.workbuddy_acp_password.is_empty() {
                std::env::var("CODEBUDDY_GATEWAY_PASSWORD").unwrap_or_default()
            } else {
                config.workbuddy_acp_password.clone()
            },
            cwd: cwd.unwrap_or(&config.workbuddy_acp_cwd).into(),
            timeout: Duration::from_secs_f64(config.workbuddy_acp_timeout),
        })
    }
    fn headers(&self) -> Result<HeaderMap, AcpError> {
        let mut h = HeaderMap::new();
        h.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert("x-codebuddy-request", HeaderValue::from_static("1"));
        if !self.password.is_empty() {
            h.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", self.password))
                    .map_err(|_| AcpError::new("Invalid ACP password header", "configuration"))?,
            );
        }
        Ok(h)
    }
    pub fn stream_chat(&self, messages: Vec<Value>) -> AcpEventStream {
        self.stream_chat_with_attempts(messages, 1)
    }

    pub fn stream_chat_with_attempts(
        &self,
        messages: Vec<Value>,
        max_attempts: usize,
    ) -> AcpEventStream {
        let (tx, rx) = mpsc::channel(ACP_EVENT_CHANNEL_CAPACITY);
        let this = self.clone();
        tokio::spawn(async move {
            let attempts = max_attempts.max(1);
            let mut last_error = None;
            for attempt in 1..=attempts {
                let emitted = Arc::new(AtomicBool::new(false));
                match this.run(messages.clone(), &tx, emitted.clone()).await {
                    Ok(()) => return,
                    Err(error) => {
                        let can_retry = error.retryable
                            && !emitted.load(Ordering::Acquire)
                            && attempt < attempts;
                        last_error = Some(error);
                        if !can_retry || tx.is_closed() {
                            break;
                        }
                    }
                }
            }
            if let Some(mut error) = last_error {
                if attempts > 1 && error.retryable {
                    error.message = format!(
                        "WorkBuddy ACP failed after {attempts} attempts: {}",
                        error.message
                    );
                    error.retryable = false;
                }
                let _ = tx.send(Err(error)).await;
            }
        });
        AcpEventStream {
            receiver: rx,
            gateway_guard: None,
        }
    }
    async fn run(
        &self,
        messages: Vec<Value>,
        tx: &mpsc::Sender<Result<NormalizedEvent, AcpError>>,
        emitted: Arc<AtomicBool>,
    ) -> Result<(), AcpError> {
        let client = Client::builder()
            .timeout(self.timeout)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(network)?;
        let mut headers = self.headers()?;
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        let response = client
            .get(format!("{}/api/v1/acp", self.base_url))
            .headers(headers)
            .send()
            .await
            .map_err(network)?;
        if response.status() != StatusCode::OK {
            return Err(AcpError::from_http_status("connection", response.status()));
        }
        let connection = response
            .headers()
            .get("acp-connection-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let token = response
            .headers()
            .get("acp-session-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        drop(response);
        if connection.is_empty() {
            return Err(AcpError::new(
                "WorkBuddy ACP did not provide a connection id",
                "protocol",
            ));
        }
        let mut session_id = String::new();
        let result=async{
            self.rpc(&client,&connection,&token,1,"initialize",json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"freemodel-workbuddy-proxy","title":"Freemodel WorkBuddy Proxy","version":"2.0.0"}}),|_|Ok(())).await?;
            self.rpc(&client,&connection,&token,2,"session/new",json!({"cwd":self.cwd,"mcpServers":[]}),|e|{if e.get("id").and_then(Value::as_i64)==Some(2){session_id=e.pointer("/result/sessionId").and_then(Value::as_str).unwrap_or("").into();}Ok(())}).await?;
            if session_id.is_empty(){return Err(AcpError::new("WorkBuddy ACP did not create a session","protocol"));}
            let prompt=serialize_messages(&messages);let mut complete=false;let mut disconnected=false;
            self.rpc(&client,&connection,&token,3,"session/prompt",json!({"sessionId":session_id,"prompt":[{"type":"text","text":prompt}]}),|e|{if e.get("method").and_then(Value::as_str)==Some("session/update")&&e.pointer("/params/update/sessionUpdate").and_then(Value::as_str)==Some("agent_message_chunk"){if let Some(text)=e.pointer("/params/update/content/text").and_then(Value::as_str).filter(|s|!s.is_empty()){emitted.store(true,Ordering::Release);match tx.try_send(Ok(NormalizedEvent::TextDelta(text.into()))){Ok(())=>{},Err(mpsc::error::TrySendError::Closed(_))=>disconnected=true,Err(mpsc::error::TrySendError::Full(_))=>return Err(AcpError::new("WorkBuddy ACP downstream buffer is full","capacity")),}}}else if e.get("id").and_then(Value::as_i64)==Some(3){let reason=e.pointer("/result/stopReason").and_then(Value::as_str).unwrap_or("");if reason=="end_turn"{complete=true;}else{return Err(prompt_stop_error(e,reason));}}Ok(())}).await?;
            if disconnected { self.cancel(&client,&connection,&token,&session_id).await; return Ok(()); }
            if !complete{return Err(AcpError::new("WorkBuddy ACP prompt ended without completion","protocol"));}let _=tx.send(Ok(NormalizedEvent::Completed)).await;Ok(())}.await;
        if result.is_err() && !session_id.is_empty() && tx.is_closed() {
            self.cancel(&client, &connection, &token, &session_id).await;
        }
        self.close(&client, &connection, &token).await;
        result
    }
    #[allow(clippy::too_many_arguments)]
    async fn rpc<F>(
        &self,
        client: &Client,
        connection: &str,
        token: &str,
        id: i64,
        method: &str,
        params: Value,
        mut event: F,
    ) -> Result<(), AcpError>
    where
        F: FnMut(&Value) -> Result<(), AcpError>,
    {
        let mut h = self.headers()?;
        h.insert(
            "acp-connection-id",
            HeaderValue::from_str(connection)
                .map_err(|_| AcpError::new("Invalid ACP connection id header", "protocol"))?,
        );
        if !token.is_empty() {
            h.insert(
                "acp-session-token",
                HeaderValue::from_str(token)
                    .map_err(|_| AcpError::new("Invalid ACP session token header", "protocol"))?,
            );
        }
        let response = client
            .post(format!("{}/api/v1/acp", self.base_url))
            .headers(h)
            .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .send()
            .await
            .map_err(network)?;
        if response.status() != StatusCode::OK {
            return Err(AcpError::from_http_status(method, response.status()));
        }
        let mut decoder = SseDecoder::default();
        let mut stream = response.bytes_stream();
        let mut found = false;
        while let Some(chunk) = stream.next().await {
            for line in decoder
                .push(&chunk.map_err(network)?)
                .map_err(|error| AcpError::new(error.to_string(), "protocol"))?
            {
                if let Some(v) = parse_data(&line)? {
                    if v.get("id").and_then(Value::as_i64) == Some(id) {
                        if let Some(error) = v.get("error") {
                            let message = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("WorkBuddy ACP JSON-RPC error");
                            let lower = message.to_lowercase();
                            let upstream_instance_limit = lower
                                .contains("maximum number of running container instances exceeded")
                                || lower.contains("max_instances");
                            let message = if upstream_instance_limit {
                                format!(
                                    "WorkBuddy upstream container capacity is exhausted: {message} This quota is controlled by the WorkBuddy service/account, not local PROXY_MAX_SIDECARS. Wait for an upstream instance to finish or change the official WorkBuddy max_instances configuration."
                                )
                            } else {
                                message.to_string()
                            };
                            let mut error = AcpError::new(
                                message,
                                if upstream_instance_limit {
                                    "capacity"
                                } else {
                                    "upstream"
                                },
                            )
                            .retryable(
                                upstream_instance_limit
                                    || [
                                        "timeout",
                                        "temporarily",
                                        "capacity",
                                        "refusal",
                                        "network",
                                        "interrupted",
                                    ]
                                    .iter()
                                    .any(|m| lower.contains(m)),
                            );
                            if upstream_instance_limit {
                                error = error.status(StatusCode::SERVICE_UNAVAILABLE);
                            }
                            return Err(error);
                        }
                        found = true;
                    }
                    event(&v)?;
                }
            }
        }
        if let Some(line) = decoder
            .finish()
            .map_err(|error| AcpError::new(error.to_string(), "protocol"))?
            && let Some(v) = parse_data(&line)?
        {
            if v.get("id").and_then(Value::as_i64) == Some(id) {
                found = true;
            }
            event(&v)?;
        }
        if !found {
            return Err(AcpError::new(
                format!("WorkBuddy ACP {method} ended without a result"),
                "protocol",
            ));
        }
        Ok(())
    }
    async fn cancel(&self, c: &Client, connection: &str, token: &str, session: &str) {
        let _ = self
            .rpc(
                c,
                connection,
                token,
                4,
                "session/cancel",
                json!({"sessionId":session}),
                |_| Ok(()),
            )
            .await;
    }
    async fn close(&self, c: &Client, connection: &str, token: &str) {
        let Ok(mut h) = self.headers() else { return };
        if let Ok(v) = HeaderValue::from_str(connection) {
            h.insert("acp-connection-id", v);
        }
        if let Ok(v) = HeaderValue::from_str(token) {
            h.insert("acp-session-token", v);
        }
        let _ = c
            .delete(format!("{}/api/v1/acp", self.base_url))
            .headers(h)
            .send()
            .await;
    }
}
pub fn serialize_messages(messages: &[Value]) -> String {
    let mut lines=vec!["Continue the following conversation. Return only the next assistant response. Do not call tools or describe tool use.".into(),String::new()];
    for m in messages {
        let mut role = m
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_uppercase();
        if role == "SYSTEM" || role == "DEVELOPER" {
            continue;
        }
        let mut content =
            crate::openai::text_from_content(m.get("content").unwrap_or(&Value::Null));
        if let Some(calls) = m.get("tool_calls") {
            content.push_str("\nTOOL_CALLS: ");
            content.push_str(&calls.to_string());
        }
        if role == "TOOL" {
            role = format!(
                "TOOL[{}]",
                m.get("tool_call_id").and_then(Value::as_str).unwrap_or("")
            );
        }
        lines.extend([format!("{role}:"), content, String::new()]);
    }
    lines.push("ASSISTANT:".into());
    lines.join("\n")
}
fn prompt_stop_error(event: &Value, reason: &str) -> AcpError {
    let fallback = format!(
        "WorkBuddy ACP stopped with reason: {}",
        if reason.is_empty() { "unknown" } else { reason }
    );
    let Some(raw) = event
        .pointer("/result/_meta/codebuddy.ai~1errorMessage")
        .and_then(Value::as_str)
    else {
        return AcpError::new(
            fallback,
            if reason == "refusal" {
                "refusal"
            } else {
                "upstream"
            },
        )
        .retryable(reason == "refusal");
    };
    let parsed = serde_json::from_str::<Value>(raw).ok();
    let detail = parsed
        .as_ref()
        .and_then(|value| value.pointer("/data/details").and_then(Value::as_str))
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|value| value.get("message").and_then(Value::as_str))
        })
        .unwrap_or(raw)
        .trim();
    let status = parsed
        .as_ref()
        .and_then(|value| value.pointer("/data/statusCode").and_then(Value::as_u64))
        .and_then(|value| u16::try_from(value).ok())
        .and_then(|value| StatusCode::from_u16(value).ok());
    let provider_category = parsed
        .as_ref()
        .and_then(|value| value.pointer("/data/category").and_then(Value::as_str))
        .unwrap_or_default();
    let category = match provider_category {
        "quota" | "capacity" => "capacity",
        "authentication" | "custom_model_auth" => "authentication",
        _ if reason == "refusal" => "refusal",
        _ => "upstream",
    };
    let upstream_instance_limit = detail
        .to_ascii_lowercase()
        .contains("maximum number of running container instances exceeded")
        || detail.to_ascii_lowercase().contains("max_instances");
    let mut error = AcpError::new(
        if upstream_instance_limit {
            format!(
                "WorkBuddy upstream container capacity is exhausted: {detail} This quota is controlled by the WorkBuddy service/account, not local PROXY_MAX_SIDECARS. Wait for an upstream instance to finish or change the official WorkBuddy max_instances configuration."
            )
        } else if detail.is_empty() {
            fallback
        } else {
            format!("WorkBuddy model request failed: {detail}")
        },
        if upstream_instance_limit {
            "capacity"
        } else {
            category
        },
    );
    if upstream_instance_limit {
        error = error.status(StatusCode::SERVICE_UNAVAILABLE);
    } else if let Some(status) = status {
        error = error.status(status);
    }
    let retryable = upstream_instance_limit
        || (provider_category != "quota"
            && category != "authentication"
            && (matches!(
                status.map(|value| value.as_u16()),
                Some(408 | 409 | 425 | 429)
            ) || status.is_some_and(|value| value.is_server_error())));
    error.retryable(retryable)
}

#[doc(hidden)]
pub fn prompt_stop_error_for_test(event: &Value, reason: &str) -> AcpError {
    prompt_stop_error(event, reason)
}

pub fn discover_all(root: Option<&Path>) -> Vec<String> {
    let root = root.map(PathBuf::from).unwrap_or_else(|| {
        std::env::var("CODEBUDDY_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::home_dir()
                    .unwrap_or_default()
                    .join(".workbuddy-ai")
            })
    });
    let Ok(read) = std::fs::read_dir(root.join("sessions")) else {
        return vec![];
    };
    let mut files: Vec<_> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
    let mut seen = HashSet::new();
    files
        .into_iter()
        .filter_map(|e| {
            let v: Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            let pid = v.get("pid")?.as_i64()?;
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return None;
            }
            let url = v
                .get("url")
                .or_else(|| v.get("endpoint"))?
                .as_str()?
                .trim_end_matches('/')
                .to_string();
            seen.insert(url.clone()).then_some(url)
        })
        .collect()
}
pub fn candidate_urls(c: &Config) -> Vec<String> {
    let mut urls = discover_all(None);
    if !c.workbuddy_acp_url.is_empty() && !urls.contains(&c.workbuddy_acp_url) {
        urls.push(c.workbuddy_acp_url.clone());
    }
    urls
}
fn parse_data(line: &str) -> Result<Option<Value>, AcpError> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') || !line.starts_with("data:") {
        return Ok(None);
    }
    let data = line[5..].trim();
    if data.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|_| AcpError::new("Malformed JSON in WorkBuddy ACP stream", "protocol"))?;
    if !value.is_object() {
        return Err(AcpError::new(
            "Invalid JSON value in WorkBuddy ACP stream",
            "protocol",
        ));
    }
    Ok(Some(value))
}
fn network(e: reqwest::Error) -> AcpError {
    if e.is_timeout() {
        AcpError::new("WorkBuddy ACP request timed out", "timeout").retryable(true)
    } else {
        AcpError::new("WorkBuddy ACP connection failed", "network").retryable(true)
    }
}
