use freemodel_workbuddy_proxy::{
    session_store::SessionStore,
    sidecar::{SidecarManager, process_matches},
};
use serial_test::serial;
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};
use tempfile::tempdir;

fn fake_codebuddy() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_fake-codebuddy")
        .map(std::path::PathBuf::from)
        .expect("Cargo exposes the fake sidecar binary")
}

#[test]
fn current_test_process_is_not_mistaken_for_sidecar() {
    assert!(!process_matches(
        std::process::id() as i64,
        "proxy-test-marker"
    ));
}
#[test]
fn nonexistent_process_is_not_owned() {
    assert!(!process_matches(99_999_999, "proxy-test-marker"));
}

#[tokio::test]
async fn missing_cli_fails_without_sidecar_metadata_or_runtime_artifacts() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let store = SessionStore::new(root.path().join("sessions.json"), 10);
    let session = store
        .create(
            project.to_str().unwrap(),
            "Test",
            Some("proxy-sidecar1"),
            false,
        )
        .await
        .unwrap();
    let runtime = root.path().join("runtime");
    let manager = SidecarManager::new(
        store.clone(),
        root.path().join("missing-codebuddy"),
        &runtime,
        0.1,
        1.0,
        1,
    );
    let error = manager.ensure(&session).await.unwrap_err().to_string();
    assert!(error.contains("Official CodeBuddy CLI was not found"));
    assert!(
        store
            .get(&session.id)
            .await
            .unwrap()
            .unwrap()
            .sidecar
            .is_empty()
    );
    assert!(!runtime.exists());
}

#[tokio::test]
async fn successful_sidecar_is_reused_and_stopped_safely() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let store = SessionStore::new(root.path().join("sessions.json"), 10);
    let session = store
        .create(
            project.to_str().unwrap(),
            "Test",
            Some("proxy-sidecar3"),
            false,
        )
        .await
        .unwrap();
    let runtime = root.path().join("runtime");
    let manager = SidecarManager::new(store.clone(), fake_codebuddy(), &runtime, 3.0, 0.0, 1);
    let first = manager.ensure(&session).await.unwrap();
    let second = manager.ensure(&session).await.unwrap();
    assert_eq!(first, second);
    let stored = store.get(&session.id).await.unwrap().unwrap();
    assert!(
        stored
            .sidecar
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    );
    assert_eq!(
        stored
            .sidecar
            .get("url")
            .and_then(serde_json::Value::as_str),
        Some(first.as_str())
    );
    assert!(manager.stop(&session.id).await.unwrap());
    assert!(
        store
            .get(&session.id)
            .await
            .unwrap()
            .unwrap()
            .sidecar
            .is_empty()
    );
}

#[tokio::test]
async fn max_sidecars_blocks_a_second_active_session() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let store = SessionStore::new(root.path().join("sessions.json"), 10);
    let first = store
        .create(
            project.to_str().unwrap(),
            "One",
            Some("proxy-sidecar4"),
            false,
        )
        .await
        .unwrap();
    let second = store
        .create(
            project.to_str().unwrap(),
            "Two",
            Some("proxy-sidecar5"),
            false,
        )
        .await
        .unwrap();
    let manager = SidecarManager::new(
        store,
        fake_codebuddy(),
        root.path().join("runtime"),
        3.0,
        0.0,
        1,
    );
    manager.ensure(&first).await.unwrap();
    let error = manager.ensure(&second).await.unwrap_err().to_string();
    assert!(error.contains("Maximum active proxy sidecars reached"));
    manager.stop_all().await;
}

#[tokio::test]
async fn stop_unknown_session_is_safe_not_found() {
    let root = tempdir().unwrap();
    let store = SessionStore::new(root.path().join("sessions.json"), 10);
    let manager = SidecarManager::new(
        store,
        root.path().join("missing-codebuddy"),
        root.path().join("runtime"),
        0.1,
        1.0,
        1,
    );
    assert!(manager.stop("proxy-missing1").await.is_err());
}

#[tokio::test]
#[serial]
async fn sidecar_environment_is_minimized() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let runtime = root.path().join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    let capture = runtime.join(".test-sidecar-env.json");
    std::fs::write(&capture, b"{}").unwrap();
    std::fs::set_permissions(&capture, std::fs::Permissions::from_mode(0o600)).unwrap();

    let store = SessionStore::new(root.path().join("sessions.json"), 10);
    let session = store
        .create(
            project.to_str().unwrap(),
            "Environment",
            Some("proxy-sidecar6"),
            false,
        )
        .await
        .unwrap();

    let original = [
        "PROXY_API_KEY",
        "FREEMODEL_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "CODEBUDDY_GATEWAY_PASSWORD",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "NODE_OPTIONS",
        "PYTHONPATH",
        "BASH_ENV",
    ]
    .map(|name| (name, std::env::var_os(name)));
    unsafe {
        std::env::set_var("PROXY_API_KEY", "proxy-secret");
        std::env::set_var("FREEMODEL_API_KEY", "freemodel-secret");
        std::env::set_var("OPENAI_API_KEY", "openai-secret");
        std::env::set_var("ANTHROPIC_API_KEY", "anthropic-secret");
        std::env::set_var("CODEBUDDY_GATEWAY_PASSWORD", "gateway-secret");
        std::env::set_var("LD_PRELOAD", "/tmp/not-a-real-library.so");
        std::env::set_var("LD_LIBRARY_PATH", "/tmp/untrusted");
        std::env::set_var("NODE_OPTIONS", "--inspect=127.0.0.1:9");
        std::env::set_var("PYTHONPATH", "/tmp/untrusted-python");
        std::env::set_var("BASH_ENV", "/tmp/untrusted-shell");
    }

    let manager = SidecarManager::new(store, fake_codebuddy(), &runtime, 3.0, 0.0, 1);
    let url = manager.ensure(&session).await.unwrap();
    assert!(url.starts_with("http://127.0.0.1:"));
    manager.stop_all().await;

    for (name, value) in original {
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }

    let environment: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(&capture).unwrap()).unwrap();
    assert_eq!(
        environment
            .get("WORKBUDDY_PROXY_SIDECAR")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        environment
            .get("WORKBUDDY_PROXY_SESSION")
            .map(String::as_str),
        Some(session.id.as_str())
    );
    assert_eq!(
        environment
            .get("CODEBUDDY_GATEWAY_AUTH")
            .map(String::as_str),
        Some("none")
    );
    assert!(environment.contains_key("PATH"));
    for name in [
        "PROXY_API_KEY",
        "FREEMODEL_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "CODEBUDDY_GATEWAY_PASSWORD",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "NODE_OPTIONS",
        "PYTHONPATH",
        "BASH_ENV",
    ] {
        assert!(!environment.contains_key(name), "leaked {name}");
    }
    assert_eq!(
        std::fs::metadata(&capture).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn zero_idle_timeout_does_not_stop_or_mutate_sessions() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let store = SessionStore::new(root.path().join("sessions.json"), 10);
    let session = store
        .create(
            project.to_str().unwrap(),
            "Test",
            Some("proxy-sidecar2"),
            false,
        )
        .await
        .unwrap();
    let manager = SidecarManager::new(
        store.clone(),
        root.path().join("missing-codebuddy"),
        root.path().join("runtime"),
        0.1,
        0.0,
        1,
    );
    manager.reap_idle().await.unwrap();
    assert!(store.get(&session.id).await.unwrap().is_some());
}
