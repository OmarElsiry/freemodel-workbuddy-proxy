use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Preferences {
    pub version: u32,
    #[serde(default)]
    pub recent_projects: Vec<String>,
    #[serde(default)]
    pub last_sessions: BTreeMap<String, String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_true")]
    pub sidebar: bool,
    #[serde(default)]
    pub no_color: bool,
}
fn default_model() -> String {
    "gpt-5.6-sol".into()
}
fn default_true() -> bool {
    true
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            recent_projects: vec![],
            last_sessions: BTreeMap::new(),
            model: default_model(),
            sidebar: true,
            no_color: std::env::var_os("NO_COLOR").is_some(),
        }
    }
}
impl Preferences {
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(mut value) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        if value.version != 1 {
            return Self::default();
        }
        value.recent_projects.truncate(10);
        value
    }
    pub fn remember(&mut self, project: &str, session: Option<&str>) {
        self.recent_projects.retain(|p| p != project);
        self.recent_projects.insert(0, project.into());
        self.recent_projects.truncate(10);
        if let Some(id) = session {
            self.last_sessions.insert(project.into(), id.into());
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        let parent = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let mut temp = tempfile::Builder::new()
            .prefix(".tui-preferences.")
            .tempfile_in(parent)?;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temp.write_all(&serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        File::open(parent)?.sync_all()
    }
}
pub fn path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("tui-preferences.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_round_trip_has_no_secret_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let mut p = Preferences::default();
        p.remember("/tmp/a", Some("proxy-12345678"));
        p.save(&path).unwrap();
        assert_eq!(Preferences::load(&path), p);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let text = fs::read_to_string(path).unwrap();
        assert!(!text.to_lowercase().contains("api_key"));
    }
    #[test]
    fn malformed_is_safe_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p");
        fs::write(&path, "nope").unwrap();
        assert_eq!(Preferences::load(&path).version, 1);
    }
}
