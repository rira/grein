/// Represents the focusable panes in the Ratatui 4-pane layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    SessionTree,
    Output,
    Composer,
    Plan,
}

impl PaneFocus {
    /// Cycle to the next pane in navigation order.
    pub fn next(self) -> Self {
        match self {
            Self::SessionTree => Self::Output,
            Self::Output => Self::Plan,
            Self::Plan => Self::Composer,
            Self::Composer => Self::SessionTree,
        }
    }

    /// Cycle to the previous pane in reverse navigation order.
    pub fn prev(self) -> Self {
        match self {
            Self::SessionTree => Self::Composer,
            Self::Output => Self::SessionTree,
            Self::Plan => Self::Output,
            Self::Composer => Self::Plan,
        }
    }
}
