//! TUI focus navigation and state management.

/// Identifies which pane in the TUI currently has user focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaneFocus {
    /// Sidebar pane (navigation / thread selection).
    #[default]
    Sidebar,
    /// Active thread transcript pane (center pane).
    Transcript,
    /// Composer input pane (prompt / message editor).
    Composer,
    /// Inspector details pane.
    Inspector,
}

impl PaneFocus {
    /// Returns the next pane in forward cyclic navigation order.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::Transcript,
            Self::Transcript => Self::Composer,
            Self::Composer => Self::Inspector,
            Self::Inspector => Self::Sidebar,
        }
    }

    /// Returns the previous pane in backward cyclic navigation order.
    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Sidebar => Self::Inspector,
            Self::Transcript => Self::Sidebar,
            Self::Composer => Self::Transcript,
            Self::Inspector => Self::Composer,
        }
    }
}

/// Navigation action triggered by keyboard shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusNavigationAction {
    /// Forward navigation to next pane.
    Tab,
    /// Backward navigation to previous pane.
    BackTab,
}

/// State machine managing the current active pane focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusManager {
    current: PaneFocus,
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new(PaneFocus::Sidebar)
    }
}

impl FocusManager {
    /// Creates a new `FocusManager` initialized to the specified pane.
    #[must_use]
    pub const fn new(initial: PaneFocus) -> Self {
        Self { current: initial }
    }

    /// Returns the currently focused pane.
    #[must_use]
    pub const fn current(&self) -> PaneFocus {
        self.current
    }

    /// Updates focus according to the provided navigation action.
    pub fn handle_action(&mut self, action: FocusNavigationAction) {
        match action {
            FocusNavigationAction::Tab => {
                self.current = self.current.next();
            }
            FocusNavigationAction::BackTab => {
                self.current = self.current.previous();
            }
        }
    }
}
