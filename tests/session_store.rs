use freemodel_workbuddy_proxy::session_store::{SessionStore, automatic_session_id};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

#[tokio::test]
async fn crud_history_bounds_and_private_file() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let path = dir.path().join("sessions.json");
    let store = SessionStore::new(&path, 1);
    let session = store
        .create(
            project.to_str().unwrap(),
            "Test",
            Some("proxy-12345678"),
            false,
        )
        .await
        .unwrap();
    store
        .append_history(
            &session.id,
            vec![
                json!({"role":"user","content":"a"}),
                json!({"role":"assistant","content":"b"}),
                json!({"role":"user","content":"c"}),
            ],
        )
        .await
        .unwrap();
    let loaded = store.get(&session.id).await.unwrap().unwrap();
    assert_eq!(loaded.history.len(), 2);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        store
            .list(Some(project.to_str().unwrap()))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store.delete(&session.id).await.unwrap());
}
#[tokio::test]
async fn automatic_id_stays_stable_when_history_grows_and_is_project_scoped() {
    let root = tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let first = vec![
        json!({"role":"system","content":"rules"}),
        json!({"role":"user","content":"hello"}),
    ];
    let mut grown = first.clone();
    grown.push(json!({"role":"assistant","content":"hi"}));
    grown.push(json!({"role":"user","content":"next"}));
    assert_eq!(
        automatic_session_id(a.to_str().unwrap(), &first).unwrap(),
        automatic_session_id(a.to_str().unwrap(), &grown).unwrap()
    );
    assert_ne!(
        automatic_session_id(a.to_str().unwrap(), &first).unwrap(),
        automatic_session_id(b.to_str().unwrap(), &first).unwrap()
    );
}
#[tokio::test]
async fn rejects_invalid_history() {
    let root = tempdir().unwrap();
    let p = root.path().join("p");
    std::fs::create_dir(&p).unwrap();
    let store = SessionStore::new(root.path().join("s.json"), 10);
    let s = store
        .create(p.to_str().unwrap(), "", Some("proxy-12345678"), false)
        .await
        .unwrap();
    assert!(
        store
            .append_history(&s.id, vec![json!({"role":"invalid","content":"x"})])
            .await
            .is_err()
    );
}
