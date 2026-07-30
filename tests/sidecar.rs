use freemodel_workbuddy_proxy::sidecar::process_matches;

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
