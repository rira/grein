use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    None,
    Submit(String),
}

#[derive(Debug, Default, Clone)]
pub struct InputComposer {
    active_thread: Option<String>,
    drafts: HashMap<String, String>,
}

impl InputComposer {
    pub fn new() -> Self {
        Self {
            active_thread: None,
            drafts: HashMap::new(),
        }
    }

    pub fn set_active_thread(&mut self, thread_id: impl Into<String>) {
        self.active_thread = Some(thread_id.into());
    }

    pub fn get_text(&self) -> &str {
        match &self.active_thread {
            Some(thread_id) => self.drafts.get(thread_id).map(|s| s.as_str()).unwrap_or(""),
            None => "",
        }
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> ComposerAction {
        let thread_id = match &self.active_thread {
            Some(id) => id.clone(),
            None => return ComposerAction::None,
        };

        match event.code {
            KeyCode::Enter => {
                if event.modifiers.contains(KeyModifiers::SHIFT)
                    || event.modifiers.contains(KeyModifiers::CONTROL)
                {
                    let draft = self.drafts.entry(thread_id).or_default();
                    draft.push('\n');
                    ComposerAction::None
                } else {
                    let text = self.drafts.remove(&thread_id).unwrap_or_default();
                    ComposerAction::Submit(text)
                }
            }
            KeyCode::Char(ch) => {
                let draft = self.drafts.entry(thread_id).or_default();
                draft.push(ch);
                ComposerAction::None
            }
            KeyCode::Backspace => {
                if let Some(draft) = self.drafts.get_mut(&thread_id) {
                    draft.pop();
                }
                ComposerAction::None
            }
            _ => ComposerAction::None,
        }
    }
}
