use freemodel_workbuddy_proxy::config::{Config, is_protected_workbuddy_url};
use std::collections::HashMap;
use tempfile::tempdir;

#[test]
fn defaults_to_public_http_and_loopback() {
    let dir = tempdir().unwrap();
    let env = HashMap::from([("HOME".into(), dir.path().to_string_lossy().to_string())]);
    let config = Config::load_with_env(dir.path(), &env).unwrap();
    assert_eq!(config.base_url, "https://api.freemodel.dev/v1");
    assert_eq!(config.transport, "http");
    assert_eq!(config.host, "127.0.0.1");
}
#[test]
fn environment_precedes_saved_config() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"FREEMODEL_BASE_URL":"https://saved.example/v1","FREEMODEL_TRANSPORT":"http"}"#,
    )
    .unwrap();
    let env = HashMap::from([
        ("HOME".into(), dir.path().to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://env.example/v1/".into(),
        ),
    ]);
    assert_eq!(
        Config::load_with_env(dir.path(), &env).unwrap().base_url,
        "https://env.example/v1"
    );
}
#[test]
fn protected_host_requires_acp_but_lookalike_does_not() {
    assert!(is_protected_workbuddy_url("https://work.freemodel.dev/v1").unwrap());
    assert!(!is_protected_workbuddy_url("https://work.freemodel.dev.attacker.example/v1").unwrap());
    let dir = tempdir().unwrap();
    let env = HashMap::from([
        ("HOME".into(), dir.path().to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://work.freemodel.dev/v1".into(),
        ),
        ("FREEMODEL_TRANSPORT".into(), "http".into()),
    ]);
    assert!(Config::load_with_env(dir.path(), &env).is_err());
}
#[test]
fn key_falls_back_to_codex_auth_without_source_embedding() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".codex")).unwrap();
    std::fs::write(
        dir.path().join(".codex/auth.json"),
        r#"{"OPENAI_API_KEY":"test-only-key"}"#,
    )
    .unwrap();
    let env = HashMap::from([("HOME".into(), dir.path().to_string_lossy().to_string())]);
    assert_eq!(
        Config::load_with_env(dir.path(), &env).unwrap().api_key,
        "test-only-key"
    );
    assert!(!include_str!("../src/config.rs").contains("test-only-key"));
}
