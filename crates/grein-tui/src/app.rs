#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePane {
    #[default]
    Composer,
    ThreadStream,
    SessionSidebar,
    WorkspaceInspector,
}

impl ActivePane {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Composer => Self::ThreadStream,
            Self::ThreadStream => Self::SessionSidebar,
            Self::SessionSidebar => Self::WorkspaceInspector,
            Self::WorkspaceInspector => Self::Composer,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Composer => Self::WorkspaceInspector,
            Self::ThreadStream => Self::Composer,
            Self::SessionSidebar => Self::ThreadStream,
            Self::WorkspaceInspector => Self::SessionSidebar,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppState {
    active_pane: ActivePane,
    composer_buffer: String,
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn active_pane(&self) -> ActivePane {
        self.active_pane
    }

    pub fn set_active_pane(&mut self, pane: ActivePane) {
        self.active_pane = pane;
    }

    pub fn cycle_focus_next(&mut self) {
        self.active_pane = self.active_pane.next();
    }

    pub fn cycle_focus_prev(&mut self) {
        self.active_pane = self.active_pane.prev();
    }

    #[must_use]
    pub fn composer_input(&self) -> &str {
        &self.composer_buffer
    }

    pub fn handle_char_input(&mut self, ch: char) {
        if self.active_pane == ActivePane::Composer {
            self.composer_buffer.push(ch);
        }
    }

    pub fn handle_backspace(&mut self) {
        if self.active_pane == ActivePane::Composer {
            self.composer_buffer.pop();
        }
    }
}
