use freemodel_workbuddy_proxy::config::{Config, is_protected_workbuddy_url};
use std::{collections::HashMap, os::unix::fs::PermissionsExt};
use tempfile::tempdir;

#[test]
fn defaults_to_workbuddy_service_over_official_acp() {
    let dir = tempdir().unwrap();
    let env = HashMap::from([("HOME".into(), dir.path().to_string_lossy().to_string())]);
    let config = Config::load_with_env(dir.path(), &env).unwrap();
    assert_eq!(config.base_url, "https://work.freemodel.dev/v1");
    assert_eq!(config.transport, "workbuddy_acp");
    assert_eq!(
        config.workbuddy_cli_path,
        std::path::PathBuf::from("codebuddy")
    );
}

#[test]
fn configured_public_url_defaults_to_http_and_loopback() {
    let dir = tempdir().unwrap();
    let env = HashMap::from([
        ("HOME".into(), dir.path().to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://upstream.example/v1/".into(),
        ),
    ]);
    let config = Config::load_with_env(dir.path(), &env).unwrap();
    assert_eq!(config.base_url, "https://upstream.example/v1");
    assert_eq!(config.transport, "http");
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.max_sidecars, 16);
}
#[test]
fn default_project_is_canonical_and_must_be_a_directory() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&project, &alias).unwrap();
    let env = HashMap::from([
        ("HOME".into(), root.path().to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://example.test/v1".into(),
        ),
        (
            "PROXY_DEFAULT_PROJECT".into(),
            alias.to_string_lossy().to_string(),
        ),
    ]);
    assert_eq!(
        Config::load_with_env(root.path(), &env)
            .unwrap()
            .default_project,
        std::fs::canonicalize(project).unwrap()
    );

    for invalid in [root.path().join("missing"), root.path().join("file.txt")] {
        if invalid.extension().is_some() {
            std::fs::write(&invalid, "x").unwrap();
        }
        let env = HashMap::from([
            ("HOME".into(), root.path().to_string_lossy().to_string()),
            (
                "FREEMODEL_BASE_URL".into(),
                "https://example.test/v1".into(),
            ),
            (
                "PROXY_DEFAULT_PROJECT".into(),
                invalid.to_string_lossy().to_string(),
            ),
        ]);
        assert!(Config::load_with_env(root.path(), &env).is_err());
    }
}

#[test]
fn saved_project_config_precedes_inherited_environment() {
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
        "https://saved.example/v1"
    );
}

