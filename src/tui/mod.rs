pub mod app;
pub mod client;
pub mod commands;
pub mod composer;
pub mod event;
pub mod preferences;
pub mod setup;
pub mod terminal;
pub mod view;

use anyhow::Result;
use app::{Action, App, Effect, Modal};
use client::{ProxyClient, StreamEvent, StreamRequest};
use crossterm::event::Event as CrosstermEvent;
use event::{UiEvent, channel};
use preferences::{Preferences, path as preferences_path};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

pub async fn run(config: &crate::config::Config) -> Result<()> {
    println!("Freemodel WorkBuddy Proxy · Rust TUI");
    let proxy_url = setup::proxy_url(config);
    let base_url = format!("{proxy_url}/v1");
    println!("Preflight: {} transport · {proxy_url}", config.transport);
    println!("OpenAI-compatible base URL: {base_url}");
    let server = setup::ensure_server(config).await?;
    println!(
        "Proxy: online{}",
        if server.started_here {
            " (started by this TUI)"
        } else {
            ""
        }
    );
    println!(
        "Official CLI: {}",
        if config.workbuddy_cli_path.is_file() {
            "available"
        } else {
            "not found"
        }
    );
    println!("API key: {}", mask_key(&config.api_key));
    let pref_path = preferences_path(&config.runtime_dir);
    let mut prefs = Preferences::load(&pref_path);
    let client = ProxyClient::new(&proxy_url, config.api_key.clone())?;
    let project = setup::choose_project(config, &prefs)?;
    let session = guided_session(
        &client,
        &project,
        prefs.last_sessions.get(&project).map(String::as_str),
    )
    .await?;
    prefs.remember(&project, Some(&session.id));
    prefs.save(&pref_path)?;
    let mut app = App::new(
        project,
        session,
        prefs.model.clone(),
        base_url,
        prefs.sidebar,
        prefs.no_color,
    );
    app.projects = prefs.recent_projects.clone();
    let mut terminal = terminal::TerminalGuard::enter()?;
    let (tx, mut rx) = channel(256);
    let terminal_task = event::spawn_terminal(tx.clone());
    let mut cancels: HashMap<u64, CancellationToken> = HashMap::new();
    while app.running {
        terminal
            .terminal_mut()
            .draw(|frame| view::render(frame, &app))?;
        let Some(event) = rx.recv().await else { break };
        let actions = match event {
            UiEvent::Input(CrosstermEvent::Key(key)) => {
                if let Some(modal) = &app.modal {
                    event::modal_key_action(key, matches!(modal, Modal::Confirm(_)))
                        .into_iter()
                        .collect()
                } else {
                    let width = terminal
                        .terminal_mut()
                        .size()
                        .map(|area| view::composer_width(area.width))
                        .unwrap_or(1);
                    event::key_action(key, app.busy(), width)
                        .into_iter()
                        .collect()
                }
            }
            UiEvent::Input(CrosstermEvent::Paste(text)) => {
                let text = terminal::sanitize_paste(&text);
                if app.modal.is_some() {
                    text.chars().map(Action::ModalInput).collect()
                } else {
                    vec![Action::InsertText(text)]
                }
            }
            UiEvent::Input(CrosstermEvent::Mouse(mouse)) => {
                event::mouse_action(mouse).into_iter().collect()
            }
            UiEvent::Input(CrosstermEvent::Resize(_, _)) | UiEvent::Tick => vec![],
            UiEvent::Stream(stream) => {
                if let Some(request_id) = finished_stream_request(&stream) {
                    cancels.remove(&request_id);
                }
                vec![stream_action(stream)]
            }
            _ => vec![],
        };
        for action in actions {
            let effects = app.update(action);
            for effect in effects {
                execute_effect(
                    effect,
                    &mut app,
                    &client,
                    &tx,
                    &mut cancels,
                    &mut prefs,
                    &pref_path,
                    config,
                )
                .await?;
            }
        }
    }
    terminal.restore();
    drop(rx);
    drop(tx);
    let _ = terminal_task.await;
    server.detach();
    Ok(())
}

