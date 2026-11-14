//! Pane focus definitions and cyclic navigation.

/// Represents the currently active pane in the TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    /// Bottom pane prompt composer (default active pane).
    #[default]
    Composer,
    /// Left pane active sessions overview.
    Sessions,
    /// Central chat conversation view.
    Chat,
    /// Right pane diff review and inspector view.
    Diff,
}

impl PaneFocus {
    /// Cycles forward to the next pane in sequence:
    /// Composer -> Sessions -> Chat -> Diff -> Composer.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Composer => Self::Sessions,
            Self::Sessions => Self::Chat,
            Self::Chat => Self::Diff,
            Self::Diff => Self::Composer,
        }
    }

    /// Cycles backward to the previous pane in sequence:
    /// Composer -> Diff -> Chat -> Sessions -> Composer.
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Composer => Self::Diff,
            Self::Sessions => Self::Composer,
            Self::Chat => Self::Sessions,
            Self::Diff => Self::Chat,
        }
    }
}
