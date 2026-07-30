use crossterm::{
    cursor::Show,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, Stdout},
    panic,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: Arc<AtomicBool>,
}
impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        if let Err(e) = execute!(out, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(e);
        }
        let terminal = Terminal::new(CrosstermBackend::new(out))?;
        let active = Arc::new(AtomicBool::new(true));
        install_panic_restore(active.clone());
        Ok(Self { terminal, active })
    }
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
    pub fn restore(&mut self) {
        if self.active.swap(false, Ordering::SeqCst) {
            let _ = disable_raw_mode();
            let _ = execute!(
                self.terminal.backend_mut(),
                DisableMouseCapture,
                LeaveAlternateScreen,
                Show
            );
            let _ = self.terminal.show_cursor();
        }
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
fn install_panic_restore(active: Arc<AtomicBool>) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if active.swap(false, Ordering::SeqCst) {
            let _ = disable_raw_mode();
            let _ = execute!(
                io::stdout(),
                DisableMouseCapture,
                LeaveAlternateScreen,
                Show
            );
        }
        previous(info);
    }));
}

pub fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == '\n' || c == '\t' || (!c.is_control() && c != '\u{1b}') {
                c
            } else {
                '�'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strips_terminal_controls_but_preserves_text_layout() {
        assert_eq!(sanitize("ok\u{1b}[31m\nnext\u{7}"), "ok�[31m\nnext�");
    }
}
