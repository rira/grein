#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Composer,
    Workspace,
    Chat,
    Blackboard,
}

impl PaneFocus {
    pub fn next(self) -> Self {
        match self {
            Self::Composer => Self::Workspace,
            Self::Workspace => Self::Chat,
            Self::Chat => Self::Blackboard,
            Self::Blackboard => Self::Composer,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Composer => Self::Blackboard,
            Self::Workspace => Self::Composer,
            Self::Chat => Self::Workspace,
            Self::Blackboard => Self::Chat,
        }
    }
}
