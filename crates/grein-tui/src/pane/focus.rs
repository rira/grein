/// Active focus pane within the TUI workspace layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaneFocus {
    #[default]
    Workspace,
    Composer,
    Logs,
}

impl PaneFocus {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::Composer => "Composer",
            Self::Logs => "Logs",
        }
    }

    #[must_use]
    pub fn next(&self) -> Self {
        match self {
            Self::Workspace => Self::Composer,
            Self::Composer => Self::Logs,
            Self::Logs => Self::Workspace,
        }
    }

    #[must_use]
    pub fn previous(&self) -> Self {
        match self {
            Self::Workspace => Self::Logs,
            Self::Composer => Self::Workspace,
            Self::Logs => Self::Composer,
        }
    }
}

impl From<PaneFocus> for crate::app::FocusedPane {
    fn from(focus: PaneFocus) -> Self {
        match focus {
            PaneFocus::Workspace => Self::Tree,
            PaneFocus::Logs => Self::Transcript,
            PaneFocus::Composer => Self::Composer,
        }
    }
}

impl From<crate::app::FocusedPane> for PaneFocus {
    fn from(pane: crate::app::FocusedPane) -> Self {
        match pane {
            crate::app::FocusedPane::Tree => Self::Workspace,
            crate::app::FocusedPane::Transcript | crate::app::FocusedPane::Terminal => Self::Logs,
            crate::app::FocusedPane::Composer => Self::Composer,
        }
    }
}
