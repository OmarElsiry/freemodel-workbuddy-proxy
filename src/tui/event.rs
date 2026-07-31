use super::{
    app::{Action, Modal},
    client::StreamEvent,
};
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum UiEvent {
    Input(CrosstermEvent),
    Stream(StreamEvent),
    Tick,
}
pub fn channel(capacity: usize) -> (mpsc::Sender<UiEvent>, mpsc::Receiver<UiEvent>) {
    mpsc::channel(capacity)
}
pub fn spawn_terminal(tx: mpsc::Sender<UiEvent>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        loop {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(value) => {
                        if tx.blocking_send(UiEvent::Input(value)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {
                    if tx.blocking_send(UiEvent::Tick).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}
pub fn modal_key_action(key: KeyEvent, confirm: bool) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    Some(match key.code {
        KeyCode::Esc => Action::CloseModal,
        KeyCode::Enter if confirm => Action::Confirmed,
        KeyCode::Enter => Action::ModalSubmit,
        KeyCode::Up => Action::ModalUp,
        KeyCode::Down => Action::ModalDown,
        KeyCode::Backspace => Action::ModalBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => Action::ModalInput(c),
        _ => return None,
    })
}

pub fn key_action(key: KeyEvent, busy: bool, composer_width: usize) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    Some(match (key.code, ctrl, alt, shift) {
        (KeyCode::Enter, _, true, _) | (KeyCode::Enter, _, _, true) => Action::Newline,
        (KeyCode::Enter, _, _, _) => Action::Submit,
        (KeyCode::Char('c'), true, _, _) if busy => Action::Cancel,
        (KeyCode::Char('a'), true, _, _) => Action::SelectAll,
        (KeyCode::Char('c'), true, _, _) => Action::CopySelection,
        (KeyCode::Char('x'), true, _, _) => Action::CutSelection,
        (KeyCode::Char('v'), true, _, _) => Action::PasteClipboard,
        (KeyCode::Char('q'), true, _, _) => Action::QuitRequested,
        (KeyCode::Esc, _, _, _) if busy => Action::Cancel,
        (KeyCode::Esc, _, _, _) => Action::CloseModal,
        (KeyCode::F(1), _, _, _) => Action::Open(Modal::Help),
        (KeyCode::Char('k'), true, _, _) => Action::Open(Modal::Help),
        (KeyCode::Char('n'), true, _, _) => Action::Command(cmd("new")),
        (KeyCode::Char('o'), true, _, _) => Action::Command(cmd("sessions")),
        (KeyCode::Char('p'), true, _, _) => Action::Command(cmd("project")),
        (KeyCode::Char('r'), true, _, _) => Action::Retry,
        (KeyCode::Char('e'), true, _, _) => Action::EditLast,
        (KeyCode::Char('m'), true, _, _) => Action::Command(cmd("models")),
        (KeyCode::Char('f'), true, _, _) => Action::Open(Modal::Search),
        (KeyCode::Char('l'), true, _, _) => Action::Notify("Screen redrawn".into()),
        (KeyCode::PageUp, _, _, _) => Action::Scroll(10),
        (KeyCode::PageDown, _, _, _) => Action::Scroll(-10),
        (KeyCode::Up, false, _, select) => Action::Up {
            width: composer_width,
            select,
        },
        (KeyCode::Down, false, _, select) => Action::Down {
            width: composer_width,
            select,
        },
        (KeyCode::Left, false, _, select) => Action::Left { select },
        (KeyCode::Right, false, _, select) => Action::Right { select },
        (KeyCode::Home, false, _, select) => Action::Home { select },
        (KeyCode::End, false, _, select) => Action::End { select },
        (KeyCode::Backspace, _, _, _) => Action::Backspace,
        (KeyCode::Delete, _, _, _) => Action::Delete,
        (KeyCode::Char(c), false, _, _) => Action::Insert(c),
        _ => return None,
    })
}
fn cmd(name: &str) -> super::commands::ParsedCommand {
    super::commands::ParsedCommand {
        name: name.into(),
        args: vec![],
    }
}
pub fn mouse_action(mouse: MouseEvent) -> Option<Action> {
    use crossterm::event::MouseEventKind;
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(Action::Scroll(3)),
        MouseEventKind::ScrollDown => Some(Action::Scroll(-3)),
        _ => None,
    }
}
