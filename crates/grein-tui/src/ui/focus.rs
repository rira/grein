#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Composer,
    Tree,
    Chat,
    Blackboard,
}

#[allow(non_upper_case_globals)]
impl PaneFocus {
    pub const Workspace: Self = Self::Tree;

    pub fn next(self) -> Self {
        match self {
            Self::Composer => Self::Tree,
            Self::Tree => Self::Chat,
            Self::Chat => Self::Blackboard,
            Self::Blackboard => Self::Composer,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Composer => Self::Blackboard,
            Self::Tree => Self::Composer,
            Self::Chat => Self::Tree,
            Self::Blackboard => Self::Chat,
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
