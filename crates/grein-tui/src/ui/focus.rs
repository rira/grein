//! Focus navigation state machine for the 4-pane layout.

/// Represents the currently focused pane in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    /// Thread and Anchor Navigator (left pane).
    #[default]
    Tree,
    /// Conversation thread display.
    Thread,
    /// Message composer and input prompt.
    Composer,
    /// Diff and inspection viewer.
    Diff,
}

const PANES: [PaneFocus; 4] = [
    PaneFocus::Tree,
    PaneFocus::Thread,
    PaneFocus::Composer,
    PaneFocus::Diff,
];

/// Manages focus transitions across the TUI panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FocusNavigation {
    index: usize,
}

impl FocusNavigation {
    /// Creates a new focus navigation instance initialized to the default pane.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the currently focused pane.
    #[must_use]
    pub fn current(&self) -> PaneFocus {
        PANES[self.index % PANES.len()]
    }

    /// Cycles focus forward to the next pane (Tab).
    pub fn next(&mut self) {
        self.index = (self.index + 1) % PANES.len();
    }

    /// Cycles focus backward to the previous pane (BackTab).
    pub fn prev(&mut self) {
        self.index = (self.index + PANES.len() - 1) % PANES.len();
    }

    /// Explicitly sets focus to a specific pane.
    pub fn set_focus(&mut self, focus: PaneFocus) {
        if let Some(pos) = PANES.iter().position(|&p| p == focus) {
            self.index = pos;
        }
    }
}
