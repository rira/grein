use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TuiError {
    #[error("Terminal I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for TuiError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedPane {
    #[default]
    WorkspaceTree,
    AgentStream,
    ContextInspector,
    PromptComposer,
}

impl FocusedPane {
    pub fn next(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::AgentStream,
            Self::AgentStream => Self::ContextInspector,
            Self::ContextInspector => Self::PromptComposer,
            Self::PromptComposer => Self::WorkspaceTree,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::PromptComposer,
            Self::AgentStream => Self::WorkspaceTree,
            Self::ContextInspector => Self::AgentStream,
            Self::PromptComposer => Self::ContextInspector,
        }
    }
}

#[derive(Debug, Default)]
pub struct App {
    pub focused_pane: FocusedPane,
    pub input_buffer: String,
    pub cursor_position: usize,
    pub submitted_prompts: Vec<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn run(&mut self) -> Result<(), TuiError> {
        Ok(())
    }

    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    pub fn focus_prev(&mut self) {
        self.focused_pane = self.focused_pane.prev();
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> Result<bool, TuiError> {
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return Ok(true);
        }

        match key.code {
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.focus_prev();
                } else {
                    self.focus_next();
                }
                Ok(false)
            }
            KeyCode::BackTab => {
                self.focus_prev();
                Ok(false)
            }
            _ => match self.focused_pane {
                FocusedPane::PromptComposer => self.handle_composer_key(key),
                _ => self.handle_navigation_key(key),
            },
        }
    }

    fn handle_navigation_key(&mut self, key: KeyEvent) -> Result<bool, TuiError> {
        if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE {
            self.should_quit = true;
            return Ok(true);
        }
        Ok(false)
    }

    fn handle_composer_key(&mut self, key: KeyEvent) -> Result<bool, TuiError> {
        match key.code {
            KeyCode::Char(c) => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    let byte_idx = self.byte_index_at_cursor();
                    self.input_buffer.insert(byte_idx, c);
                    self.cursor_position += 1;
                }
                Ok(false)
            }
            KeyCode::Backspace => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                    let byte_idx = self.byte_index_at_cursor();
                    self.input_buffer.remove(byte_idx);
                }
                Ok(false)
            }
            KeyCode::Delete => {
                let char_count = self.input_buffer.chars().count();
                if self.cursor_position < char_count {
                    let byte_idx = self.byte_index_at_cursor();
                    self.input_buffer.remove(byte_idx);
                }
                Ok(false)
            }
            KeyCode::Left => {
                self.cursor_position = self.cursor_position.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Right => {
                let char_count = self.input_buffer.chars().count();
                if self.cursor_position < char_count {
                    self.cursor_position += 1;
                }
                Ok(false)
            }
            KeyCode::Home => {
                self.cursor_position = 0;
                Ok(false)
            }
            KeyCode::End => {
                self.cursor_position = self.input_buffer.chars().count();
                Ok(false)
            }
            KeyCode::Enter => {
                if !self.input_buffer.trim().is_empty() {
                    let submitted = std::mem::take(&mut self.input_buffer);
                    self.submitted_prompts.push(submitted);
                } else {
                    self.input_buffer.clear();
                }
                self.cursor_position = 0;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn byte_index_at_cursor(&self) -> usize {
        self.input_buffer
            .char_indices()
            .nth(self.cursor_position)
            .map(|(idx, _)| idx)
            .unwrap_or(self.input_buffer.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_default_focus_is_workspace_tree() {
        let app = App::new();
        assert_eq!(app.focused_pane, FocusedPane::WorkspaceTree);
    }

    #[test]
    fn test_tab_cycles_forward_through_four_panes() {
        let mut app = App::new();
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        app.handle_key_event(tab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::AgentStream);

        app.handle_key_event(tab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::ContextInspector);

        app.handle_key_event(tab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::PromptComposer);

        app.handle_key_event(tab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::WorkspaceTree);
    }

    #[test]
    fn test_backtab_cycles_backward_through_four_panes() {
        let mut app = App::new();
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);

        app.handle_key_event(backtab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::PromptComposer);

        app.handle_key_event(backtab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::ContextInspector);

        app.handle_key_event(backtab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::AgentStream);

        app.handle_key_event(backtab).unwrap();
        assert_eq!(app.focused_pane, FocusedPane::WorkspaceTree);
    }

    #[test]
    fn test_q_quits_in_non_composer_panes() {
        for pane in [
            FocusedPane::WorkspaceTree,
            FocusedPane::AgentStream,
            FocusedPane::ContextInspector,
        ] {
            let mut app = App::new();
            app.focused_pane = pane;
            let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
            let result = app.handle_key_event(key);
            assert_eq!(result, Ok(true));
            assert!(app.should_quit);
        }
    }

    #[test]
    fn test_composer_typing_immunity_for_q_key() {
        let mut app = App::new();
        app.focused_pane = FocusedPane::PromptComposer;

        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key_event(key);

        assert_eq!(result, Ok(false));
        assert_eq!(app.input_buffer, "q");
        assert_eq!(app.cursor_position, 1);
        assert!(!app.should_quit);
    }
}
