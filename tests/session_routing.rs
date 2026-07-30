use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use freemodel_workbuddy_proxy::{
    config::Config,
    server::{AppState, router},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use tempfile::tempdir;
use tower::ServiceExt;

fn state() -> (tempfile::TempDir, AppState, String) {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let mut env = HashMap::from([
        ("HOME".into(), root.path().to_string_lossy().to_string()),
        (
            "PROXY_SESSION_STORE".into(),
            root.path()
                .join("sessions.json")
                .to_string_lossy()
                .to_string(),
        ),
        (
            "PROXY_RUNTIME_DIR".into(),
            root.path().join("runtime").to_string_lossy().to_string(),
        ),
        (
            "WORKBUDDY_CLI_PATH".into(),
            root.path().join("missing").to_string_lossy().to_string(),
        ),
    ]);
    env.insert(
        "PROXY_DEFAULT_PROJECT".into(),
        project.to_string_lossy().to_string(),
    );
    let config = Config::load_with_env(root.path(), &env).unwrap();
    let state = AppState::new(config).unwrap();
    (root, state, project.to_string_lossy().to_string())
}
#[tokio::test]
async fn management_crud_and_validation() {
    let (_root, state, project) = state();
    let app = router(state);
    let response = app
        .clone()
        .oneshot(
            Request::post("/proxy/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"project":project,"title":"Test"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let session: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = session["id"].as_str().unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/proxy/sessions/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            Request::delete(format!("/proxy/sessions/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
#[tokio::test]
async fn unknown_explicit_session_fails_before_sidecar() {
    let (_root, state, project) = state();
    let response = router(state)
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-workbuddy-session", "proxy-unknown1")
                .header("x-workbuddy-project", project)
                .body(Body::from(
                    r#"{"messages":[{"role":"user","content":"x"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
