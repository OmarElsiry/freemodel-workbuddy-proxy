use crate::{error::ProxyError, models::SessionRecord};
use chrono::Utc;
use fs2::FileExt;
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct SessionStore {
    path: PathBuf,
    max_history_messages: usize,
    lock: Arc<Mutex<()>>,
}
impl SessionStore {
    pub fn new(path: impl AsRef<Path>, max_turns: usize) -> Self {
        Self {
            path: path.as_ref().into(),
            max_history_messages: max_turns.max(1) * 2,
            lock: Arc::new(Mutex::new(())),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    fn lock_path(&self) -> PathBuf {
        self.path.with_extension("lock")
    }
    fn with_data<T>(
        &self,
        write: bool,
        f: impl FnOnce(&mut Map<String, Value>) -> Result<T, ProxyError>,
    ) -> Result<T, ProxyError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(ioerr)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(ioerr)?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(self.lock_path())
            .map_err(ioerr)?;
        lock.lock_exclusive().map_err(ioerr)?;
        let mut data = if self.path.exists() {
            let v: Value =
                serde_json::from_slice(&fs::read(&self.path).map_err(ioerr)?).map_err(|e| {
                    ProxyError::Internal(format!("Unable to read proxy session store: {e}"))
                })?;
            v.as_object()
                .cloned()
                .ok_or_else(|| ProxyError::Internal("Invalid proxy session store format".into()))?
        } else {
            let mut initial = Map::new();
            initial.insert("version".into(), Value::from(1));
            initial.insert("sessions".into(), Value::Object(Map::new()));
            initial
        };
        if data.get("version").and_then(Value::as_u64) != Some(1)
            || !data.get("sessions").is_some_and(Value::is_object)
        {
            return Err(ProxyError::Internal(
                "Invalid proxy session store format".into(),
            ));
        }
        let result = f(&mut data)?;
        if write {
            self.atomic_write(&Value::Object(data))?;
        }
        FileExt::unlock(&lock).map_err(ioerr)?;
        Ok(result)
    }
    fn atomic_write(&self, value: &Value) -> Result<(), ProxyError> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| ProxyError::Internal("Invalid session store path".into()))?;
        let mut temp = tempfile::Builder::new()
            .prefix(&format!(".{}.", file_name.to_string_lossy()))
            .tempfile_in(parent)
            .map_err(ioerr)?;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(ioerr)?;
        temp.write_all(
            &serde_json::to_vec_pretty(value).map_err(|e| ProxyError::Internal(e.to_string()))?,
        )
        .map_err(ioerr)?;
        temp.as_file().sync_all().map_err(ioerr)?;
        temp.persist(&self.path).map_err(|e| ioerr(e.error))?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).map_err(ioerr)?;
        File::open(parent).and_then(|f| f.sync_all()).map_err(ioerr)
    }
    pub async fn list(&self, project: Option<&str>) -> Result<Vec<SessionRecord>, ProxyError> {
        let _g = self.lock.lock().await;
        let canonical = project.map(canonical_project).transpose()?;
        self.with_data(false, |data| {
            let mut records = records(data)?;
            if let Some(p) = canonical.as_deref() {
                records.retain(|r| r.project == p);
            }
            records.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            Ok(records)
        })
    }
    pub async fn get(&self, id: &str) -> Result<Option<SessionRecord>, ProxyError> {
        let id = validate_session_id(id)?;
        let _g = self.lock.lock().await;
        self.with_data(false, |d| match d["sessions"].get(id) {
            Some(v) => Ok(Some(serde_json::from_value(v.clone()).map_err(formaterr)?)),
            None => Ok(None),
        })
    }
    pub async fn create(
        &self,
        project: &str,
        title: &str,
        id: Option<&str>,
        automatic: bool,
    ) -> Result<SessionRecord, ProxyError> {
        let project = canonical_project(project)?;
        let id = validate_session_id(
            &id.map(str::to_string)
                .unwrap_or_else(|| format!("proxy-{}", Uuid::new_v4().simple())),
        )?
        .to_string();
        let _g = self.lock.lock().await;
        self.with_data(true, |d| {
            if let Some(v) = d["sessions"].get(&id) {
                let record: SessionRecord = serde_json::from_value(v.clone()).map_err(formaterr)?;
                if record.project != project {
                    return Err(ProxyError::Conflict(
                        "Proxy session belongs to a different project".into(),
                    ));
                }
                return Ok(record);
            }
            let now = Utc::now().to_rfc3339();
            let record = SessionRecord {
                id: id.clone(),
                title: if title.is_empty() {
                    Path::new(&project)
                        .file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or("Proxy session")
                        .chars()
                        .take(120)
                        .collect()
                } else {
                    title.chars().take(120).collect()
                },
                project,
                automatic,
                created_at: now.clone(),
                updated_at: now,
                history: vec![],
                sidecar: Map::new(),
                extra: Map::new(),
            };
            let sessions = d
                .get_mut("sessions")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| ProxyError::Internal("Invalid proxy session store format".into()))?;
            sessions.insert(
                id,
                serde_json::to_value(&record).map_err(|e| ProxyError::Internal(e.to_string()))?,
            );
            Ok(record)
        })
    }
    pub async fn automatic(
        &self,
        project: &str,
        messages: &[Value],
    ) -> Result<SessionRecord, ProxyError> {
        let id = automatic_session_id(project, messages)?;
        self.create(project, "Automatic client session", Some(&id), true)
            .await
    }
    pub async fn update(
        &self,
        id: &str,
        title: Option<&str>,
        history: Option<Vec<Value>>,
        sidecar: Option<Map<String, Value>>,
    ) -> Result<SessionRecord, ProxyError> {
        let id = validate_session_id(id)?.to_string();
        let _g = self.lock.lock().await;
        self.with_data(true, |d| {
            let value = d
                .get_mut("sessions")
                .and_then(Value::as_object_mut)
                .and_then(|s| s.get_mut(&id))
                .ok_or_else(|| ProxyError::NotFound("Unknown proxy session".into()))?;
            let mut r: SessionRecord = serde_json::from_value(value.clone()).map_err(formaterr)?;
            if let Some(t) = title
                && !t.is_empty()
            {
                r.title = t.chars().take(120).collect();
            }
            if let Some(h) = history {
                validate_history(&h)?;
                r.history = h
                    .into_iter()
                    .rev()
                    .take(self.max_history_messages)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
            }
            if let Some(s) = sidecar {
                r.sidecar = s;
            }
            r.updated_at = Utc::now().to_rfc3339();
            *value = serde_json::to_value(&r).map_err(|e| ProxyError::Internal(e.to_string()))?;
            Ok(r)
        })
    }
    pub async fn append_history(
        &self,
        id: &str,
        messages: Vec<Value>,
    ) -> Result<SessionRecord, ProxyError> {
        validate_history(&messages)?;
        let id = validate_session_id(id)?.to_string();
        let _g = self.lock.lock().await;
        self.with_data(true, |data| {
            let value = data
                .get_mut("sessions")
                .and_then(Value::as_object_mut)
                .and_then(|sessions| sessions.get_mut(&id))
                .ok_or_else(|| ProxyError::NotFound("Unknown proxy session".into()))?;
            let mut record: SessionRecord =
                serde_json::from_value(value.clone()).map_err(formaterr)?;
            record.history.extend(messages);
            if record.history.len() > self.max_history_messages {
                record.history.drain(
                    ..record
                        .history
                        .len()
                        .saturating_sub(self.max_history_messages),
                );
            }
            record.updated_at = Utc::now().to_rfc3339();
            *value = serde_json::to_value(&record)
                .map_err(|error| ProxyError::Internal(error.to_string()))?;
            Ok(record)
        })
    }
    pub async fn clear_sidecar(&self, id: &str) -> Result<SessionRecord, ProxyError> {
        self.update(id, None, None, Some(Map::new())).await
    }
    pub async fn delete(&self, id: &str) -> Result<bool, ProxyError> {
        let id = validate_session_id(id)?.to_string();
        let _g = self.lock.lock().await;
        self.with_data(true, |d| {
            Ok(d.get_mut("sessions")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| ProxyError::Internal("Invalid proxy session store format".into()))?
                .remove(&id)
                .is_some())
        })
    }
    pub async fn clear_stale_runtime(&self) -> Result<(), ProxyError> {
        let sessions = self.list(None).await?;
        for r in sessions {
            if let Some(pid) = r.sidecar.get("pid").and_then(Value::as_i64)
                && !Path::new(&format!("/proc/{pid}")).exists()
            {
                self.clear_sidecar(&r.id).await?;
            }
        }
        Ok(())
    }
}
pub fn canonical_project(path: &str) -> Result<String, ProxyError> {
    let p = fs::canonicalize(shellexpand(path))
        .map_err(|_| ProxyError::Invalid(format!("Project directory does not exist: {path}")))?;
    if !p.is_dir() {
        return Err(ProxyError::Invalid(format!(
            "Project directory does not exist: {}",
            p.display()
        )));
    }
    Ok(p.to_string_lossy().into())
}
pub fn validate_session_id(id: &str) -> Result<&str, ProxyError> {
    let re = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_-]{7,127}$").unwrap();
    let id = id.trim();
    if re.is_match(id) {
        Ok(id)
    } else {
        Err(ProxyError::Invalid("Invalid proxy session ID".into()))
    }
}
pub fn validate_history(h: &[Value]) -> Result<(), ProxyError> {
    for (index, m) in h.iter().enumerate() {
        let Some(o) = m.as_object() else {
            return Err(ProxyError::Invalid(format!(
                "history item {index} must be an object"
            )));
        };
        if !["system", "developer", "user", "assistant", "tool"]
            .contains(&o.get("role").and_then(Value::as_str).unwrap_or(""))
        {
            return Err(ProxyError::Invalid(format!(
                "history item {index} has an invalid role"
            )));
        }
        if !o.contains_key("content") {
            return Err(ProxyError::Invalid(format!(
                "history item {index} is missing content"
            )));
        }
    }
    Ok(())
}
pub fn stable_context_text(messages: &[Value]) -> String {
    let mut out = vec![];
    for m in messages {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        if !["system", "developer", "user"].contains(&role) {
            continue;
        }
        out.push(format!(
            "{role}:{}",
            crate::openai::text_from_content(m.get("content").unwrap_or(&Value::Null))
        ));
        if role == "user" {
            break;
        }
    }
    out.join("\n")
}
pub fn automatic_session_id(project: &str, messages: &[Value]) -> Result<String, ProxyError> {
    let mut h = Sha256::new();
    h.update(canonical_project(project)?);
    h.update([0]);
    h.update(stable_context_text(messages));
    Ok(format!("auto-{}", &format!("{:x}", h.finalize())[..24]))
}
fn records(d: &Map<String, Value>) -> Result<Vec<SessionRecord>, ProxyError> {
    d.get("sessions")
        .and_then(Value::as_object)
        .ok_or_else(|| ProxyError::Internal("Invalid proxy session store format".into()))?
        .values()
        .cloned()
        .map(|v| serde_json::from_value(v).map_err(formaterr))
        .collect()
}
fn ioerr(e: std::io::Error) -> ProxyError {
    ProxyError::Internal(format!("Session store I/O failed: {e}"))
}
fn formaterr(e: serde_json::Error) -> ProxyError {
    ProxyError::Internal(format!("Invalid proxy session store format: {e}"))
}
fn shellexpand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        std::env::home_dir().unwrap_or_default().join(rest)
    } else {
        p.into()
    }
}