async fn guided_session(
    client: &ProxyClient,
    project: &str,
    preferred: Option<&str>,
) -> Result<crate::models::SessionRecord> {
    loop {
        let sessions = client
            .list_sessions(project)
            .await
            .map_err(anyhow::Error::msg)?;
        println!("\nProxy sessions for this project:");
        println!("  0. Create new session");
        for (i, s) in sessions.iter().enumerate() {
            let recent = if preferred == Some(&s.id) {
                " · last used"
            } else {
                ""
            };
            println!(
                "  {}. {} · {} messages{}",
                i + 1,
                s.title,
                s.history.len(),
                recent
            );
        }
        let value = setup::prompt(
            "Select session",
            preferred
                .and_then(|id| {
                    sessions
                        .iter()
                        .position(|s| s.id == id)
                        .map(|i| (i + 1).to_string())
                })
                .as_deref()
                .or(Some("0")),
        )?;
        let Ok(choice) = value.parse::<usize>() else {
            println!("Enter a number from 0 to {}.", sessions.len());
            continue;
        };
        if choice == 0 {
            let title = setup::prompt(
                "New session title",
                std::path::Path::new(project)
                    .file_name()
                    .and_then(|v| v.to_str()),
            )?;
            return client
                .create_session(project, &title)
                .await
                .map_err(anyhow::Error::msg);
        }
        if let Some(session) = sessions.get(choice - 1) {
            return Ok(session.clone());
        }
        println!("Enter a number from 0 to {}.", sessions.len());
    }
}

fn finished_stream_request(event: &StreamEvent) -> Option<u64> {
    match event {
        StreamEvent::Completed { request_id, .. }
        | StreamEvent::Failed { request_id, .. }
        | StreamEvent::Cancelled { request_id } => Some(*request_id),
        StreamEvent::Connected { .. } | StreamEvent::Delta { .. } => None,
    }
}

