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
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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
    let (_, dim, _) = colors(app);
    let content_width = usize::from(area.width.saturating_sub(2).max(1));
    let lines = transcript_lines(app, content_width);
    let viewport_height = area.height.saturating_sub(2);
    let max_scroll = lines
        .len()
        .saturating_sub(usize::from(viewport_height))
        .min(usize::from(u16::MAX)) as u16;
    let scroll_from_top = if app.auto_scroll {
        max_scroll
    } else {
        max_scroll.saturating_sub(app.scroll.min(max_scroll))
    };
    let title = format!(
        " Conversation · {} messages{} ",
        app.messages.len(),
        app.search
            .as_ref()
            .map(|q| format!(" · search: {q}"))
            .unwrap_or_default()
    );
    let transcript = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(dim)),
        )
        .scroll((scroll_from_top, 0));
    frame.render_widget(transcript, area);
}

fn transcript_lines<'a>(app: &'a App, width: usize) -> Vec<Line<'a>> {
    let (c, _, warn) = colors(app);
    let query = app.search.as_deref().unwrap_or("").to_lowercase();
    let mut lines = Vec::new();
    for message in &app.messages {
        let label = match message.role.as_str() {
            "user" => "You",
            "assistant" => "Assistant",
            other => other,
        };
        let marker = match message.status {
            MessageStatus::Complete => "",
            MessageStatus::Streaming => "  [streaming]",
            MessageStatus::Cancelled => "  [cancelled]",
            MessageStatus::Failed => "  [failed]",
        };
        let role_color = if message.role == "user" {
            c
        } else if message.status == MessageStatus::Failed {
            warn
        } else {
            Color::White
        };
        let content = sanitize(&message.content);
        let highlighted = !query.is_empty() && content.to_lowercase().contains(&query);
        let content = if content.is_empty() && message.status == MessageStatus::Streaming {
            "▌".into()
        } else {
            content
        };
        let content_style = Style::default().fg(if highlighted { warn } else { Color::Reset });
        lines.push(Line::from(Span::styled(
            format!("{label}{marker}"),
            Style::default().fg(role_color).add_modifier(Modifier::BOLD),
        )));
        lines.extend(
            wrap_text(&content, width)
                .into_iter()
                .map(|line| Line::from(Span::styled(line, content_style))),
        );
        lines.push(Line::raw(""));
    }
    lines
}

fn wrap_text(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut result = Vec::new();
    for logical in value.split('\n') {
        if logical.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut line_width: usize = 0;
        for grapheme in UnicodeSegmentation::graphemes(logical, true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
            if line_width > 0 && line_width.saturating_add(grapheme_width) > width {
                result.push(std::mem::take(&mut line));
                line_width = 0;
            }
            line.push_str(grapheme);
            line_width = line_width.saturating_add(grapheme_width);
            if line_width >= width {
                result.push(std::mem::take(&mut line));
                line_width = 0;
            }
        }
        if !line.is_empty() {
            result.push(line);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
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
        Line::from("Proxy"),
        Line::raw(app.base_url.clone()),
        Line::raw(""),
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
        App::new(
            "/tmp".into(),
            s,
            "gpt-5.6-sol".into(),
            "http://127.0.0.1:40589/v1".into(),
            true,
            false,
        )
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
    fn wraps_long_unicode_transcript_content_into_visible_rows() {
        assert_eq!(wrap_text("ab界🙂cd", 4), vec!["ab界", "🙂cd"]);
        assert_eq!(
            wrap_text("first\n\nsecond", 20),
            vec!["first", "", "second"]
        );

        let mut app = app();
        app.messages.push(super::super::app::ChatMessage {
            role: "assistant".into(),
            content: "FIRST_MARKER ".repeat(12)
                + "\nEXPLICIT_NEWLINE\n"
                + &"界🙂".repeat(50)
                + " VISIBLE_TAIL",
            status: MessageStatus::Complete,
        });
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Assistant"), "{rendered}");
        assert!(rendered.contains("EXPLICIT_NEWLINE"), "{rendered}");
        assert!(rendered.contains("VISIBLE_TAIL"), "{rendered}");
        let marker_rows = rendered
            .lines()
            .filter(|line| line.contains("FIRST_MARKER"))
            .count();
        assert!(marker_rows >= 2, "content was not wrapped:\n{rendered}");
    }

    #[test]
    fn follows_tail_of_message_taller_than_transcript_viewport() {
        let mut app = app();
        app.messages.push(super::super::app::ChatMessage {
            role: "assistant".into(),
            content: (0..80)
                .map(|index| format!("ROW_{index:02}"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\nOVERSIZED_VISIBLE_TAIL",
            status: MessageStatus::Streaming,
        });
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("OVERSIZED_VISIBLE_TAIL"), "{rendered}");
        assert!(!rendered.contains("ROW_00"), "{rendered}");

        app.auto_scroll = false;
        app.scroll = u16::MAX;
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Assistant  [streaming]"), "{rendered}");
        assert!(rendered.contains("ROW_00"), "{rendered}");
        assert!(!rendered.contains("OVERSIZED_VISIBLE_TAIL"), "{rendered}");
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
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Assistant  [failed]"), "{rendered}");
        assert!(
            rendered.contains("Request failed: Could not connect to the local proxy"),
            "{rendered}"
        );
    }

    fn rendered_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