#[test]
fn inherited_workbuddy_environment_cannot_override_explicit_public_project_config() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"FREEMODEL_API_KEY":"saved-public-key","FREEMODEL_BASE_URL":"https://generic.example/v1","FREEMODEL_TRANSPORT":"http"}"#,
    )
    .unwrap();
    let env = HashMap::from([
        ("HOME".into(), dir.path().to_string_lossy().to_string()),
        ("FREEMODEL_API_KEY".into(), "inherited-workbuddy-key".into()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://work.freemodel.dev/v1".into(),
        ),
        ("FREEMODEL_TRANSPORT".into(), "workbuddy_acp".into()),
    ]);
    let config = Config::load_with_env(dir.path(), &env).unwrap();
    assert_eq!(config.api_key, "saved-public-key");
    assert_eq!(config.base_url, "https://generic.example/v1");
    assert_eq!(config.transport, "http");
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
fn protected_host_url_variants_are_exact_and_case_insensitive() {
    for protected in [
        "https://WORK.FREEMODEL.DEV/v1/",
        "https://work.freemodel.dev:443/v1?x=1#fragment",
        "http://work.freemodel.dev/v1",
    ] {
        assert!(
            is_protected_workbuddy_url(protected).unwrap(),
            "{protected}"
        );
    }
    for public in [
        "https://generic.example/v1",
        "https://work.freemodel.dev.example/v1",
        "https://example.com/work.freemodel.dev/v1",
        "http://127.0.0.1:40589/v1",
        "http://[::1]:40589/v1",
    ] {
        assert!(!is_protected_workbuddy_url(public).unwrap(), "{public}");
    }
    for invalid in [
        "",
        "work.freemodel.dev",
        "ftp://work.freemodel.dev/v1",
        "/v1",
    ] {
        assert!(is_protected_workbuddy_url(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn rejects_invalid_transports_ports_timeouts_and_limits() {
    let cases = [
        ("FREEMODEL_TRANSPORT", "socket"),
        ("PROXY_PORT", "0"),
        ("PROXY_PORT", "65536"),
        ("PROXY_PORT", "nope"),
        ("WORKBUDDY_ACP_TIMEOUT", "0"),
        ("WORKBUDDY_ACP_TIMEOUT", "NaN"),
        ("WORKBUDDY_ACP_MAX_ATTEMPTS", "0"),
        ("PROXY_MAX_HISTORY_TURNS", "-1"),
        ("PROXY_MAX_SIDECARS", "0"),
        ("PROXY_SIDECAR_STARTUP_TIMEOUT", "-2"),
        ("PROXY_SIDECAR_IDLE_TIMEOUT", "-1"),
        ("PROXY_HOST", ""),
    ];
    for (key, value) in cases {
        let dir = tempdir().unwrap();
        let env = HashMap::from([
            ("HOME".into(), dir.path().to_string_lossy().to_string()),
            (key.into(), value.into()),
        ]);
        assert!(
            Config::load_with_env(dir.path(), &env).is_err(),
            "{key}={value} must fail"
        );
    }
}

#[test]
fn api_key_save_is_atomic_private_and_preserves_unrelated_keys() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"FREEMODEL_BASE_URL":"https://saved.example/v1","UNRELATED":{"keep":true}}"#,
    )
    .unwrap();
    let env = HashMap::from([("HOME".into(), dir.path().to_string_lossy().to_string())]);
    let config = Config::load_with_env(dir.path(), &env).unwrap();
    config.save_api_key("  test-save-key  ").unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["FREEMODEL_API_KEY"], "test-save-key");
    assert_eq!(saved["FREEMODEL_BASE_URL"], "https://saved.example/v1");
    assert_eq!(saved["UNRELATED"]["keep"], true);
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn cli_path_uses_explicit_config_or_path_discovery_without_machine_specific_default() {
    let dir = tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let discovered = bin.join("codebuddy");
    std::fs::write(&discovered, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&discovered, std::fs::Permissions::from_mode(0o700)).unwrap();
    let env = HashMap::from([
        ("HOME".into(), dir.path().to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://example.test/v1".into(),
        ),
        ("PATH".into(), bin.to_string_lossy().to_string()),
    ]);
    assert_eq!(
        Config::load_with_env(dir.path(), &env)
            .unwrap()
            .workbuddy_cli_path,
        discovered
    );

    let explicit = dir.path().join("custom-codebuddy");
    let env = HashMap::from([
        ("HOME".into(), dir.path().to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://example.test/v1".into(),
        ),
        ("PATH".into(), bin.to_string_lossy().to_string()),
        (
            "WORKBUDDY_CLI_PATH".into(),
            explicit.to_string_lossy().to_string(),
        ),
    ]);
    assert_eq!(
        Config::load_with_env(dir.path(), &env)
            .unwrap()
            .workbuddy_cli_path,
        explicit
    );
    assert!(!include_str!("../src/config.rs").contains("/home/potterparker/"));
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
    let env = HashMap::from([
        ("HOME".into(), dir.path().to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://example.test/v1".into(),
        ),
    ]);
    assert_eq!(
        Config::load_with_env(dir.path(), &env).unwrap().api_key,
        "test-only-key"
    );
    assert!(!include_str!("../src/config.rs").contains("test-only-key"));
}