fn stream_action(event: StreamEvent) -> Action {
    match event {
        StreamEvent::Connected { request_id } => Action::StreamConnected { request_id },
        StreamEvent::Delta {
            request_id,
            text,
            elapsed,
            source_delta,
        } => Action::StreamDelta {
            request_id,
            text,
            elapsed,
            source_delta,
        },
        StreamEvent::Completed {
            request_id, total, ..
        } => Action::StreamCompleted { request_id, total },
        StreamEvent::Failed {
            request_id,
            message,
        } => Action::StreamFailed {
            request_id,
            message,
        },
        StreamEvent::Cancelled { request_id } => Action::StreamCancelled { request_id },
    }
}
#[allow(clippy::too_many_arguments)]
async fn execute_effect(
    effect: Effect,
    app: &mut App,
    client: &ProxyClient,
    tx: &tokio::sync::mpsc::Sender<UiEvent>,
    cancels: &mut HashMap<u64, CancellationToken>,
    prefs: &mut Preferences,
    pref_path: &std::path::Path,
    config: &crate::config::Config,
) -> Result<()> {
    match effect {
        Effect::Send {
            request_id,
            messages,
            model,
        } => {
            let cancel = CancellationToken::new();
            cancels.insert(request_id, cancel.clone());
            let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(64);
            client.stream_chat(
                StreamRequest {
                    request_id,
                    session_id: app.session.id.clone(),
                    project: app.project.clone(),
                    model,
                    messages,
                },
                cancel,
                stream_tx,
            );
            let ui = tx.clone();
            tokio::spawn(async move {
                while let Some(event) = stream_rx.recv().await {
                    if ui.send(UiEvent::Stream(event)).await.is_err() {
                        break;
                    }
                }
            });
        }
        Effect::Cancel { request_id } => {
            if let Some(token) = cancels.remove(&request_id) {
                token.cancel()
            }
        }
        Effect::SaveTranscript { messages } => {
            match client.replace_history(&app.session.id, &messages).await {
                Ok(record) => {
                    app.session = record;
                    app.update(Action::HistorySaved);
                }
                Err(e) => {
                    app.update(Action::HistorySaveFailed(e));
                }
            }
        }
        Effect::LoadSessions => match client.list_sessions(&app.project).await {
            Ok(v) => {
                app.update(Action::SessionsLoaded(v));
                app.modal = Some(Modal::Sessions)
            }
            Err(e) => app.modal = Some(Modal::Error(e)),
        },
        Effect::LoadModels => match client.models().await {
            Ok(v) => {
                app.update(Action::ModelsLoaded(v));
                app.modal = Some(Modal::Models)
            }
            Err(e) => app.modal = Some(Modal::Error(e)),
        },
        Effect::SelectModel(model) => {
            match client.models().await {
                Ok(models) if models.iter().any(|item| item.id == model) => {
                    app.models = models;
                    app.update(Action::ModelChanged(model));
                }
                Ok(_) => {
                    app.modal = Some(Modal::Error(format!("Unknown model: {model}")));
                    return Ok(());
                }
                Err(e) => {
                    app.modal = Some(Modal::Error(e));
                    return Ok(());
                }
            }
            prefs.model = app.model.clone();
            if let Err(e) = prefs.save(pref_path) {
                app.update(Action::Notify(format!("Preferences were not saved: {e}")));
            }
        }
        Effect::LoadDiagnostics => match client.diagnostics().await {
            Ok(value) => {
                app.update(Action::DiagnosticsLoaded(format!(
                    "Version: {}\nUptime: {}s\nBase URL: {}/v1\nBind: {}\nTransport: {}\nUpstream: {}\nSession store: {}\nRuntime: {}\nSidecars: {}/{}\nResponses API: {}\nClient function tools: {}\nSkills execution: {}\nVision input: {}\nLocal image paths: {}\nImage generation: {}\nRSS: {}",
                    value.version,
                    value.uptime_seconds,
                    value.bind_url,
                    value.bind_url,
                    value.transport,
                    value.upstream_host,
                    value.session_store,
                    value.runtime_dir,
                    value.active_sidecars,
                    value.max_sidecars,
                    value.capabilities.responses_api,
                    value.capabilities.client_function_tools,
                    value.capabilities.skills_execution,
                    value.capabilities.vision_input,
                    value.capabilities.local_image_paths,
                    value.capabilities.image_generation,
                    value
                        .rss_bytes
                        .map(|v| format!("{:.1} MiB", v as f64 / 1_048_576.0))
                        .unwrap_or_else(|| "unavailable".into())
                )));
            }
            Err(e) => app.modal = Some(Modal::Error(e)),
        },
        Effect::CreateSession(title) => match client
            .create_session(&app.project, title.as_deref().unwrap_or("Proxy session"))
            .await
        {
            Ok(v) => {
                app.update(Action::SessionChanged(v));
            }
            Err(e) => app.modal = Some(Modal::Error(e)),
        },
        Effect::SwitchSession(value) => {
            let selected = if let Ok(index) = value.parse::<usize>() {
                app.sessions.get(index.saturating_sub(1)).cloned()
            } else if let Some(session) = app.sessions.iter().find(|s| s.id == value).cloned() {
                Some(session)
            } else {
                client.get_session(&value).await.ok()
            };
            if let Some(v) = selected.filter(|session| session.project == app.project) {
                app.update(Action::SessionChanged(v));
            } else {
                app.modal = Some(Modal::Error(
                    "Session was not found in the current project.".into(),
                ));
            }
        }
        Effect::RenameSession(title) => {
            match client.rename_session(&app.session.id, &title).await {
                Ok(v) => {
                    app.session = v;
                    app.update(Action::Notify("Session renamed".into()));
                }
                Err(e) => app.modal = Some(Modal::Error(e)),
            }
        }
        Effect::DeleteSession => match client.delete_session(&app.session.id).await {
            Ok(()) => {
                let sessions = client
                    .list_sessions(&app.project)
                    .await
                    .map_err(anyhow::Error::msg)?;
                if let Some(v) = sessions.into_iter().next() {
                    app.update(Action::SessionChanged(v));
                } else {
                    let v = client
                        .create_session(&app.project, "Proxy session")
                        .await
                        .map_err(anyhow::Error::msg)?;
                    app.update(Action::SessionChanged(v));
                }
                app.update(Action::Notify("Session deleted".into()));
            }
            Err(e) => app.modal = Some(Modal::Error(e)),
        },
        Effect::ClearHistory => match client.clear_history(&app.session.id).await {
            Ok(v) => {
                app.update(Action::SessionChanged(v));
                app.update(Action::Notify("Saved history cleared".into()));
            }
            Err(e) => app.modal = Some(Modal::Error(e)),
        },
        Effect::ChangeProject(path) => {
            terminal_change_project(app, client, path, config).await?;
        }
        Effect::Copy(scope) => {
            let text = if scope == "all" {
                app.messages
                    .iter()
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            } else {
                app.messages
                    .iter()
                    .rev()
                    .find(|m| m.role == "assistant")
                    .map(|m| m.content.clone())
                    .unwrap_or_default()
            };
            match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
                Ok(()) => {
                    app.update(Action::Notify("Copied to clipboard".into()));
                }
                Err(_) => {
                    app.modal = Some(Modal::Error(
                        "Clipboard is unavailable in this terminal".into(),
                    ))
                }
            }
        }
        Effect::WriteClipboard { text, cut } => {
            match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
                Ok(()) => {
                    if cut {
                        app.update(Action::CutCommitted);
                        app.update(Action::Notify("Cut selection to clipboard".into()));
                    } else {
                        app.update(Action::Notify("Copied selection to clipboard".into()));
                    }
                }
                Err(_) => {
                    app.update(Action::Notify(
                        "Clipboard is unavailable in this terminal".into(),
                    ));
                }
            }
        }
        Effect::ReadClipboard => {
            match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
                Ok(text) => {
                    app.update(Action::InsertText(terminal::sanitize_paste(&text)));
                }
                Err(_) => {
                    app.update(Action::Notify(
                        "Clipboard is unavailable in this terminal".into(),
                    ));
                }
            }
        }
        Effect::LoadLogs => {
            let contents = read_log_tail(&server_log_path(config), 80);
            app.update(Action::LogsLoaded(format!(
                "Proxy log: {}\n\n{}",
                server_log_path(config).display(),
                contents
            )));
        }
        Effect::SaveKey(value) => match config.save_api_key(&value) {
            Ok(()) => {
                app.update(Action::Notify(
                    "API key saved. Restart the proxy to apply it.".into(),
                ));
            }
            Err(e) => app.modal = Some(Modal::Error(format!("API key was not saved: {e}"))),
        },
        Effect::SavePreferences => {
            prefs.model = app.model.clone();
            prefs.sidebar = app.sidebar;
            prefs.no_color = app.no_color;
            prefs.remember(&app.project, Some(&app.session.id));
            if let Err(e) = prefs.save(pref_path) {
                app.update(Action::Notify(format!("Preferences were not saved: {e}")));
            }
        }
        Effect::Quit => app.running = false,
    }
    Ok(())
}
fn server_log_path(config: &crate::config::Config) -> std::path::PathBuf {
    config.project_root.join("proxy_server.log")
}

