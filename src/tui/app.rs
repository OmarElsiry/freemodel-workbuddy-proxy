use super::{commands::ParsedCommand, composer::Composer};
use crate::models::{ModelInfo, SessionRecord};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Composer,
    Transcript,
    Sidebar,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Checking,
    Starting,
    Online,
    Degraded,
    Offline,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenerationState {
    Idle,
    Connecting { request_id: u64 },
    Streaming { request_id: u64 },
    Cancelling { request_id: u64 },
    Failed(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistenceState {
    Clean,
    Saving,
    Unsaved(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modal {
    Help,
    Diagnostics,
    Logs,
    Sessions,
    Models,
    Projects,
    Settings,
    Search,
    Key,
    Confirm(ConfirmAction),
    Error(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    DeleteSession,
    ClearHistory,
    Quit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageStatus {
    Complete,
    Streaming,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub status: MessageStatus,
}
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub started: Option<Instant>,
    pub first_delta: Option<Duration>,
    pub total: Option<Duration>,
    pub deltas: usize,
    pub bytes: usize,
}

#[derive(Clone, Debug)]
pub struct App {
    pub running: bool,
    pub focus: Focus,
    pub connection: ConnectionState,
    pub generation: GenerationState,
    pub persistence: PersistenceState,
    pub modal: Option<Modal>,
    pub composer: Composer,
    pub messages: Vec<ChatMessage>,
    pub project: String,
    pub session: SessionRecord,
    pub sessions: Vec<SessionRecord>,
    pub projects: Vec<String>,
    pub model: String,
    pub models: Vec<ModelInfo>,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub search: Option<String>,
    pub notifications: VecDeque<String>,
    pub metrics: Metrics,
    pub sidebar: bool,
    pub no_color: bool,
    pub modal_input: Composer,
    pub modal_selected: usize,
    pub diagnostics: String,
    next_request_id: u64,
    pub last_user: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Insert(char),
    Newline,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Submit,
    Scroll(i16),
    ToggleSidebar,
    Open(Modal),
    CloseModal,
    Notify(String),
    QuitRequested,
    Confirmed,
    StartRequest {
        request_id: u64,
        user: String,
    },
    StreamConnected {
        request_id: u64,
    },
    StreamDelta {
        request_id: u64,
        text: String,
        elapsed: Duration,
    },
    StreamCompleted {
        request_id: u64,
        total: Duration,
    },
    StreamFailed {
        request_id: u64,
        message: String,
    },
    StreamCancelled {
        request_id: u64,
    },
    Cancel,
    HistorySaved,
    HistorySaveFailed(String),
    ModalInput(char),
    ModalBackspace,
    ModalUp,
    ModalDown,
    ModalSubmit,
    DiagnosticsLoaded(String),
    LogsLoaded(String),
    SessionsLoaded(Vec<SessionRecord>),
    ModelsLoaded(Vec<ModelInfo>),
    SessionChanged(SessionRecord),
    ModelChanged(String),
    Command(ParsedCommand),
    Retry,
    EditLast,
    Search(String),
    ClearSearch,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Send {
        request_id: u64,
        messages: Vec<Value>,
        model: String,
    },
    Cancel {
        request_id: u64,
    },
    SaveTranscript {
        messages: Vec<Value>,
    },
    LoadSessions,
    LoadModels,
    SelectModel(String),
    LoadDiagnostics,
    CreateSession(Option<String>),
    SwitchSession(String),
    RenameSession(String),
    DeleteSession,
    ClearHistory,
    ChangeProject(Option<String>),
    Copy(String),
    LoadLogs,
    SaveKey(String),
    SavePreferences,
    Quit,
}

impl App {
    pub fn new(
        project: String,
        session: SessionRecord,
        model: String,
        sidebar: bool,
        no_color: bool,
    ) -> Self {
        let messages = session.history.iter().filter_map(from_value).collect();
        Self {
            running: true,
            focus: Focus::Composer,
            connection: ConnectionState::Online,
            generation: GenerationState::Idle,
            persistence: PersistenceState::Clean,
            modal: None,
            composer: Composer::default(),
            messages,
            project,
            session,
            sessions: vec![],
            projects: vec![],
            model,
            models: vec![],
            scroll: 0,
            auto_scroll: true,
            search: None,
            notifications: VecDeque::new(),
            metrics: Metrics::default(),
            sidebar,
            no_color,
            modal_input: Composer::default(),
            modal_selected: 0,
            diagnostics: String::new(),
            next_request_id: 1,
            last_user: None,
        }
    }
    pub fn busy(&self) -> bool {
        matches!(
            self.generation,
            GenerationState::Connecting { .. }
                | GenerationState::Streaming { .. }
                | GenerationState::Cancelling { .. }
        )
    }
    pub fn current_request_id(&self) -> Option<u64> {
        match self.generation {
            GenerationState::Connecting { request_id }
            | GenerationState::Streaming { request_id }
            | GenerationState::Cancelling { request_id } => Some(request_id),
            _ => None,
        }
    }
    pub fn history_values(&self) -> Vec<Value> {
        self.messages
            .iter()
            .filter(|m| m.status == MessageStatus::Complete)
            .map(|m| json!({"role":m.role,"content":m.content}))
            .collect()
    }
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Insert(c) if !self.busy() && self.modal.is_none() => self.composer.insert(c),
            Action::Newline if !self.busy() => self.composer.newline(),
            Action::Backspace if !self.busy() => self.composer.backspace(),
            Action::Delete if !self.busy() => self.composer.delete(),
            Action::Left => self.composer.left(),
            Action::Right => self.composer.right(),
            Action::Up => self.composer.up(),
            Action::Down => self.composer.down(),
            Action::Home => self.composer.home(),
            Action::End => self.composer.end(),
            Action::Submit => return self.submit(),
            Action::ModalInput(c) => self.modal_input.insert(c),
            Action::ModalBackspace => self.modal_input.backspace(),
            Action::ModalUp => self.modal_selected = self.modal_selected.saturating_sub(1),
            Action::ModalDown => {
                let len = match self.modal {
                    Some(Modal::Sessions) => self.sessions.len(),
                    Some(Modal::Models) => self.models.len(),
                    Some(Modal::Projects) => self.projects.len().saturating_add(1),
                    Some(Modal::Settings) => 2,
                    _ => 0,
                };
                if len > 0 {
                    self.modal_selected = (self.modal_selected + 1).min(len - 1);
                }
            }
            Action::ModalSubmit => return self.modal_submit(),
            Action::DiagnosticsLoaded(value) => {
                self.diagnostics = value;
                self.modal = Some(Modal::Diagnostics);
            }
            Action::LogsLoaded(value) => {
                self.diagnostics = value;
                self.modal = Some(Modal::Logs);
            }
            Action::Scroll(delta) => {
                self.auto_scroll = false;
                self.scroll = if delta < 0 {
                    self.scroll.saturating_sub(delta.unsigned_abs())
                } else {
                    self.scroll.saturating_add(delta as u16)
                }
            }
            Action::ToggleSidebar => {
                self.sidebar = !self.sidebar;
                return vec![Effect::SavePreferences];
            }
            Action::Open(m) => {
                self.modal_input.clear();
                self.modal_selected = 0;
                self.modal = Some(m);
            }
            Action::CloseModal => {
                self.modal = None;
                self.modal_input.clear();
            }
            Action::Notify(v) => self.notify(v),
            Action::QuitRequested => {
                if self.busy() || matches!(self.persistence, PersistenceState::Unsaved(_)) {
                    self.modal = Some(Modal::Confirm(ConfirmAction::Quit))
                } else {
                    return vec![Effect::Quit];
                }
            }
            Action::Confirmed => return self.confirm(),
            Action::StartRequest { request_id, user } => {
                self.last_user = Some(user.clone());
                self.messages.push(ChatMessage {
                    role: "user".into(),
                    content: user,
                    status: MessageStatus::Complete,
                });
                self.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: String::new(),
                    status: MessageStatus::Streaming,
                });
                self.generation = GenerationState::Connecting { request_id };
                self.metrics = Metrics {
                    started: Some(Instant::now()),
                    ..Default::default()
                };
                self.auto_scroll = true
            }
            Action::StreamConnected { request_id }
                if self.current_request_id() == Some(request_id) =>
            {
                self.generation = GenerationState::Streaming { request_id }
            }
            Action::StreamDelta {
                request_id,
                text,
                elapsed,
            } if self.current_request_id() == Some(request_id) => {
                if self.metrics.first_delta.is_none() {
                    self.metrics.first_delta = Some(elapsed);
                }
                self.metrics.deltas += 1;
                self.metrics.bytes += text.len();
                if let Some(m) = self
                    .messages
                    .last_mut()
                    .filter(|m| m.status == MessageStatus::Streaming)
                {
                    m.content.push_str(&text);
                }
            }
            Action::StreamCompleted { request_id, total }
                if self.current_request_id() == Some(request_id) =>
            {
                self.metrics.total = Some(total);
                self.generation = GenerationState::Idle;
                if let Some(m) = self.messages.last_mut() {
                    m.status = MessageStatus::Complete;
                }
                if self.messages.len() >= 2 {
                    self.persistence = PersistenceState::Saving;
                    return vec![Effect::SaveTranscript {
                        messages: self.history_values(),
                    }];
                }
            }
            Action::StreamFailed {
                request_id,
                message,
            } if self.current_request_id() == Some(request_id) => {
                self.generation = GenerationState::Failed(message.clone());
                if let Some(m) = self.messages.last_mut() {
                    m.status = MessageStatus::Failed;
                }
                self.notify(format!("Request failed: {message}"));
            }
            Action::StreamCancelled { request_id }
                if self.current_request_id() == Some(request_id) =>
            {
                self.generation = GenerationState::Idle;
                if let Some(m) = self.messages.last_mut() {
                    m.status = MessageStatus::Cancelled;
                }
                self.notify("Generation cancelled".into());
            }
            Action::Cancel => {
                if let Some(id) = self.current_request_id() {
                    self.generation = GenerationState::Cancelling { request_id: id };
                    return vec![Effect::Cancel { request_id: id }];
                }
            }
            Action::HistorySaved => self.persistence = PersistenceState::Clean,
            Action::HistorySaveFailed(e) => {
                self.persistence = PersistenceState::Unsaved(e.clone());
                self.notify(format!("History was not saved: {e}"));
            }
            Action::SessionsLoaded(v) => self.sessions = v,
            Action::ModelsLoaded(v) => self.models = v,
            Action::SessionChanged(v) => {
                self.session = v.clone();
                self.project = v.project.clone();
                self.messages = v.history.iter().filter_map(from_value).collect();
                self.modal = None;
                self.generation = GenerationState::Idle;
                return vec![Effect::SavePreferences];
            }
            Action::ModelChanged(v) => {
                self.model = v;
                self.modal = None;
                return vec![Effect::SavePreferences];
            }
            Action::Command(c) => return self.command(c),
            Action::Retry => return self.retry(),
            Action::EditLast => {
                if self.busy() {
                    self.notify("Cancel the active response before editing".into());
                } else if let Some(v) = self.last_user.clone() {
                    self.composer.set(v.clone());
                    if self.messages.last().is_some_and(|m| m.role == "assistant") {
                        self.messages.pop();
                    }
                    if self
                        .messages
                        .last()
                        .is_some_and(|m| m.role == "user" && m.content == v)
                    {
                        self.messages.pop();
                    }
                    self.persistence = PersistenceState::Unsaved(
                        "Edited turn will replace saved history after resend".into(),
                    );
                } else {
                    self.notify("There is no request to edit".into());
                }
            }
            Action::Search(q) => {
                self.search = Some(q);
                self.modal = None
            }
            Action::ClearSearch => self.search = None,
            _ => {}
        }
        vec![]
    }
    fn submit(&mut self) -> Vec<Effect> {
        if self.busy() {
            self.notify("Cancel the active response before sending another message".into());
            return vec![];
        }
        let input = self.composer.text();
        if input.trim().is_empty() {
            return vec![];
        }
        if input.trim_start().starts_with('/') {
            match super::commands::parse(&input) {
                Ok(Some(c)) => {
                    self.composer.clear();
                    return self.command(c);
                }
                Err(e) => {
                    self.notify(e);
                    return vec![];
                }
                _ => {}
            }
        }
        self.composer.clear();
        self.start(input)
    }
    fn start(&mut self, user: String) -> Vec<Effect> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.update(Action::StartRequest {
            request_id: id,
            user,
        });
        vec![Effect::Send {
            request_id: id,
            messages: self.history_values(),
            model: self.model.clone(),
        }]
    }
    fn retry(&mut self) -> Vec<Effect> {
        if self.busy() {
            self.notify("Cancel the active response before retrying".into());
            return vec![];
        }
        let Some(user) = self.last_user.clone() else {
            self.notify("There is no request to retry".into());
            return vec![];
        };
        if self.messages.last().is_some_and(|m| m.role == "assistant") {
            self.messages.pop();
        }
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == "user" && m.content == user)
        {
            self.messages.pop();
        }
        self.persistence =
            PersistenceState::Unsaved("Retry will replace saved history after completion".into());
        self.start(user)
    }
    fn command(&mut self, c: ParsedCommand) -> Vec<Effect> {
        let args = c.args.join(" ");
        match c.name.as_str() {
            "help" | "shortcuts" => self.modal = Some(Modal::Help),
            "status" | "diagnostics" => return vec![Effect::LoadDiagnostics],
            "models" => return vec![Effect::LoadModels],
            "new" => return vec![Effect::CreateSession((!args.is_empty()).then_some(args))],
            "sessions" => return vec![Effect::LoadSessions],
            "switch" => {
                if let Some(id) = c.args.first() {
                    return vec![Effect::SwitchSession(id.clone())];
                } else {
                    self.notify("Usage: /switch <id|index>".into())
                }
            }
            "rename" => {
                if args.is_empty() {
                    self.notify("Usage: /rename <title>".into())
                } else {
                    return vec![Effect::RenameSession(args)];
                }
            }
            "delete" => self.modal = Some(Modal::Confirm(ConfirmAction::DeleteSession)),
            "clear" => self.modal = Some(Modal::Confirm(ConfirmAction::ClearHistory)),
            "project" => {
                if args.is_empty() {
                    self.modal_input.clear();
                    self.modal_selected = if self.projects.is_empty() {
                        0
                    } else {
                        self.projects
                            .iter()
                            .position(|project| project == &self.project)
                            .unwrap_or(self.projects.len())
                    };
                    self.modal = Some(Modal::Projects);
                } else {
                    return vec![Effect::ChangeProject(Some(args))];
                }
            }
            "model" => {
                if args.is_empty() {
                    return vec![Effect::LoadModels];
                }
                return vec![Effect::SelectModel(args)];
            }
            "retry" => return self.retry(),
            "edit" => return self.update(Action::EditLast),
            "cancel" => return self.update(Action::Cancel),
            "search" => {
                if args.is_empty() {
                    self.modal = Some(Modal::Search)
                } else {
                    self.search = Some(args)
                }
            }
            "copy" => {
                return vec![Effect::Copy(
                    c.args.first().cloned().unwrap_or_else(|| "last".into()),
                )];
            }
            "logs" => return vec![Effect::LoadLogs],
            "key" => {
                self.modal_input.clear();
                self.modal = Some(Modal::Key);
            }
            "settings" => self.modal = Some(Modal::Settings),
            "quit" => return self.update(Action::QuitRequested),
            _ => {}
        }
        vec![]
    }
    fn modal_submit(&mut self) -> Vec<Effect> {
        match self.modal.clone() {
            Some(Modal::Confirm(_)) => self.confirm(),
            Some(Modal::Sessions) => self
                .sessions
                .get(self.modal_selected)
                .map(|session| vec![Effect::SwitchSession(session.id.clone())])
                .unwrap_or_default(),
            Some(Modal::Models) => self
                .models
                .get(self.modal_selected)
                .map(|model| vec![Effect::SelectModel(model.id.clone())])
                .unwrap_or_default(),
            Some(Modal::Search) => {
                let query = self.modal_input.text();
                self.modal_input.clear();
                if query.trim().is_empty() {
                    self.update(Action::ClearSearch);
                    self.modal = None;
                    vec![]
                } else {
                    self.update(Action::Search(query))
                }
            }
            Some(Modal::Projects) => {
                let typed = self.modal_input.text();
                let path = if self.modal_selected < self.projects.len() {
                    self.projects[self.modal_selected].clone()
                } else {
                    typed
                };
                if path.trim().is_empty() {
                    self.notify("Enter an absolute or ~/ project path".into());
                    vec![]
                } else {
                    vec![Effect::ChangeProject(Some(path))]
                }
            }
            Some(Modal::Settings) => {
                match self.modal_selected {
                    0 => self.sidebar = !self.sidebar,
                    1 => self.no_color = !self.no_color,
                    _ => return vec![],
                }
                vec![Effect::SavePreferences]
            }
            Some(Modal::Key) => {
                let key = self.modal_input.text();
                if key.trim().is_empty() {
                    self.notify("API key was empty; configuration was not changed".into());
                    vec![]
                } else {
                    self.modal_input.clear();
                    self.modal = None;
                    vec![Effect::SaveKey(key)]
                }
            }
            _ => vec![],
        }
    }
    fn confirm(&mut self) -> Vec<Effect> {
        match self.modal.take() {
            Some(Modal::Confirm(ConfirmAction::DeleteSession)) => vec![Effect::DeleteSession],
            Some(Modal::Confirm(ConfirmAction::ClearHistory)) => vec![Effect::ClearHistory],
            Some(Modal::Confirm(ConfirmAction::Quit)) => {
                if let Some(id) = self.current_request_id() {
                    vec![Effect::Cancel { request_id: id }, Effect::Quit]
                } else {
                    vec![Effect::Quit]
                }
            }
            _ => vec![],
        }
    }
    fn notify(&mut self, value: String) {
        if self.notifications.len() >= 4 {
            self.notifications.pop_front();
        }
        self.notifications.push_back(value)
    }
}
fn from_value(v: &Value) -> Option<ChatMessage> {
    Some(ChatMessage {
        role: v.get("role")?.as_str()?.into(),
        content: crate::openai::text_from_content(v.get("content").unwrap_or(&Value::Null)),
        status: MessageStatus::Complete,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> SessionRecord {
        serde_json::from_value(json!({"id":"proxy-12345678","title":"Test","project":"/tmp","automatic":false,"created_at":"x","updated_at":"x","history":[],"sidecar":{}})).unwrap()
    }
    #[test]
    fn late_stream_events_are_ignored() {
        let mut a = App::new("/tmp".into(), session(), "gpt".into(), true, false);
        a.update(Action::StartRequest {
            request_id: 2,
            user: "x".into(),
        });
        a.update(Action::StreamDelta {
            request_id: 1,
            text: "wrong".into(),
            elapsed: Duration::ZERO,
        });
        assert_eq!(a.messages.last().unwrap().content, "");
    }
    #[test]
    fn completion_saves_exact_turn() {
        let mut a = App::new("/tmp".into(), session(), "gpt".into(), true, false);
        a.update(Action::StartRequest {
            request_id: 1,
            user: "x".into(),
        });
        a.update(Action::StreamDelta {
            request_id: 1,
            text: "y".into(),
            elapsed: Duration::ZERO,
        });
        assert!(matches!(
            a.update(Action::StreamCompleted {
                request_id: 1,
                total: Duration::from_secs(1)
            })
            .as_slice(),
            [Effect::SaveTranscript { .. }]
        ));
    }
    #[test]
    fn destructive_commands_confirm() {
        let mut a = App::new("/tmp".into(), session(), "gpt".into(), true, false);
        a.composer.set("/delete");
        assert!(a.update(Action::Submit).is_empty());
        assert_eq!(a.modal, Some(Modal::Confirm(ConfirmAction::DeleteSession)));
        assert_eq!(a.update(Action::Confirmed), vec![Effect::DeleteSession]);
    }
    #[test]
    fn failed_and_cancelled_generations_do_not_persist() {
        for cancelled in [false, true] {
            let mut a = App::new("/tmp".into(), session(), "gpt".into(), true, false);
            a.update(Action::StartRequest {
                request_id: 1,
                user: "x".into(),
            });
            let effects = if cancelled {
                a.update(Action::StreamCancelled { request_id: 1 })
            } else {
                a.update(Action::StreamFailed {
                    request_id: 1,
                    message: "broken".into(),
                })
            };
            assert!(effects.is_empty());
            assert_ne!(a.messages.last().unwrap().status, MessageStatus::Complete);
        }
    }
    #[test]
    fn retry_replaces_the_previous_turn_in_transcript() {
        let mut a = App::new("/tmp".into(), session(), "gpt".into(), true, false);
        a.update(Action::StartRequest {
            request_id: 1,
            user: "question".into(),
        });
        a.update(Action::StreamDelta {
            request_id: 1,
            text: "old".into(),
            elapsed: Duration::ZERO,
        });
        a.update(Action::StreamCompleted {
            request_id: 1,
            total: Duration::ZERO,
        });
        let effects = a.update(Action::Retry);
        assert!(matches!(effects.as_slice(), [Effect::Send { .. }]));
        assert_eq!(a.messages.iter().filter(|m| m.role == "user").count(), 1);
        assert_eq!(a.messages.last().unwrap().status, MessageStatus::Streaming);
    }
    #[test]
    fn modal_pickers_and_settings_apply_selected_values() {
        let mut a = App::new("/tmp".into(), session(), "old".into(), true, false);
        a.models = vec![ModelInfo {
            id: "new".into(),
            object: "model".into(),
            created: 0,
            owned_by: "test".into(),
        }];
        a.modal = Some(Modal::Models);
        assert_eq!(
            a.update(Action::ModalSubmit),
            vec![Effect::SelectModel("new".into())]
        );
        a.modal = Some(Modal::Settings);
        a.modal_selected = 0;
        assert_eq!(a.update(Action::ModalSubmit), vec![Effect::SavePreferences]);
        assert!(!a.sidebar);
    }
    #[test]
    fn direct_model_and_project_commands_have_real_effects() {
        let mut a = App::new("/tmp".into(), session(), "old".into(), true, false);
        a.composer.set("/model new");
        assert_eq!(
            a.update(Action::Submit),
            vec![Effect::SelectModel("new".into())]
        );
        a.composer.set("/project");
        assert!(a.update(Action::Submit).is_empty());
        assert_eq!(a.modal, Some(Modal::Projects));
    }
}
