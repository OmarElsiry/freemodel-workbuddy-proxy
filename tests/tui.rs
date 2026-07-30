use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use freemodel_workbuddy_proxy::{
    sse::SseDecoder,
    tui::{commands, composer::Composer, event, terminal::sanitize},
};
use predicates::prelude::PredicateBooleanExt;
use std::{
    net::TcpListener,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    process::Command,
};

#[test]
fn launcher_rejects_invalid_arguments_before_build_work() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_cmd::assert::Assert::new(
        Command::new("bash")
            .arg(root.join("start.sh"))
            .arg("--unsupported")
            .current_dir(root)
            .output()
            .unwrap(),
    )
    .failure()
    .code(2)
    .stderr(predicates::str::contains("Usage:").and(predicates::str::contains("[--server-only]")))
    .stdout(predicates::str::is_empty());
}

#[test]
fn launcher_always_delegates_freshness_to_cargo() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir().unwrap();
    let cargo = temp.path().join("cargo");
    let log = temp.path().join("cargo.log");
    std::fs::write(
        &cargo,
        "#!/bin/bash\nprintf '%s\\n' \"$*\" >> \"$CARGO_LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let output = Command::new("timeout")
        .args(["1s", "bash"])
        .arg(root.join("start.sh"))
        .arg("--server-only")
        .current_dir(root)
        .env("PATH", path)
        .env("CARGO_LOG", &log)
        .env("PROXY_PORT", port.to_string())
        .output()
        .unwrap();
    assert!(
        matches!(output.status.code(), Some(124 | 143)) || output.status.signal() == Some(15),
        "unexpected launcher timeout status: {:?}",
        output.status
    );
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        format!(
            "build --release --manifest-path {}/Cargo.toml\n",
            root.display()
        )
    );
}

#[test]
fn tui_parser_preserves_fragmented_sse_lines() {
    let mut decoder = SseDecoder::default();
    assert!(
        decoder
            .push(b"data: {\"choices\":[{\"delta\":{")
            .unwrap()
            .is_empty()
    );
    let lines = decoder
        .push(b"\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n")
        .unwrap();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("hello"));
    assert_eq!(lines[2], "data: [DONE]");
}

#[test]
fn command_registry_covers_documented_power_features() {
    for name in [
        "sessions",
        "rename",
        "delete",
        "clear",
        "retry",
        "edit",
        "cancel",
        "diagnostics",
        "settings",
        "quit",
    ] {
        assert!(
            commands::COMMANDS
                .iter()
                .any(|command| command.name == name)
        );
    }
    assert!(commands::parse("/rename \"UX review\"").is_ok());
    assert!(commands::parse("/unknown").is_err());
}

#[test]
fn composer_handles_multiline_unicode_edits() {
    let mut composer = Composer::new("hello");
    composer.newline();
    composer.insert_str("界🙂");
    assert_eq!(composer.text(), "hello\n界🙂");
    composer.left();
    composer.backspace();
    assert_eq!(composer.text(), "hello\n🙂");
}

#[test]
fn shortcuts_distinguish_send_newline_cancel_and_quit() {
    assert!(matches!(
        event::key_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), false),
        Some(freemodel_workbuddy_proxy::tui::app::Action::Submit)
    ));
    assert!(matches!(
        event::key_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), false),
        Some(freemodel_workbuddy_proxy::tui::app::Action::Newline)
    ));
    assert!(matches!(
        event::key_action(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            true
        ),
        Some(freemodel_workbuddy_proxy::tui::app::Action::Cancel)
    ));
    assert!(matches!(
        event::key_action(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            false
        ),
        Some(freemodel_workbuddy_proxy::tui::app::Action::QuitRequested)
    ));
}

#[test]
fn remote_terminal_controls_are_sanitized() {
    assert!(!sanitize("safe\u{1b}[2J text").contains('\u{1b}'));
}

#[test]
fn all_documented_shortcuts_have_semantic_actions() {
    use freemodel_workbuddy_proxy::tui::app::{Action, Modal};
    let cases = [
        (
            KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
            Action::Open(Modal::Help),
        ),
        (
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            Action::Open(Modal::Help),
        ),
        (
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Action::Open(Modal::Help),
        ),
        (
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            Action::Open(Modal::Search),
        ),
        (
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Action::Retry,
        ),
        (
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
            Action::EditLast,
        ),
        (
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            Action::Scroll(10),
        ),
        (
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            Action::Scroll(-10),
        ),
        (
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            Action::Home,
        ),
        (KeyEvent::new(KeyCode::End, KeyModifiers::NONE), Action::End),
        (
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            Action::Backspace,
        ),
        (
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            Action::Delete,
        ),
    ];
    for (key, expected) in cases {
        assert_eq!(event::key_action(key, false), Some(expected));
    }
    for (key, command) in [
        ('n', "new"),
        ('o', "sessions"),
        ('p', "project"),
        ('m', "models"),
    ] {
        let action = event::key_action(
            KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL),
            false,
        );
        assert!(matches!(action, Some(Action::Command(ref value)) if value.name == command));
    }
}

#[test]
fn modal_and_mouse_controls_are_mapped_without_side_effects() {
    use freemodel_workbuddy_proxy::tui::app::Action;
    assert_eq!(
        event::modal_key_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), true),
        Some(Action::Confirmed)
    );
    assert_eq!(
        event::modal_key_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), false),
        Some(Action::ModalSubmit)
    );
    assert_eq!(
        event::modal_key_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), false),
        Some(Action::CloseModal)
    );
    let mouse = |kind| MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(
        event::mouse_action(mouse(MouseEventKind::ScrollUp)),
        Some(Action::Scroll(3))
    );
    assert_eq!(
        event::mouse_action(mouse(MouseEventKind::ScrollDown)),
        Some(Action::Scroll(-3))
    );
    assert!(event::mouse_action(mouse(MouseEventKind::Down(MouseButton::Left))).is_none());
}

#[test]
fn command_parser_handles_all_commands_case_quotes_escapes_and_failures() {
    for command in commands::COMMANDS {
        let parsed = commands::parse(&format!("/{}", command.name))
            .unwrap()
            .unwrap();
        assert_eq!(parsed.name, command.name);
    }
    assert_eq!(
        commands::parse("/RENAME 'quoted title'")
            .unwrap()
            .unwrap()
            .args,
        vec!["quoted title"]
    );
    assert_eq!(
        commands::parse(r#"/rename escaped\ title"#)
            .unwrap()
            .unwrap()
            .args,
        vec!["escaped title"]
    );
    assert!(commands::parse("/").is_err());
    assert!(commands::parse(r#"/rename trailing\"#).is_err());
    assert!(commands::parse("/rename \"unclosed").is_err());
    assert_eq!(commands::parse("not a command").unwrap(), None);
    assert!(
        commands::completions("/mo")
            .iter()
            .any(|value| value.name == "model")
    );
    assert!(
        commands::completions("/mo")
            .iter()
            .any(|value| value.name == "models")
    );
}

#[test]
fn key_release_events_are_ignored() {
    let release = KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Release,
        state: KeyEventState::NONE,
    };
    assert!(event::key_action(release, false).is_none());
    assert!(event::modal_key_action(release, false).is_none());
}
