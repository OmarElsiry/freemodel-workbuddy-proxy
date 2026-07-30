use crate::{config::Config, sse::SseDecoder};
use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

fn proxy_url(config: &Config) -> String {
    let host = if config.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        &config.host
    };
    format!("http://{host}:{}", config.port)
}
async fn online(client: &reqwest::Client, url: &str) -> bool {
    client
        .get(format!("{url}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}
pub async fn start_background(config: &Config) -> Result<()> {
    let client = reqwest::Client::new();
    let url = proxy_url(config);
    if online(&client, &url).await {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config.project_root.join("proxy_server.log"))?;
    let err = log.try_clone()?;
    Command::new(exe)
        .arg("server")
        .current_dir(&config.project_root)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .spawn()?;
    for _ in 0..30 {
        if online(&client, &url).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("Proxy server did not become healthy")
}
fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    print!(
        "{label}{}: ",
        default.map(|d| format!(" [{d}]")).unwrap_or_default()
    );
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let v = line.trim();
    Ok(if v.is_empty() {
        default.unwrap_or_default().into()
    } else {
        v.into()
    })
}
async fn choose_session(client: &reqwest::Client, url: &str, project: &str) -> Result<Value> {
    let sessions = client
        .get(format!("{url}/proxy/sessions"))
        .query(&[("project", project)])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let list = sessions["data"].as_array().cloned().unwrap_or_default();
    println!("0. Create new session");
    for (i, s) in list.iter().enumerate() {
        println!(
            "{}. {} ({})",
            i + 1,
            s["title"].as_str().unwrap_or("Proxy session"),
            s["id"].as_str().unwrap_or("")
        );
    }
    let choice = prompt("Select session", Some("0"))?
        .parse::<usize>()
        .unwrap_or(0);
    if choice > 0 && choice <= list.len() {
        return Ok(list[choice - 1].clone());
    }
    let title = prompt(
        "New session title",
        Path::new(project).file_name().and_then(|v| v.to_str()),
    )?;
    Ok(client
        .post(format!("{url}/proxy/sessions"))
        .json(&json!({"project":project,"title":title}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
pub async fn run(config: &Config) -> Result<()> {
    start_background(config).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?;
    let url = proxy_url(config);
    println!("Freemodel WorkBuddy Proxy TUI");
    let project = loop {
        let value = prompt(
            "Project directory",
            Some(config.default_project.to_string_lossy().as_ref()),
        )?;
        match std::fs::canonicalize(&value) {
            Ok(p) if p.is_dir() => break p.to_string_lossy().to_string(),
            _ => println!("Directory does not exist: {value}"),
        }
    };
    let session = choose_session(&client, &url, &project).await?;
    let id = session["id"]
        .as_str()
        .context("Session has no id")?
        .to_string();
    let mut history = session["history"].as_array().cloned().unwrap_or_default();
    println!(
        "Session: {} ({id}). Type exit to return.",
        session["title"].as_str().unwrap_or("Proxy session")
    );
    loop {
        let user = prompt("You", None)?;
        if matches!(user.trim().to_lowercase().as_str(), "exit" | "quit" | ":q") {
            break;
        }
        if user.trim().is_empty() {
            continue;
        }
        history.push(json!({"role":"user","content":user}));
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("x-workbuddy-session", HeaderValue::from_str(&id)?);
        headers.insert("x-workbuddy-project", HeaderValue::from_str(&project)?);
        if !config.api_key.is_empty() {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", config.api_key))?,
            );
        }
        let response = client
            .post(format!("{url}/v1/chat/completions"))
            .headers(headers)
            .json(&json!({"model":"gpt-5.6-sol","messages":history,"stream":true}))
            .send()
            .await?;
        if !response.status().is_success() {
            println!(
                "HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
            history.pop();
            continue;
        }
        print!("Assistant: ");
        io::stdout().flush()?;
        let mut decoder = SseDecoder::default();
        let mut stream = response.bytes_stream();
        let mut assistant = String::new();
        let mut success = false;
        while let Some(chunk) = stream.next().await {
            for line in decoder.push(&chunk?) {
                let line = line.trim();
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line[5..].trim();
                if data == "[DONE]" {
                    success = true;
                    break;
                }
                let value: Value = serde_json::from_str(data).context("Malformed proxy SSE")?;
                if let Some(error) = value.get("error") {
                    anyhow::bail!(
                        "{}",
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Proxy stream failed")
                    );
                }
                if let Some(text) = value
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                {
                    print!("{text}");
                    io::stdout().flush()?;
                    assistant.push_str(text);
                }
            }
            if success {
                break;
            }
        }
        println!();
        if success {
            let assistant_message = json!({"role":"assistant","content":assistant});
            history.push(assistant_message.clone());
            client
                .post(format!("{url}/proxy/sessions/{id}/history"))
                .json(&json!({"messages":[history[history.len()-2].clone(),assistant_message]}))
                .send()
                .await?
                .error_for_status()?;
        } else {
            history.pop();
            println!("Stream ended without [DONE]");
        }
    }
    Ok(())
}
