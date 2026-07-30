use crate::{error::ProxyError, models::SessionRecord, session_store::SessionStore};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid, setsid},
};
use parking_lot::Mutex as SyncMutex;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::Mutex, time::sleep};

const CODEBUDDY_REQUEST_HEADER: &str = "x-codebuddy-request";
const SIDECAR_LOG_TAIL_BYTES: usize = 4096;

struct Managed {
    child: std::process::Child,
}
#[derive(Clone)]
pub struct SidecarManager {
    store: SessionStore,
    cli: PathBuf,
    runtime: PathBuf,
    startup: Duration,
    idle: Duration,
    max_sidecars: usize,
    locks: Arc<SyncMutex<HashMap<String, Arc<Mutex<()>>>>>,
    last: Arc<SyncMutex<HashMap<String, Instant>>>,
    processes: Arc<SyncMutex<HashMap<String, Managed>>>,
}
impl SidecarManager {
    pub fn new(
        store: SessionStore,
        cli: impl AsRef<Path>,
        runtime: impl AsRef<Path>,
        startup: f64,
        idle: f64,
        max: usize,
    ) -> Self {
        Self {
            store,
            cli: cli.as_ref().into(),
            runtime: runtime.as_ref().into(),
            startup: Duration::from_secs_f64(startup),
            idle: Duration::from_secs_f64(idle),
            max_sidecars: max,
            locks: Default::default(),
            last: Default::default(),
            processes: Default::default(),
        }
    }
    fn lock(&self, id: &str) -> Arc<Mutex<()>> {
        let mut l = self.locks.lock();
        l.entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    pub async fn ensure(&self, session: &SessionRecord) -> Result<String, ProxyError> {
        let guard = self.lock(&session.id).lock_owned().await;
        let current = self
            .store
            .get(&session.id)
            .await?
            .ok_or_else(|| ProxyError::NotFound("Proxy session no longer exists".into()))?;
        let pid = current.sidecar.get("pid").and_then(Value::as_i64);
        let url = current.sidecar.get("url").and_then(Value::as_str);
        let marker = format!("proxy_{}", session.id);
        if let (Some(pid), Some(url)) = (pid, url)
            && process_matches(pid, &marker)
            && healthy(url).await
        {
            self.last.lock().insert(session.id.clone(), Instant::now());
            return Ok(url.into());
        }
        if !self.cli.is_file() {
            return Err(ProxyError::Invalid(format!(
                "Official CodeBuddy CLI was not found: {}",
                self.cli.display()
            )));
        }
        if self.processes.lock().len() >= self.max_sidecars {
            return Err(ProxyError::Upstream(format!(
                "Maximum active proxy sidecars reached ({})",
                self.max_sidecars
            )));
        }
        fs::create_dir_all(&self.runtime).map_err(ioerr)?;
        fs::set_permissions(&self.runtime, fs::Permissions::from_mode(0o700)).map_err(ioerr)?;
        let port = free_port()?;
        let url = format!("http://127.0.0.1:{port}");
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.runtime.join(format!("{}.log", session.id)))
            .map_err(ioerr)?;
        let err = log.try_clone().map_err(ioerr)?;
        let mut command = Command::new(&self.cli);
        configure_sidecar_environment(&mut command, &self.runtime);
        command
            .args([
                "--serve",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--session-id",
                &marker,
                "--permission-mode",
                "bypassPermissions",
            ])
            .current_dir(&current.project)
            .env("WORKBUDDY_PROXY_SIDECAR", "1")
            .env("WORKBUDDY_PROXY_SESSION", &session.id)
            .env("CODEBUDDY_GATEWAY_AUTH", "none")
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err);
        unsafe {
            command.pre_exec(|| setsid().map(|_| ()).map_err(std::io::Error::other));
        }
        let log_path = self.runtime.join(format!("{}.log", session.id));
        let mut child = command.spawn().map_err(ioerr)?;
        let health_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(1500))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                ProxyError::Internal(format!("Unable to create sidecar health client: {error}"))
            })?;
        let deadline = Instant::now() + self.startup;
        loop {
            if let Some(code) = child.try_wait().map_err(ioerr)? {
                return Err(sidecar_startup_error(
                    format!("CodeBuddy sidecar exited with status {code}"),
                    &log_path,
                ));
            }
            let health_error = match health(&health_client, &url).await {
                Ok(()) => {
                    let mut sidecar = Map::new();
                    sidecar.insert("pid".into(), json!(child.id()));
                    sidecar.insert("port".into(), json!(port));
                    sidecar.insert("url".into(), json!(url));
                    sidecar.insert("marker".into(), json!(marker));
                    sidecar.insert(
                        "started_at".into(),
                        json!(
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs_f64()
                        ),
                    );
                    if let Err(e) = self
                        .store
                        .update(&session.id, None, None, Some(sidecar))
                        .await
                    {
                        terminate_child(&mut child);
                        return Err(e);
                    }
                    self.last.lock().insert(session.id.clone(), Instant::now());
                    self.processes
                        .lock()
                        .insert(session.id.clone(), Managed { child });
                    drop(guard);
                    return Ok(url);
                }
                Err(error) => error,
            };
            if Instant::now() >= deadline {
                terminate_child(&mut child);
                return Err(sidecar_startup_error(
                    format!("Timed out waiting for the CodeBuddy sidecar ({health_error})"),
                    &log_path,
                ));
            }
            sleep(Duration::from_millis(200)).await;
        }
    }
    pub async fn stop(&self, id: &str) -> Result<bool, ProxyError> {
        let sidecar = self
            .store
            .get(id)
            .await?
            .ok_or_else(|| ProxyError::NotFound("Unknown proxy session".into()))?
            .sidecar;
        let pid = sidecar.get("pid").and_then(Value::as_i64);
        let marker = sidecar
            .get("marker")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("proxy_{id}"));
        let mut stopped = false;
        if let Some(mut managed) = self.processes.lock().remove(id) {
            terminate_child(&mut managed.child);
            stopped = true;
        } else if let Some(pid) = pid.filter(|p| process_matches(*p, &marker)) {
            let p = Pid::from_raw(pid as i32);
            let _ = killpg(p, Signal::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(5);
            while Path::new(&format!("/proc/{pid}")).exists() && Instant::now() < deadline {
                sleep(Duration::from_millis(100)).await;
            }
            if Path::new(&format!("/proc/{pid}")).exists() && process_matches(pid, &marker) {
                let _ = killpg(p, Signal::SIGKILL);
            }
            stopped = true;
        }
        self.store.clear_sidecar(id).await?;
        self.last.lock().remove(id);
        Ok(stopped)
    }
    pub async fn reap_idle(&self) -> Result<(), ProxyError> {
        if self.idle.is_zero() {
            return Ok(());
        }
        let expired: Vec<_> = self
            .last
            .lock()
            .iter()
            .filter(|(_, t)| t.elapsed() >= self.idle)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            let _ = self.stop(&id).await;
        }
        Ok(())
    }
    pub async fn stop_all(&self) {
        let ids: Vec<_> = self.processes.lock().keys().cloned().collect();
        for id in ids {
            let _ = self.stop(&id).await;
        }
    }
}
const SIDECAR_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TMPDIR",
    "TEMP",
    "TMP",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "WORKBUDDY_APP_NAME",
    "WORKBUDDY_APP_PATH",
    "WORKBUDDY_APP_VERSION",
    "WORKBUDDY_CONFIG_DIR",
    "WORKBUDDY_DATA_FOLDER_NAME",
    "WORKBUDDY_EXTRA_PATHS",
    "WORKBUDDY_IS_PACKAGED",
    "WORKBUDDY_LOCALE",
    "WORKBUDDY_NODE_ENV",
    "WORKBUDDY_PRODUCT_NAME",
    "WORKBUDDY_PROMPT_TEMPLATES_DIR",
    "WORKBUDDY_RESOURCES_PATH",
    "WORKBUDDY_USER_DATA_DIR",
    "CODEBUDDY_BUILTIN_SKILLS_DIR",
    "CODEBUDDY_CONFIG_DIR",
    "CODEBUDDY_HOST",
    "CODEBUDDY_INTERNET_ENVIRONMENT",
    "CODEBUDDY_NODE_BIN",
    "CLIENT_INFO_IDE_TYPE",
    "CLIENT_INFO_MACHINE_ID",
    "CLIENT_INFO_PLATFORM",
    "CLIENT_INFO_PLATFORM_VERSION",
    "CLIENT_INFO_PLUGIN_NAME",
    "CLIENT_INFO_PLUGIN_VERSION",
    "CLIENT_INFO_PRODUCT_NAME",
    "CLIENT_INFO_PRODUCT_VERSION",
    "CLIENT_INFO_USER_AGENT_EXTENSION",
];

