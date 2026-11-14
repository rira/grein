use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Represents the four navigable panes in the Grein TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FocusPane {
    #[default]
    WorkspaceTree,
    ChatStream,
    Composer,
    ContextInspector,
}

impl FocusPane {
    /// Advances focus forward in the cycle:
    /// `WorkspaceTree -> ChatStream -> Composer -> ContextInspector -> WorkspaceTree`.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::ChatStream,
            Self::ChatStream => Self::Composer,
            Self::Composer => Self::ContextInspector,
            Self::ContextInspector => Self::WorkspaceTree,
        }
    }

    /// Moves focus backward in the cycle:
    /// `WorkspaceTree -> ContextInspector -> Composer -> ChatStream -> WorkspaceTree`.
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::ContextInspector,
            Self::ChatStream => Self::WorkspaceTree,
            Self::Composer => Self::ChatStream,
            Self::ContextInspector => Self::Composer,
        }
    }
}

/// Root state container for the Grein terminal user interface.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct App {
    focused_pane: FocusPane,
    should_quit: bool,
}

impl App {
    /// Creates a new `App` instance with default focus on `FocusPane::WorkspaceTree`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused_pane: FocusPane::WorkspaceTree,
            should_quit: false,
        }
    }

    /// Returns the currently focused pane.
    #[must_use]
    pub const fn focused_pane(&self) -> FocusPane {
        self.focused_pane
    }

    /// Cycles focus forward to the next pane.
    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    /// Cycles focus backward to the previous pane.
    pub fn focus_prev(&mut self) {
        self.focused_pane = self.focused_pane.prev();
    }

    /// Returns whether the application has requested termination.
    #[must_use]
    pub const fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Handles an incoming terminal key event.
    pub fn handle_key_event(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        match key.code {
            KeyCode::BackTab => {
                self.focus_prev();
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.focus_prev();
                } else {
                    self.focus_next();
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            _ => {}
        }
    }
}
