#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Composer,
    Tree,
    Chat,
}

#[allow(non_upper_case_globals)]
impl PaneFocus {
    pub const Workspace: Self = Self::Tree;

    pub fn next(self) -> Self {
        match self {
            Self::Composer => Self::Tree,
            Self::Tree => Self::Chat,
            Self::Chat => Self::Composer,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Composer => Self::Chat,
            Self::Tree => Self::Composer,
            Self::Chat => Self::Tree,
        }
    }
}

impl From<PaneFocus> for crate::app::FocusedPane {
    fn from(focus: PaneFocus) -> Self {
        match focus {
            PaneFocus::Tree => Self::Tree,
            PaneFocus::Chat => Self::Transcript,
            PaneFocus::Composer => Self::Composer,
        }
    }
}

impl From<crate::app::FocusedPane> for PaneFocus {
    fn from(pane: crate::app::FocusedPane) -> Self {
        match pane {
            crate::app::FocusedPane::Tree => Self::Tree,
            crate::app::FocusedPane::Transcript | crate::app::FocusedPane::Terminal => Self::Chat,
            crate::app::FocusedPane::Composer => Self::Composer,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FocusNavigation {
    current: PaneFocus,
}

impl FocusNavigation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> PaneFocus {
        self.current
    }

    pub fn next(&mut self) {
        self.current = self.current.next();
    }

    pub fn prev(&mut self) {
        self.current = self.current.prev();
    }

    pub fn set_focus(&mut self, focus: PaneFocus) {
        self.current = focus;
    }
}
