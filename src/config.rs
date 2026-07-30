use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};
use url::Url;

use crate::{error::ProxyError, models::ModelInfo};

pub const DEFAULT_PUBLIC_BASE_URL: &str = "https://api.freemodel.dev/v1";
pub const DEFAULT_CODEBUDDY_CLI: &str = "/home/potterparker/workbuddy-linux/workbuddy-app/resources/app.asar.unpacked/cli/bin/codebuddy";

#[derive(Clone, Debug)]
pub struct Config {
    pub project_root: PathBuf,
    pub config_file: PathBuf,
    pub base_url: String,
    pub api_key: String,
    pub transport: String,
    pub workbuddy_acp_url: String,
    pub workbuddy_acp_password: String,
    pub workbuddy_acp_cwd: PathBuf,
    pub workbuddy_acp_timeout: f64,
    pub workbuddy_acp_max_attempts: usize,
    pub workbuddy_cli_path: PathBuf,
    pub session_store: PathBuf,
    pub runtime_dir: PathBuf,
    pub default_project: PathBuf,
    pub sidecar_startup_timeout: f64,
    pub sidecar_idle_timeout: f64,
    pub max_history_turns: usize,
    pub host: String,
    pub port: u16,
    pub cors_origins: Vec<String>,
    pub proxy_api_key: String,
    pub max_sidecars: usize,
    pub models: Vec<ModelInfo>,
}

impl Config {
    pub fn load(project_root: impl AsRef<Path>) -> Result<Self, ProxyError> {
        Self::load_with_env(project_root, &env::vars().collect())
    }

    pub fn load_with_env(
        project_root: impl AsRef<Path>,
        environment: &HashMap<String, String>,
    ) -> Result<Self, ProxyError> {
        let project_root = project_root.as_ref().to_path_buf();
        let config_file = project_root.join("config.json");
        let saved = read_object(&config_file);
        let get = |name: &str, fallback: Value| -> Value {
            saved
                .get(name)
                .cloned()
                .or_else(|| environment.get(name).map(|v| Value::String(v.clone())))
                .unwrap_or(fallback)
        };
        let text = |name: &str, fallback: &str| -> String {
            value_text(get(name, Value::String(fallback.into())))
        };

        let base_url = text("FREEMODEL_BASE_URL", DEFAULT_PUBLIC_BASE_URL)
            .trim_end_matches('/')
            .to_string();
        let protected = is_protected_workbuddy_url(&base_url)?;
        let transport_default = if protected { "workbuddy_acp" } else { "http" };
        let transport = text("FREEMODEL_TRANSPORT", transport_default)
            .trim()
            .to_lowercase();
        if transport != "http" && transport != "workbuddy_acp" {
            return Err(ProxyError::Invalid(format!(
                "Unsupported FREEMODEL_TRANSPORT: {transport}"
            )));
        }
        if protected && transport != "workbuddy_acp" {
            return Err(ProxyError::Invalid(
                "https://work.freemodel.dev requires FREEMODEL_TRANSPORT=workbuddy_acp".into(),
            ));
        }

        let home = environment
            .get("HOME")
            .map(PathBuf::from)
            .or_else(env::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut api_key = text("FREEMODEL_API_KEY", "").trim().to_string();
        if api_key.is_empty() {
            let auth = read_object(&home.join(".codex/auth.json"));
            api_key = auth
                .get("FREEMODEL_API_KEY")
                .or_else(|| auth.get("OPENAI_API_KEY"))
                .map(|value| value_text(value.clone()))
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        let acp_cwd = expand_path(
            &text("WORKBUDDY_ACP_CWD", project_root.to_string_lossy().as_ref()),
            &home,
        );
        let startup = parse_f64(
            get("PROXY_SIDECAR_STARTUP_TIMEOUT", Value::from(90.0)),
            "PROXY_SIDECAR_STARTUP_TIMEOUT",
        )?;
        let idle = parse_f64(
            get("PROXY_SIDECAR_IDLE_TIMEOUT", Value::from(900.0)),
            "PROXY_SIDECAR_IDLE_TIMEOUT",
        )?;
        let acp_timeout = parse_f64(
            get("WORKBUDDY_ACP_TIMEOUT", Value::from(180.0)),
            "WORKBUDDY_ACP_TIMEOUT",
        )?;
        let attempts = parse_usize(
            get("WORKBUDDY_ACP_MAX_ATTEMPTS", Value::from(4)),
            "WORKBUDDY_ACP_MAX_ATTEMPTS",
        )?;
        let max_history = parse_usize(
            get("PROXY_MAX_HISTORY_TURNS", Value::from(100)),
            "PROXY_MAX_HISTORY_TURNS",
        )?;
        let max_sidecars = parse_usize(
            get("PROXY_MAX_SIDECARS", Value::from(8)),
            "PROXY_MAX_SIDECARS",
        )?;
        if !startup.is_finite()
            || !acp_timeout.is_finite()
            || !idle.is_finite()
            || startup <= 0.0
            || acp_timeout <= 0.0
            || attempts == 0
            || max_history == 0
            || max_sidecars == 0
            || idle < 0.0
        {
            return Err(ProxyError::Invalid(
                "Timeouts and limits are outside their valid range".into(),
            ));
        }
        let port_raw = text("PROXY_PORT", "40589")
            .parse::<u16>()
            .map_err(|_| ProxyError::Invalid("PROXY_PORT must be between 1 and 65535".into()))?;
        if port_raw == 0 {
            return Err(ProxyError::Invalid(
                "PROXY_PORT must be between 1 and 65535".into(),
            ));
        }
        let host = text("PROXY_HOST", "127.0.0.1").trim().to_string();
        if host.is_empty() {
            return Err(ProxyError::Invalid("PROXY_HOST must not be empty".into()));
        }
        let cors_origins = text("PROXY_CORS_ORIGINS", "http://127.0.0.1,http://localhost")
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect();

        Ok(Self {
            project_root: project_root.clone(),
            config_file,
            base_url,
            api_key,
            transport,
            workbuddy_acp_url: text("WORKBUDDY_ACP_URL", "http://127.0.0.1:44741")
                .trim_end_matches('/')
                .to_string(),
            workbuddy_acp_password: text("WORKBUDDY_ACP_PASSWORD", ""),
            workbuddy_acp_cwd: acp_cwd,
            workbuddy_acp_timeout: acp_timeout,
            workbuddy_acp_max_attempts: attempts,
            workbuddy_cli_path: expand_path(
                &text("WORKBUDDY_CLI_PATH", DEFAULT_CODEBUDDY_CLI),
                &home,
            ),
            session_store: expand_path(
                &text(
                    "PROXY_SESSION_STORE",
                    project_root
                        .join(".proxy-sessions.json")
                        .to_string_lossy()
                        .as_ref(),
                ),
                &home,
            ),
            runtime_dir: expand_path(
                &text(
                    "PROXY_RUNTIME_DIR",
                    project_root
                        .join(".proxy-runtime")
                        .to_string_lossy()
                        .as_ref(),
                ),
                &home,
            ),
            default_project: expand_path(
                &text(
                    "PROXY_DEFAULT_PROJECT",
                    project_root.to_string_lossy().as_ref(),
                ),
                &home,
            ),
            sidecar_startup_timeout: startup,
            sidecar_idle_timeout: idle,
            max_history_turns: max_history,
            host,
            port: port_raw,
            cors_origins,
            proxy_api_key: text("PROXY_API_KEY", ""),
            max_sidecars,
            models: available_models(),
        })
    }

    pub fn save_api_key(&self, key: &str) -> Result<(), ProxyError> {
        let mut object = read_object(&self.config_file);
        object.insert("FREEMODEL_API_KEY".into(), Value::String(key.trim().into()));
        let bytes =
            serde_json::to_vec_pretty(&object).map_err(|e| ProxyError::Internal(e.to_string()))?;
        let parent = self.config_file.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|e| ProxyError::Internal(format!("Unable to create config directory: {e}")))?;
        let mut temp = tempfile::Builder::new()
            .prefix(".freemodel-config.")
            .tempfile_in(parent)
            .map_err(|e| ProxyError::Internal(format!("Unable to create config file: {e}")))?;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| ProxyError::Internal(format!("Unable to protect config file: {e}")))?;
        temp.write_all(&bytes)
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|e| ProxyError::Internal(format!("Unable to save API key: {e}")))?;
        temp.persist(&self.config_file)
            .map_err(|e| ProxyError::Internal(format!("Unable to save API key: {}", e.error)))?;
        fs::set_permissions(&self.config_file, fs::Permissions::from_mode(0o600))
            .map_err(|e| ProxyError::Internal(format!("Unable to protect config file: {e}")))
    }
}

