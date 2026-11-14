use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    WorkspaceTree,
    AgentOutput,
    PromptComposer,
    DiffInspector,
}

impl PaneFocus {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::AgentOutput,
            Self::AgentOutput => Self::PromptComposer,
            Self::PromptComposer => Self::DiffInspector,
            Self::DiffInspector => Self::WorkspaceTree,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::DiffInspector,
            Self::DiffInspector => Self::PromptComposer,
            Self::PromptComposer => Self::AgentOutput,
            Self::AgentOutput => Self::WorkspaceTree,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        self.previous()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct App {
    focused_pane: PaneFocus,
    composer_input: String,
    should_quit: bool,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn focused_pane(&self) -> PaneFocus {
        self.focused_pane
    }

    #[must_use]
    pub fn composer_input(&self) -> &str {
        &self.composer_input
    }

    #[must_use]
    pub const fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    pub fn focus_previous(&mut self) {
        self.focused_pane = self.focused_pane.previous();
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::BackTab => {
                self.focus_previous();
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_previous();
            }
            KeyCode::Tab => {
                self.focus_next();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('q') if self.focused_pane != PaneFocus::PromptComposer => {
                self.should_quit = true;
            }
            KeyCode::Char(c) if self.focused_pane == PaneFocus::PromptComposer => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    self.composer_input.push(c);
                }
            }
            KeyCode::Backspace if self.focused_pane == PaneFocus::PromptComposer => {
                self.composer_input.pop();
            }
            _ => {}
        }
    }
}

pub async fn run() -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_backtab_and_shift_tab_cycles_focus_backward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), PaneFocus::WorkspaceTree);

        // Cycle backward using KeyCode::BackTab
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), PaneFocus::DiffInspector);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), PaneFocus::PromptComposer);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), PaneFocus::AgentOutput);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), PaneFocus::WorkspaceTree);

        // Cycle backward using KeyCode::Tab + KeyModifiers::SHIFT
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane(), PaneFocus::DiffInspector);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane(), PaneFocus::PromptComposer);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane(), PaneFocus::AgentOutput);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane(), PaneFocus::WorkspaceTree);
    }
}
