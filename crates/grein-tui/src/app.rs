use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

/// Identifiers for the four primary workspace panes in the TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    WorkspaceTree,
    AgentStream,
    Composer,
    DiffPreview,
}

impl FocusPane {
    /// Advance focus to the next pane in clockwise order.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::AgentStream,
            Self::AgentStream => Self::Composer,
            Self::Composer => Self::DiffPreview,
            Self::DiffPreview => Self::WorkspaceTree,
        }
    }

    /// Move focus to the previous pane in counter-clockwise order.
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::DiffPreview,
            Self::AgentStream => Self::WorkspaceTree,
            Self::Composer => Self::AgentStream,
            Self::DiffPreview => Self::Composer,
        }
    }
}

/// Sealed domain error hierarchy for TUI operations.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TuiError {
    #[error("Terminal I/O error: {0}")]
    Io(String),
}

/// Core TUI application state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub focus: FocusPane,
    pub input_buffer: String,
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Construct a new `App` instance in its initial state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            focus: FocusPane::default(),
            input_buffer: String::new(),
            should_quit: false,
        }
    }

    /// Return the currently focused pane.
    #[must_use]
    pub const fn focus(&self) -> FocusPane {
        self.focus
    }

    /// Return the current contents of the composer input buffer.
    #[must_use]
    pub fn input_buffer(&self) -> &str {
        &self.input_buffer
    }

    /// Return whether the application has been flagged to exit.
    #[must_use]
    pub const fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Process a single terminal key event.
    ///
    /// Returns `Ok(true)` if the application should exit, or `Ok(false)` to continue.
    ///
    /// # Errors
    ///
    /// Returns `TuiError` if event handling encounters an unrecoverable state error.
    pub fn handle_key_event(&mut self, key: KeyEvent) -> Result<bool, TuiError> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return Ok(true);
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = self.focus.next();
                Ok(false)
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev();
                Ok(false)
            }
            KeyCode::Char('q') if self.focus != FocusPane::Composer => {
                self.should_quit = true;
                Ok(true)
            }
            KeyCode::Char(c) if self.focus == FocusPane::Composer => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    self.input_buffer.push(c);
                }
                Ok(false)
            }
            KeyCode::Backspace if self.focus == FocusPane::Composer => {
                self.input_buffer.pop();
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_composer_typing_immunity_for_q_key() {
        let mut app = App::new();
        app.focus = FocusPane::Composer;

        let key_event = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key_event(key_event);

        assert_eq!(result, Ok(false));
        assert_eq!(app.input_buffer(), "q");
    }
}
