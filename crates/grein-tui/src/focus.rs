//! Focus management across TUI panes.

/// Identifies the active focus pane in the 4-pane TUI navigation cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaneFocus {
    /// Workspaces navigation pane.
    #[default]
    Workspaces,
    /// Tasks list pane.
    Tasks,
    /// Messages and event log pane.
    Messages,
    /// Non-blocking prompt input composer pane.
    Composer,
}

impl PaneFocus {
    #[allow(non_upper_case_globals)]
    pub const Workspace: Self = Self::Workspaces;
    #[allow(non_upper_case_globals)]
    pub const Task: Self = Self::Tasks;
    #[allow(non_upper_case_globals)]
    pub const Message: Self = Self::Messages;

    /// All available pane focus variants in cycle order.
    pub const ALL: [Self; 4] = [
        Self::Workspaces,
        Self::Tasks,
        Self::Messages,
        Self::Composer,
    ];

    /// Advances focus to the next pane in the 4-pane Tab cycle.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Workspaces => Self::Tasks,
            Self::Tasks => Self::Messages,
            Self::Messages => Self::Composer,
            Self::Composer => Self::Workspaces,
        }
    }

    /// Retreats focus to the previous pane in the 4-pane BackTab cycle.
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Workspaces => Self::Composer,
            Self::Tasks => Self::Workspaces,
            Self::Messages => Self::Tasks,
            Self::Composer => Self::Messages,
        }
    }

    /// Returns all available pane focus variants in cycle order.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &Self::ALL
    }
}

impl std::fmt::Display for PaneFocus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workspaces => write!(f, "Workspaces"),
            Self::Tasks => write!(f, "Tasks"),
            Self::Messages => write!(f, "Messages"),
            Self::Composer => write!(f, "Composer"),
        }
    }
}
