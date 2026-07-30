use super::{
    app::{
        App, ConfirmAction, ConnectionState, GenerationState, MessageStatus, Modal,
        PersistenceState,
    },
    commands::COMMANDS,
    terminal::sanitize,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::time::Duration;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < 32 || area.height < 10 {
        return render_small(frame, area);
    }
    let [header, body, composer, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(composer_height(app, area.width)),
            Constraint::Length(2),
        ])
        .areas(area);
    render_header(frame, app, header);
    let wide = area.width >= 100 && app.sidebar;
    let [chat, side] = if wide {
        let parts = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
            .split(body);
        [parts[0], parts[1]]
    } else {
        [body, Rect::default()]
    };
    render_chat(frame, app, chat);
    if wide {
        render_sidebar(frame, app, side);
    }
    render_composer(frame, app, composer);
    render_footer(frame, app, footer);
    if let Some(modal) = &app.modal {
        render_modal(frame, app, modal, centered(area, 75, 75));
    }
}
fn composer_height(app: &App, width: u16) -> u16 {
    let lines = app.composer.text().lines().count().max(1) as u16;
    lines.min(5).saturating_add(2).min(width.max(1))
}
fn colors(app: &App) -> (Color, Color, Color) {
    if app.no_color {
        (Color::White, Color::Gray, Color::White)
    } else {
        (Color::Cyan, Color::DarkGray, Color::Yellow)
    }
}
fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let (c, _, warn) = colors(app);
    let connection = match app.connection {
        ConnectionState::Checking => "CHECKING",
        ConnectionState::Starting => "STARTING",
        ConnectionState::Online => "ONLINE",
        ConnectionState::Degraded => "DEGRADED",
        ConnectionState::Offline => "OFFLINE",
    };
    let generation = match &app.generation {
        GenerationState::Idle => "IDLE",
        GenerationState::Connecting { .. } => "CONNECTING",
        GenerationState::Streaming { .. } => "STREAMING",
        GenerationState::Cancelling { .. } => "CANCELLING",
        GenerationState::Failed(_) => "FAILED",
    };
    let text = Line::from(vec![
        Span::styled(
            " Freemodel WorkBuddy ",
            Style::default().fg(c).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "[{connection}]  Project: {}  Session: {}  Model: {}  [{generation}]",
            short(&app.project, 26),
            short(&app.session.title, 22),
            app.model
        )),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default().borders(Borders::ALL).border_style(
                    Style::default().fg(if connection == "ONLINE" { c } else { warn }),
                ),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}
fn render_chat(frame: &mut Frame, app: &App, area: Rect) {
    let (c, dim, warn) = colors(app);
    let query = app.search.as_deref().unwrap_or("").to_lowercase();
    let items = app.messages.iter().map(|m| {
        let label = match m.role.as_str() {
            "user" => "You",
            "assistant" => "Assistant",
            other => other,
        };
        let marker = match m.status {
            MessageStatus::Complete => "",
            MessageStatus::Streaming => "  [streaming]",
            MessageStatus::Cancelled => "  [cancelled]",
            MessageStatus::Failed => "  [failed]",
        };
        let role_color = if m.role == "user" {
            c
        } else if m.status == MessageStatus::Failed {
            warn
        } else {
            Color::White
        };
        let content = sanitize(&m.content);
        let highlighted = !query.is_empty() && content.to_lowercase().contains(&query);
        ListItem::new(Text::from(vec![
            Line::from(vec![Span::styled(
                format!("{label}{marker}"),
                Style::default().fg(role_color).add_modifier(Modifier::BOLD),
            )]),
            Line::from(Span::styled(
                if content.is_empty() && m.status == MessageStatus::Streaming {
                    "▌".into()
                } else {
                    content
                },
                Style::default().fg(if highlighted { warn } else { Color::Reset }),
            )),
            Line::raw(""),
        ]))
    });
    let title = format!(
        " Conversation · {} messages{} ",
        app.messages.len(),
        app.search
            .as_ref()
            .map(|q| format!(" · search: {q}"))
            .unwrap_or_default()
    );
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(dim)),
        )
        .scroll_padding(1);
    let mut state = ratatui::widgets::ListState::default();
    if !app.messages.is_empty() {
        let selected = if app.auto_scroll {
            app.messages.len().saturating_sub(1)
        } else {
            app.messages
                .len()
                .saturating_sub(1 + usize::from(app.scroll))
        };
        state.select(Some(selected));
    }
    frame.render_stateful_widget(list, area, &mut state);
}
fn render_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let (_, dim, _) = colors(app);
    let first = app
        .metrics
        .first_delta
        .map(format_duration)
        .unwrap_or_else(|| "—".into());
    let total = app
        .metrics
        .total
        .map(format_duration)
        .unwrap_or_else(|| "—".into());
    let save = match &app.persistence {
        PersistenceState::Clean => "saved",
        PersistenceState::Saving => "saving",
        PersistenceState::Unsaved(_) => "unsaved",
    };
    let side = Text::from(vec![
        Line::from("Diagnostics"),
        Line::raw(format!("TTFB: {first}")),
        Line::raw(format!("Total: {total}")),
        Line::raw(format!("Deltas: {}", app.metrics.deltas)),
        Line::raw(format!("Bytes: {}", app.metrics.bytes)),
        Line::raw(format!("History: {save}")),
        Line::raw(""),
        Line::from("Shortcuts"),
        Line::raw("F1 help"),
        Line::raw("Ctrl+O sessions"),
        Line::raw("Ctrl+P project"),
        Line::raw("Ctrl+M model"),
        Line::raw("Ctrl+R retry"),
        Line::raw("Esc cancel"),
    ]);
    frame.render_widget(
        Paragraph::new(side)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(dim)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}
