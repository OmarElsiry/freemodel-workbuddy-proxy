use freemodel_workbuddy_proxy::session_store::{SessionStore, automatic_session_id};
use serde_json::{Value, json};
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
async fn preserves_extra_fields_and_rejects_corrupt_store() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let path = root.path().join("sessions.json");
    let canonical = std::fs::canonicalize(&project).unwrap();
    std::fs::write(
        &path,
        json!({
            "version":1,
            "sessions":{
                "proxy-extra123":{
                    "id":"proxy-extra123",
                    "title":"Legacy",
                    "project":canonical,
                    "automatic":false,
                    "created_at":"2026-01-01T00:00:00Z",
                    "updated_at":"2026-01-01T00:00:00Z",
                    "history":[],
                    "sidecar":{},
                    "future_field":{"kept":true}
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let store = SessionStore::new(&path, 10);
    let record = store.get("proxy-extra123").await.unwrap().unwrap();
    assert_eq!(record.extra["future_field"]["kept"], true);
    store
        .update("proxy-extra123", Some("Updated"), None, None)
        .await
        .unwrap();
    let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        disk["sessions"]["proxy-extra123"]["future_field"]["kept"],
        true
    );

    std::fs::write(&path, b"not-json").unwrap();
    assert!(store.list(None).await.is_err());
    std::fs::write(&path, r#"{"version":2,"sessions":{}}"#).unwrap();
    assert!(store.list(None).await.is_err());
    std::fs::write(&path, r#"{"version":1,"sessions":[]}"#).unwrap();
    assert!(store.list(None).await.is_err());
}

#[tokio::test]
async fn canonical_paths_handle_aliases_symlinks_unicode_and_files() {
    let root = tempdir().unwrap();
    let project = root.path().join("project with 空格");
    std::fs::create_dir(&project).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&project, &alias).unwrap();
    let messages = vec![json!({"role":"user","content":"hello"})];
    assert_eq!(
        automatic_session_id(project.to_str().unwrap(), &messages).unwrap(),
        automatic_session_id(alias.to_str().unwrap(), &messages).unwrap()
    );

    let file = root.path().join("file.txt");
    std::fs::write(&file, "x").unwrap();
    assert!(automatic_session_id(file.to_str().unwrap(), &messages).is_err());
    assert!(
        automatic_session_id(root.path().join("missing").to_str().unwrap(), &messages).is_err()
    );
}

#[tokio::test]
async fn concurrent_independent_store_instances_leave_valid_complete_json() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let path = root.path().join("sessions.json");
    let a = SessionStore::new(&path, 100);
    let b = SessionStore::new(&path, 100);
    let project_a = project.to_string_lossy().to_string();
    let project_b = project_a.clone();
    let left = tokio::spawn(async move {
        for index in 0..20 {
            a.create(
                &project_a,
                &format!("A {index}"),
                Some(&format!("proxy-a{index:07}")),
                false,
            )
            .await
            .unwrap();
        }
    });
    let right = tokio::spawn(async move {
        for index in 0..20 {
            b.create(
                &project_b,
                &format!("B {index}"),
                Some(&format!("proxy-b{index:07}")),
                false,
            )
            .await
            .unwrap();
        }
    });
    left.await.unwrap();
    right.await.unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let disk: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(disk["sessions"].as_object().unwrap().len(), 40);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.with_extension("lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[tokio::test]
async fn duplicate_ids_are_idempotent_for_same_project_and_conflict_across_projects() {
    let root = tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let store = SessionStore::new(root.path().join("sessions.json"), 10);
    let original = store
        .create(
            first.to_str().unwrap(),
            "One",
            Some("proxy-duplicate"),
            false,
        )
        .await
        .unwrap();
    let again = store
        .create(
            first.to_str().unwrap(),
            "Ignored",
            Some("proxy-duplicate"),
            false,
        )
        .await
        .unwrap();
    assert_eq!(again, original);
    assert!(
        store
            .create(
                second.to_str().unwrap(),
                "Two",
                Some("proxy-duplicate"),
                false
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_appends_do_not_lose_history_updates() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let path = root.path().join("sessions.json");
    let first = SessionStore::new(&path, 100);
    let second = SessionStore::new(&path, 100);
    let session = first
        .create(
            project.to_str().unwrap(),
            "Concurrent append",
            Some("proxy-concurrent-append"),
            false,
        )
        .await
        .unwrap();

    let id_a = session.id.clone();
    let id_b = session.id.clone();
    let left = tokio::spawn(async move {
        for index in 0..20 {
            first
                .append_history(
                    &id_a,
                    vec![json!({"role":"user","content":format!("A {index}")})],
                )
                .await
                .unwrap();
        }
    });
    let right = tokio::spawn(async move {
        for index in 0..20 {
            second
                .append_history(
                    &id_b,
                    vec![json!({"role":"assistant","content":format!("B {index}")})],
                )
                .await
                .unwrap();
        }
    });
    left.await.unwrap();
    right.await.unwrap();

    let history = SessionStore::new(&path, 100)
        .get(&session.id)
        .await
        .unwrap()
        .unwrap()
        .history;
    assert_eq!(history.len(), 40);
    for index in 0..20 {
        assert!(
            history
                .iter()
                .any(|message| message["content"] == format!("A {index}"))
        );
        assert!(
            history
                .iter()
                .any(|message| message["content"] == format!("B {index}"))
        );
    }
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
