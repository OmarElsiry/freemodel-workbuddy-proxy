use super::{client::ProxyClient, preferences::Preferences};
use crate::config::Config;
use anyhow::{Context, Result};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

pub struct ServerHandle {
    pub child: Option<Child>,
    pub log_path: PathBuf,
    pub started_here: bool,
}
impl ServerHandle {
    pub fn detach(mut self) {
        self.child.take();
    }
}
pub fn proxy_url(config: &Config) -> String {
    let host = if config.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        &config.host
    };
    format!("http://{host}:{}", config.port)
}
pub async fn ensure_server(config: &Config) -> Result<ServerHandle> {
    let base = proxy_url(config);
    let client = ProxyClient::new(&base, "")?;
    let log_path = config.project_root.join("proxy_server.log");
    match client.health().await {
        Ok(_) => {
            return Ok(ServerHandle {
                child: None,
                log_path,
                started_here: false,
            });
        }
        Err(message) if !message.contains("Could not connect") => anyhow::bail!(
            "Port {} is occupied by an incompatible service: {message}",
            config.port
        ),
        Err(_) => {}
    }
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let err = log.try_clone()?;
    let mut child = Command::new(exe)
        .arg("server")
        .current_dir(&config.project_root)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .spawn()?;
    for _ in 0..75 {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!(
                "Proxy exited during startup with {status}. See {}",
                log_path.display()
            );
        }
        if client.health().await.is_ok() {
            return Ok(ServerHandle {
                child: Some(child),
                log_path,
                started_here: true,
            });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!(
        "Proxy did not become healthy within 15 seconds. See {}",
        log_path.display()
    )
}
pub fn choose_project(config: &Config, prefs: &Preferences) -> Result<String> {
    println!("\nProject setup");
    if !prefs.recent_projects.is_empty() {
        println!("Recent projects:");
        for (i, p) in prefs.recent_projects.iter().enumerate() {
            println!("  {}. {}", i + 1, p);
        }
    }
    loop {
        let input = prompt(
            "Project path or recent number",
            Some(config.default_project.to_string_lossy().as_ref()),
        )?;
        let candidate = input
            .parse::<usize>()
            .ok()
            .and_then(|i| prefs.recent_projects.get(i.saturating_sub(1)).cloned())
            .unwrap_or(input);
        match canonical(&candidate) {
            Ok(value) => return Ok(value),
            Err(e) => println!("Invalid project: {e}"),
        }
    }
}
pub fn canonical(value: &str) -> Result<String> {
    let path = if value == "~" {
        std::env::home_dir().context("Home directory unavailable")?
    } else if let Some(rest) = value.strip_prefix("~/") {
        std::env::home_dir()
            .context("Home directory unavailable")?
            .join(rest)
    } else {
        PathBuf::from(value)
    };
    let path = std::fs::canonicalize(path)?;
    if !path.is_dir() {
        anyhow::bail!("{} is not a directory", path.display())
    }
    Ok(path.to_string_lossy().into())
}
pub fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    print!(
        "{label}{}: ",
        default.map(|d| format!(" [{d}]")).unwrap_or_default()
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    Ok(if value.is_empty() {
        default.unwrap_or_default().into()
    } else {
        value.into()
    })
}
pub fn read_secret(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let secret = rpassword::read_password()?;
    Ok(secret.trim().into())
}
