use crate::{AppAction, AppEvent, PaneFocus, TuiComponent};
use crossterm::event::KeyCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub role: MessageRole,
    pub content: String,
}

impl TranscriptMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }

    pub fn role(&self) -> &MessageRole {
        &self.role
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptPane {
    focused: bool,
    messages: Vec<TranscriptMessage>,
    scroll_offset: usize,
}

impl TranscriptPane {
    pub fn new() -> Self {
        Self {
            focused: false,
            messages: Vec::new(),
            scroll_offset: 0,
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn add_message(&mut self, message: TranscriptMessage) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[TranscriptMessage] {
        &self.messages
    }

    pub fn handle_event(&mut self, event: &AppEvent) -> bool {
        match event {
            AppEvent::FocusChanged(target) => {
                self.focused = *target == PaneFocus::Transcript;
                true
            }
            AppEvent::Key(key) => {
                if !self.focused {
                    return false;
                }
                matches!(key.code, KeyCode::Up | KeyCode::Down)
            }
            _ => false,
        }
    }

    pub fn dispatch_key(&mut self, event: &AppEvent) -> Option<AppAction> {
        if !self.focused {
            return None;
        }
        match event {
            AppEvent::Key(key) => match key.code {
                KeyCode::Down => {
                    self.scroll_offset = self.scroll_offset.saturating_add(1);
                    Some(AppAction::Scrolled { delta: 1 })
                }
                KeyCode::Up => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    Some(AppAction::Scrolled { delta: -1 })
                }
                _ => None,
            },
            _ => None,
        }
    }
}

impl TuiComponent for TranscriptPane {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn handle_event(&mut self, event: &AppEvent) -> bool {
        self.handle_event(event)
    }

    fn dispatch_key(&mut self, event: &AppEvent) -> Option<AppAction> {
        self.dispatch_key(event)
    }
}