fn configure_sidecar_environment(command: &mut Command, runtime: &Path) {
    command.env_clear();
    for name in SIDECAR_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let capture = runtime.join(".test-sidecar-env.json");
    if capture.exists() {
        command.env("WORKBUDDY_PROXY_TEST_ENV_CAPTURE", capture);
    }
    let exit_marker = runtime.join(".test-sidecar-exit");
    if exit_marker.exists() {
        command.env("WORKBUDDY_PROXY_TEST_EXIT", "1");
    }
}

pub fn process_matches(pid: i64, marker: &str) -> bool {
    let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let args: Vec<_> = raw
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    args.iter().any(|a| {
        Path::new(a)
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.contains("codebuddy"))
    }) && args.iter().any(|a| a == "--serve")
        && args.iter().any(|a| a == marker)
}
fn terminate_child(child: &mut std::process::Child) {
    if child.try_wait().ok().flatten().is_some() {
        let _ = child.wait();
        return;
    }
    let p = Pid::from_raw(child.id() as i32);
    let _ = killpg(p, Signal::SIGTERM);
    for _ in 0..50 {
        if child.try_wait().ok().flatten().is_some() {
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = killpg(p, Signal::SIGKILL);
    let _ = child.wait();
}
fn free_port() -> Result<u16, ProxyError> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).map_err(ioerr)?;
    Ok(listener.local_addr().map_err(ioerr)?.port())
}
async fn healthy(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return false;
    };
    health(&client, url).await.is_ok()
}