fn render_composer(frame: &mut Frame, app: &App, area: Rect) {
    let (c, _, warn) = colors(app);
    let busy = app.busy();
    let title = if busy {
        " Composer · response active (Esc cancels) "
    } else {
        " Composer · Enter send · Alt/Shift+Enter newline "
    };
    frame.render_widget(
        Paragraph::new(sanitize(&app.composer.text()))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if busy { warn } else { c })),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
    if !busy && app.modal.is_none() {
        let inner = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        let (row, col) = app
            .composer
            .cursor_screen_position(inner.width.max(1) as usize);
        frame.set_cursor_position((
            inner.x + (col as u16).min(inner.width.saturating_sub(1)),
            inner.y + (row as u16).min(inner.height.saturating_sub(1)),
        ));
    }
}
fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let notice = app
        .notifications
        .back()
        .cloned()
        .unwrap_or_else(|| "Ctrl+K commands · F1 help · Ctrl+Q quit".into());
    frame.render_widget(
        Paragraph::new(sanitize(&notice))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}
fn render_modal(frame: &mut Frame, app: &App, modal: &Modal, area: Rect) {
    frame.render_widget(Clear, area);
    let (c, _, warn) = colors(app);
    let (title, text, border) = match modal {
        Modal::Help => (
            "Help & commands",
            COMMANDS
                .iter()
                .map(|v| {
                    format!(
                        "{:<24} {}{}",
                        v.usage,
                        v.description,
                        v.shortcut.map(|s| format!("  [{s}]")).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            c,
        ),
        Modal::Diagnostics => (
            "Diagnostics",
            format!(
                "{}\n\nProject: {}\nSession: {}\nModel: {}\nRequest deltas: {}\nRequest bytes: {}",
                app.diagnostics,
                app.project,
                app.session.id,
                app.model,
                app.metrics.deltas,
                app.metrics.bytes
            ),
            c,
        ),
        Modal::Logs => ("Proxy logs", app.diagnostics.clone(), c),
        Modal::Confirm(action) => (
            "Confirmation",
            match action {
                ConfirmAction::DeleteSession => format!(
                    "Delete proxy session '{}' and stop only its proxy-owned sidecar?",
                    app.session.title
                ),
                ConfirmAction::ClearHistory => {
                    format!("Clear all saved TUI history for '{}' ?", app.session.title)
                }
                ConfirmAction::Quit => {
                    "A request or unsaved history exists. Cancel and quit?".into()
                }
            },
            warn,
        ),
        Modal::Error(v) => ("Error", v.clone(), warn),
        Modal::Search => (
            "Search",
            format!("Search query: {}", sanitize(&app.modal_input.text())),
            c,
        ),
        Modal::Sessions => (
            "Sessions",
            app.sessions
                .iter()
                .enumerate()
                .map(|(index, session)| {
                    format!(
                        "{} {} · {} messages · {}",
                        if index == app.modal_selected {
                            ">"
                        } else {
                            " "
                        },
                        session.title,
                        session.history.len(),
                        session.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            c,
        ),
        Modal::Models => (
            "Models",
            app.models
                .iter()
                .enumerate()
                .map(|(index, model)| {
                    format!(
                        "{} {} · {}",
                        if index == app.modal_selected {
                            ">"
                        } else {
                            " "
                        },
                        model.id,
                        model.owned_by
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            c,
        ),
        Modal::Projects => (
            "Projects",
            format!(
                "{}{}> Custom path: {}",
                app.projects
                    .iter()
                    .enumerate()
                    .map(|(index, project)| format!(
                        "{} {}\n",
                        if index == app.modal_selected {
                            ">"
                        } else {
                            " "
                        },
                        sanitize(project)
                    ))
                    .collect::<String>(),
                if app.modal_selected == app.projects.len() {
                    ""
                } else {
                    " "
                },
                sanitize(&app.modal_input.text())
            ),
            c,
        ),
        Modal::Settings => (
            "Settings",
            format!(
                "{} Sidebar: {}\n{} Colors: {}",
                if app.modal_selected == 0 { ">" } else { " " },
                if app.sidebar { "shown" } else { "hidden" },
                if app.modal_selected == 1 { ">" } else { " " },
                if app.no_color { "disabled" } else { "enabled" }
            ),
            c,
        ),
        Modal::Key => (
            "API key",
            format!(
                "Enter the new key (masked): {}",
                "•".repeat(app.modal_input.text().chars().count().min(64))
            ),
            warn,
        ),
    };
    let suffix = if matches!(modal, Modal::Confirm(_)) {
        "\n\nEnter confirms · Esc cancels"
    } else if matches!(
        modal,
        Modal::Sessions
            | Modal::Models
            | Modal::Projects
            | Modal::Settings
            | Modal::Search
            | Modal::Key
    ) {
        "\n\n↑/↓ select · Enter apply · Esc closes"
    } else {
        "\n\nEsc closes"
    };
    frame.render_widget(
        Paragraph::new(format!("{text}{suffix}"))
            .block(
                Block::default()
                    .title(format!(" {title} "))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}
fn render_small(frame: &mut Frame, area: Rect) {
    frame.render_widget(Paragraph::new("Terminal is too small for the chat workspace.\nResize to at least 32×10.\nCtrl+Q exits safely.").alignment(Alignment::Center).block(Block::default().borders(Borders::ALL)).wrap(Wrap{trim:true}),area);
}
fn centered(area: Rect, x: u16, y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - y) / 2),
            Constraint::Percentage(y),
            Constraint::Percentage((100 - y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - x) / 2),
            Constraint::Percentage(x),
            Constraint::Percentage((100 - x) / 2),
        ])
        .split(vertical[1])[1]
}
fn short(v: &str, max: usize) -> String {
    let clean = sanitize(v);
    if clean.chars().count() <= max {
        clean
    } else {
        format!(
            "…{}",
            clean
                .chars()
                .rev()
                .take(max - 1)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        )
    }
}
fn format_duration(v: Duration) -> String {
    if v.as_secs() > 0 {
        format!("{:.2}s", v.as_secs_f64())
    } else {
        format!("{}ms", v.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SessionRecord;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    fn app() -> App {
        let s:SessionRecord=serde_json::from_value(json!({"id":"proxy-12345678","title":"Test","project":"/tmp","automatic":false,"created_at":"x","updated_at":"x","history":[],"sidecar":{}})).unwrap();
        App::new("/tmp".into(), s, "gpt-5.6-sol".into(), true, false)
    }
    #[test]
    fn renders_wide_narrow_and_small_without_panic() {
        for (w, h) in [(120, 40), (60, 20), (20, 5), (0, 0)] {
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| render(f, &app())).unwrap();
        }
    }
    #[test]
    fn renders_failed_assistant_reason_in_transcript() {
        let mut app = app();
        app.messages.push(super::super::app::ChatMessage {
            role: "assistant".into(),
            content: "Request failed: Could not connect to the local proxy".into(),
            status: MessageStatus::Failed,
        });
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Assistant  [failed]"), "{rendered}");
        assert!(
            rendered.contains("Request failed: Could not connect to the local proxy"),
            "{rendered}"
        );
    }
}
