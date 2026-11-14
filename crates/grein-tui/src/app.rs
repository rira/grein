use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Focus state across the primary TUI panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Composer,
    Timeline,
    Workspace,
    Blackboard,
}

/// Central application state for the Grein TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    focused_pane: PaneFocus,
    blackboard_collapsed: bool,
    should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            focused_pane: PaneFocus::Composer,
            blackboard_collapsed: false,
            should_quit: false,
        }
    }
}

impl App {
    /// Creates a new application state instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the currently focused pane.
    pub fn focused_pane(&self) -> PaneFocus {
        self.focused_pane
    }

    /// Sets the focused pane.
    pub fn set_focused_pane(&mut self, pane: PaneFocus) {
        self.focused_pane = pane;
    }

    /// Returns whether the blackboard pane is collapsed.
    pub fn is_blackboard_collapsed(&self) -> bool {
        self.blackboard_collapsed
    }

    /// Sets the collapsed state of the blackboard pane.
    pub fn set_blackboard_collapsed(&mut self, collapsed: bool) {
        self.blackboard_collapsed = collapsed;
    }

    /// Returns whether the application should exit.
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Transitions focus to the next pane in the focus cycle.
    pub fn focus_next(&mut self) {
        self.focused_pane = match self.focused_pane {
            PaneFocus::Composer => PaneFocus::Timeline,
            PaneFocus::Timeline => PaneFocus::Workspace,
            PaneFocus::Workspace => PaneFocus::Blackboard,
            PaneFocus::Blackboard => PaneFocus::Composer,
        };
    }

    /// Transitions focus to the previous pane in reverse cycle.
    pub fn focus_previous(&mut self) {
        self.focused_pane = match self.focused_pane {
            PaneFocus::Composer => PaneFocus::Blackboard,
            PaneFocus::Blackboard => PaneFocus::Workspace,
            PaneFocus::Workspace => PaneFocus::Timeline,
            PaneFocus::Timeline => PaneFocus::Composer,
        };
    }

    /// Handles keyboard events for navigation and pane control.
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::BackTab => {
                self.focus_previous();
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.focus_previous();
                } else {
                    self.focus_next();
                }
            }
            KeyCode::Char('b') | KeyCode::Char('B')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.blackboard_collapsed = !self.blackboard_collapsed;
            }
            KeyCode::Char('[') => {
                self.blackboard_collapsed = true;
            }
            KeyCode::Char(']') => {
                self.blackboard_collapsed = false;
            }
            _ => {}
        }
    }
}