async fn health(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let response = client
        .get(format!("{url}/api/v1/health"))
        .header(CODEBUDDY_REQUEST_HEADER, "1")
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() {
                "sidecar has not accepted connections yet".to_string()
            } else if error.is_timeout() {
                "sidecar health check timed out".to_string()
            } else {
                format!("sidecar health request failed: {error}")
            }
        })?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "sidecar health endpoint returned HTTP {}",
            response.status()
        ))
    }
}

fn sidecar_startup_error(message: String, log_path: &Path) -> ProxyError {
    let tail = fs::read(log_path)
        .ok()
        .map(|bytes| {
            let start = bytes.len().saturating_sub(SIDECAR_LOG_TAIL_BYTES);
            sanitize_log_tail(&String::from_utf8_lossy(&bytes[start..]))
                .trim()
                .to_string()
        })
        .filter(|tail| !tail.is_empty());
    let detail = tail
        .map(|tail| format!(" Recent log: {}", tail.replace('\n', " | ")))
        .unwrap_or_default();
    ProxyError::Upstream(format!(
        "{message}. Sidecar log: {}.{detail}",
        log_path.display()
    ))
}

#[doc(hidden)]
pub fn sanitize_log_tail(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b && index + 1 < bytes.len() {
            match bytes[index + 1] {
                b'[' => {
                    index += 2;
                    while index < bytes.len() {
                        let byte = bytes[index];
                        index += 1;
                        if (0x40..=0x7e).contains(&byte) {
                            break;
                        }
                    }
                    continue;
                }
                b']' => {
                    index += 2;
                    while index < bytes.len() {
                        if bytes[index] == 0x07 {
                            index += 1;
                            break;
                        }
                        if bytes[index] == 0x1b
                            && index + 1 < bytes.len()
                            && bytes[index + 1] == b'\\'
                        {
                            index += 2;
                            break;
                        }
                        index += 1;
                    }
                    continue;
                }
                _ => {
                    index += 2;
                    continue;
                }
            }
        }
        let remaining = &input[index..];
        let Some(character) = remaining.chars().next() else {
            break;
        };
        index += character.len_utf8();
        if !character.is_control() || matches!(character, '\n' | '\t') {
            output.push(character);
        }
    }
    output
}

fn ioerr(e: std::io::Error) -> ProxyError {
    ProxyError::Internal(format!("Sidecar I/O failed: {e}"))
}