pub fn upstream_hostname(base_url: &str) -> Result<String, ProxyError> {
    let parsed = Url::parse(base_url)
        .map_err(|_| ProxyError::Invalid(format!("Invalid FREEMODEL_BASE_URL: {base_url}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ProxyError::Invalid(format!(
            "Invalid FREEMODEL_BASE_URL: {base_url}"
        )));
    }
    parsed
        .host_str()
        .map(|v| v.to_lowercase())
        .ok_or_else(|| ProxyError::Invalid(format!("Invalid FREEMODEL_BASE_URL: {base_url}")))
}

pub fn is_protected_workbuddy_url(base_url: &str) -> Result<bool, ProxyError> {
    Ok(upstream_hostname(base_url)? == "work.freemodel.dev")
}

fn read_object(path: &Path) -> Map<String, Value> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}
fn value_text(value: Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string().trim_matches('"').to_string())
}
fn parse_f64(value: Value, name: &str) -> Result<f64, ProxyError> {
    value_text(value)
        .parse()
        .map_err(|_| ProxyError::Invalid(format!("{name} must be a number")))
}
fn parse_usize(value: Value, name: &str) -> Result<usize, ProxyError> {
    value_text(value)
        .parse()
        .map_err(|_| ProxyError::Invalid(format!("{name} must be a positive integer")))
}
fn expand_path(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        home.to_path_buf()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(value)
    }
}
fn available_models() -> Vec<ModelInfo> {
    ["gpt-5.6-sol", "gpt 5.6 sol", "gpt-4o", "opencode-default"]
        .into_iter()
        .map(|id| ModelInfo {
            id: id.into(),
            object: "model".into(),
            created: 1_785_164_333,
            owned_by: "freemodel".into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_protected_host() {
        assert!(is_protected_workbuddy_url("https://work.freemodel.dev/v1").unwrap());
        assert!(!is_protected_workbuddy_url("https://work.freemodel.dev.attacker/v1").unwrap());
    }
}
