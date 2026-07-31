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
    let host = match config.host.as_str() {
        "0.0.0.0" => "127.0.0.1",
        "::" | "[::]" => "[::1]",
        value => value,
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
        Err(message) if !message.contains("Could not connect") => {
            anyhow::bail!(port_recovery_guidance(config.port, &message))
        }
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
fn port_recovery_guidance(port: u16, reason: &str) -> String {
    let next_port = port.checked_add(1).unwrap_or(40590);
    if cfg!(target_os = "windows") {
        format!(
            "Port {port} is occupied by an incompatible or stale service.\n\nReason:\n  {reason}\n\nFix on Windows (PowerShell):\n  1. Find the process ID (PID):\n     Get-NetTCPConnection -LocalPort {port} | Select-Object LocalAddress,LocalPort,State,OwningProcess\n\n  2. Inspect the process before stopping it:\n     Get-CimInstance Win32_Process -Filter \"ProcessId=<PID>\" | Select-Object ProcessId,Name,ExecutablePath,CommandLine\n\n  3. If it is an old freemodel-workbuddy-proxy process, stop it:\n     Stop-Process -Id <PID>\n\n  4. Start this project again.\n\nAlternative: keep the current service and use another port:\n     $env:PROXY_PORT={next_port}\n     cargo run --release -- tui\n\nDo not stop the PID unless step 2 confirms that it belongs to the stale Freemodel proxy."
        )
    } else if cfg!(target_os = "macos") {
        format!(
            "Port {port} is occupied by an incompatible or stale service.\n\nReason:\n  {reason}\n\nFix on macOS:\n  1. Find the process ID (PID):\n     lsof -nP -iTCP:{port} -sTCP:LISTEN\n\n  2. Inspect the process before stopping it:\n     ps -p <PID> -o pid=,ppid=,command=\n\n  3. If it is an old freemodel-workbuddy-proxy process, stop it gracefully:\n     kill <PID>\n\n  4. Start this project again:\n     ./start.sh\n\nAlternative: keep the current service and use another port:\n     PROXY_PORT={next_port} cargo run --release -- tui\n\nDo not stop the PID unless step 2 confirms that it belongs to the stale Freemodel proxy."
        )
    } else {
        format!(
            "Port {port} is occupied by an incompatible or stale service.\n\nReason:\n  {reason}\n\nFix on Linux:\n  1. Find the process ID (PID):\n     ss -ltnp 'sport = :{port}'\n     # If ss does not show the PID, try: lsof -nP -iTCP:{port} -sTCP:LISTEN\n\n  2. Inspect the process before stopping it:\n     ps -p <PID> -o pid=,ppid=,user=,args=\n\n  3. If it is an old freemodel-workbuddy-proxy process, stop it gracefully:\n     kill <PID>\n\n  4. Confirm that the port is free, then start again:\n     ss -ltnp 'sport = :{port}'\n     ./start.sh\n\nAlternative: keep the current service and use another port:\n     PROXY_PORT={next_port} cargo run --release -- tui\n\nDo not stop the PID unless step 2 confirms that it belongs to the stale Freemodel proxy."
        )
    }
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

#[cfg(test)]
mod tests {
    use super::{port_recovery_guidance, proxy_url};
    use crate::config::Config;
    use std::{collections::HashMap, path::Path};

    fn config(host: &str) -> Config {
        let mut environment = HashMap::new();
        environment.insert("HOME".into(), "/tmp".into());
        environment.insert(
            "FREEMODEL_BASE_URL".into(),
            "https://example.test/v1".into(),
        );
        environment.insert("PROXY_HOST".into(), host.into());
        environment.insert("PROXY_PORT".into(), "40589".into());
        Config::load_with_env(Path::new("/tmp"), &environment).unwrap()
    }

    #[test]
    fn display_url_uses_reachable_loopback_for_wildcard_binds() {
        assert_eq!(proxy_url(&config("0.0.0.0")), "http://127.0.0.1:40589");
        assert_eq!(proxy_url(&config("::")), "http://[::1]:40589");
        assert_eq!(proxy_url(&config("127.0.0.1")), "http://127.0.0.1:40589");
    }

    #[test]
    fn stale_port_error_has_safe_readable_os_specific_recovery() {
        let message = port_recovery_guidance(
            40589,
            "The configured port is running a different Freemodel proxy build",
        );
        assert!(message.contains("Port 40589"), "{message}");
        assert!(
            message.contains("Reason:\n  The configured port"),
            "{message}"
        );
        assert!(
            message.contains("Inspect the process before stopping it"),
            "{message}"
        );
        assert!(message.contains("<PID>"), "{message}");
        assert!(message.contains("PROXY_PORT=40590"), "{message}");
        assert!(message.contains("Do not stop the PID unless"), "{message}");
        if cfg!(target_os = "windows") {
            assert!(message.contains("Fix on Windows (PowerShell)"), "{message}");
            assert!(message.contains("Get-NetTCPConnection"), "{message}");
            assert!(message.contains("Stop-Process"), "{message}");
        } else if cfg!(target_os = "macos") {
            assert!(message.contains("Fix on macOS"), "{message}");
            assert!(message.contains("lsof -nP"), "{message}");
        } else {
            assert!(message.contains("Fix on Linux"), "{message}");
            assert!(message.contains("ss -ltnp"), "{message}");
            assert!(message.contains("ps -p <PID>"), "{message}");
        }
    }
}
