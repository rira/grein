//! Pane focus definitions and cyclic navigation.

/// Represents the currently active pane in the TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum PaneFocus {
    /// Bottom pane prompt composer (default active pane).
    #[default]
    Composer,
    Inspector,
    Sidebar,
    Transcript,
    Sessions,
    Chat,
    Diff,
    Details,
}

#[allow(non_upper_case_globals)]
impl PaneFocus {
    pub const Output: Self = Self::Transcript;
    /// Cycles forward to the next pane in sequence:
    /// Sidebar -> Transcript -> Composer -> Inspector -> Sidebar
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::Transcript,
            Self::Transcript => Self::Composer,
            Self::Composer => Self::Inspector,
            Self::Inspector => Self::Sidebar,
            Self::Sessions => Self::Chat,
            Self::Chat => Self::Diff,
            Self::Diff => Self::Composer,
            Self::Details => Self::Sidebar,
        }
    }

    /// Cycles backward to the previous pane in sequence:
    /// Sidebar <- Transcript <- Composer <- Inspector <- Sidebar
    #[must_use]
    pub const fn prev(self) -> Self {
        self.previous()
    }

    /// Cycles backward to the previous pane in sequence:
    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Sidebar => Self::Inspector,
            Self::Transcript => Self::Sidebar,
            Self::Composer => Self::Transcript,
            Self::Inspector => Self::Composer,
            Self::Sessions => Self::Composer,
            Self::Chat => Self::Sessions,
            Self::Diff => Self::Chat,
            Self::Details => Self::Composer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusNavigationAction {
    Tab,
    BackTab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusManager {
    current: PaneFocus,
}

impl FocusManager {
    pub fn new(initial: PaneFocus) -> Self {
        Self { current: initial }
    }

    pub fn current(&self) -> PaneFocus {
        self.current
    }

    pub fn handle_action(&mut self, action: FocusNavigationAction) {
        match action {
            FocusNavigationAction::Tab => self.current = self.current.next(),
            FocusNavigationAction::BackTab => self.current = self.current.previous(),
        }
    }
}
