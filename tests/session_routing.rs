use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
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
            "FREEMODEL_BASE_URL".into(),
            "https://work.freemodel.dev/v1".into(),
        ),
        ("FREEMODEL_TRANSPORT".into(), "workbuddy_acp".into()),
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
async fn acp_rejects_function_tools_before_sidecar_resolution() {
    let (_root, state, _project) = state();
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));
    let chat = app
        .clone()
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "messages":[{"role":"user","content":"hello"}],
                        "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat.status(), StatusCode::BAD_REQUEST);
    let body = String::from_utf8(
        chat.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("not supported by the WorkBuddy ACP transport"));

    let responses = app
        .oneshot(
            Request::post("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "input":"hello",
                        "stream":true,
                        "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(responses.status(), StatusCode::BAD_REQUEST);
    let body = String::from_utf8(
        responses
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("not supported by the WorkBuddy ACP transport"));
}

#[tokio::test]
async fn model_discovery_supports_openai_and_codex_schemas() {
    let (_root, state, _project) = state();
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));
    let response = app
        .oneshot(Request::get("/v1/models").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["object"], "list");
    assert!(body["data"].is_array());
    assert!(body["models"].is_array());
    assert_eq!(body["data"], body["models"]);
    assert!(
        body["models"]
            .as_array()
            .is_some_and(|models| !models.is_empty())
    );
}

#[tokio::test]
async fn management_crud_and_validation() {
    let (_root, state, project) = state();
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));
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
async fn health_exposes_current_build_identity() {
    let (_root, state, _project) = state();
    let response = router(state)
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let health: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(health["service"], "freemodel-proxy");
    assert_eq!(health["build_id"], freemodel_workbuddy_proxy::BUILD_ID);
}

#[tokio::test]
async fn management_rename_clear_history_and_diagnostics() {
    let (_root, state, project) = state();
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));
    let response = app
        .clone()
        .oneshot(
            Request::post("/proxy/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"project":project,"title":"Before"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let session: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = session["id"].as_str().unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::patch(format!("/proxy/sessions/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"After"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let renamed: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(renamed["title"], "After");
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/proxy/sessions/{id}/history"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"messages":[{"role":"user","content":"x"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(
            Request::put(format!("/proxy/sessions/{id}/history"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"messages":[{"role":"user","content":"replacement"},{"role":"assistant","content":"ok"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let replaced: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(replaced["history"].as_array().unwrap().len(), 2);
    assert_eq!(replaced["history"][0]["content"], "replacement");
    let response = app
        .clone()
        .oneshot(
            Request::delete(format!("/proxy/sessions/{id}/history"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let cleared: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(cleared["history"], json!([]));
    let response = app
        .oneshot(
            Request::get("/proxy/diagnostics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let diagnostics: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(diagnostics["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(diagnostics["build_id"], freemodel_workbuddy_proxy::BUILD_ID);
    assert!(diagnostics["uptime_seconds"].as_u64().is_some());
    assert!(diagnostics.get("active_sidecars").is_some());
}
#[tokio::test]
async fn management_rename_rejects_empty_title() {
    let (_root, state, project) = state();
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));
    let response = app
        .clone()
        .oneshot(
            Request::post("/proxy/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"project":project,"title":"Before"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let session: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = session["id"].as_str().unwrap();
    let response = app
        .oneshot(
            Request::patch(format!("/proxy/sessions/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"  "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn management_routes_reject_non_loopback_but_accept_ipv6_loopback() {
    let (_root, state, _project) = state();
    let denied = router(state.clone())
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [192, 0, 2, 10],
            40000,
        ))))
        .oneshot(
            Request::get("/proxy/diagnostics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let allowed = router(state)
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            std::net::Ipv6Addr::LOCALHOST,
            40000,
        ))))
        .oneshot(
            Request::get("/proxy/diagnostics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn cors_preflight_allows_local_origin_and_required_methods() {
    let (_root, state, _project) = state();
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        let response = router(state.clone())
            .layer(MockConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                40000,
            ))))
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/proxy/sessions")
                    .header("origin", "http://127.0.0.1")
                    .header("access-control-request-method", method)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{method}");
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("http://127.0.0.1")
        );
    }
}

#[tokio::test]
async fn cors_does_not_approve_unlisted_origin() {
    let (_root, state, _project) = state();
    let response = router(state)
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))))
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/proxy/sessions")
                .header("origin", "https://attacker.example")
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
}

#[tokio::test]
async fn explicit_session_project_mismatch_fails_before_sidecar() {
    let (_root, state, project) = state();
    let other = std::path::Path::new(&project)
        .parent()
        .unwrap()
        .join("other");
    std::fs::create_dir(&other).unwrap();
    let session = state
        .store
        .create(&project, "Test", Some("proxy-routing1"), false)
        .await
        .unwrap();
    let response = router(state)
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))))
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-workbuddy-session", session.id)
                .header("x-workbuddy-project", other.to_string_lossy().as_ref())
                .body(Body::from(
                    r#"{"messages":[{"role":"user","content":"x"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn malformed_management_bodies_return_client_errors() {
    let (_root, state, project) = state();
    let app = router(state).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        40000,
    ))));
    let invalid_create = app
        .clone()
        .oneshot(
            Request::post("/proxy/sessions")
                .header("content-type", "application/json")
                .body(Body::from("[]"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_create.status(), StatusCode::BAD_REQUEST);

    let created = app
        .clone()
        .oneshot(
            Request::post("/proxy/sessions")
                .header("content-type", "application/json")
                .body(Body::from(json!({"project":project}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let session: Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = session["id"].as_str().unwrap();
    for method in ["POST", "PUT"] {
        let invalid_history = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/proxy/sessions/{id}/history"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"messages":"not-an-array"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            invalid_history.status(),
            StatusCode::BAD_REQUEST,
            "{method}"
        );
    }
}

#[tokio::test]
async fn unknown_explicit_session_fails_before_sidecar() {
    let (_root, state, project) = state();
    let response = router(state)
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))))
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
