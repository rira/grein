use crate::focus::PaneFocus;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use std::time::Duration;

/// Root TUI application state coordinating focus and input routing.
#[derive(Debug, Default)]
pub struct App {
    focus: PaneFocus,
    composer_content: String,
    last_submitted_prompt: Option<String>,
}

impl App {
    /// Creates a new TUI application instance.
    pub fn new() -> Self {
        Self {
            focus: PaneFocus::default(),
            composer_content: String::new(),
            last_submitted_prompt: None,
        }
    }

    /// Returns current pane focus.
    pub fn focus(&self) -> PaneFocus {
        self.focus
    }

    /// Explicitly updates pane focus.
    pub fn set_focus(&mut self, focus: PaneFocus) {
        self.focus = focus;
    }

    /// Returns the current text buffer of the composer pane.
    pub fn composer_content(&self) -> &str {
        &self.composer_content
    }

    /// Returns the last submitted prompt, if any.
    pub fn last_submitted_prompt(&self) -> Option<String> {
        self.last_submitted_prompt.clone()
    }

    /// Routes keyboard events based on active focus and key modifier.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = self.focus.next();
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev();
            }
            _ => {
                if self.focus == PaneFocus::Composer {
                    match key.code {
                        KeyCode::Char(c) => {
                            self.composer_content.push(c);
                        }
                        KeyCode::Backspace => {
                            self.composer_content.pop();
                        }
                        KeyCode::Enter => {
                            let prompt = std::mem::take(&mut self.composer_content);
                            self.last_submitted_prompt = Some(prompt);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Performs non-blocking event polling and event dispatching.
    pub fn poll_events(&mut self, timeout: Duration) -> std::io::Result<()> {
        if event::poll(timeout).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                self.handle_key(key);
            }
        }
        Ok(())
    }
}