fn read_log_tail(path: &std::path::Path, max_lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return "Log file does not exist yet.".into();
    };
    let mut lines = text.lines().rev().take(max_lines).collect::<Vec<_>>();
    lines.reverse();
    let clean = lines
        .join("\n")
        .chars()
        .map(|c| {
            if c == '\n' || c == '\t' || (!c.is_control() && c != '\u{1b}') {
                c
            } else {
                '�'
            }
        })
        .collect::<String>();
    if clean.is_empty() {
        "Log file is empty.".into()
    } else {
        clean
    }
}

async fn terminal_change_project(
    app: &mut App,
    client: &ProxyClient,
    path: Option<String>,
    config: &crate::config::Config,
) -> Result<()> {
    let value = path.unwrap_or_else(|| config.default_project.to_string_lossy().into());
    let project = setup::canonical(&value)?;
    let sessions = client
        .list_sessions(&project)
        .await
        .map_err(anyhow::Error::msg)?;
    let session = if let Some(v) = sessions.into_iter().next() {
        v
    } else {
        client
            .create_session(&project, "Proxy session")
            .await
            .map_err(anyhow::Error::msg)?
    };
    app.update(Action::SessionChanged(session));
    if !app.projects.iter().any(|value| value == &project) {
        app.projects.insert(0, project);
        app.projects.truncate(10);
    }
    Ok(())
}
fn mask_key(value: &str) -> String {
    if value.is_empty() {
        "not configured".into()
    } else if value.chars().count() > 8 {
        let chars: Vec<_> = value.chars().collect();
        format!(
            "{}••••{}",
            chars[..4].iter().collect::<String>(),
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    } else {
        "configured".into()
    }
}
