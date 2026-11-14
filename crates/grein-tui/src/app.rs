#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pane {
    #[default]
    Workspace,
    Blackboard,
    Chat,
    Composer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub blackboard_collapsed: bool,
    pub focused_pane: Pane,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            blackboard_collapsed: false,
            focused_pane: Pane::Workspace,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focusable_panes(&self) -> Vec<Pane> {
        let all = [
            Pane::Workspace,
            Pane::Blackboard,
            Pane::Chat,
            Pane::Composer,
        ];
        if self.blackboard_collapsed {
            all.into_iter()
                .filter(|pane| *pane != Pane::Blackboard)
                .collect()
        } else {
            all.to_vec()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppConfig {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Tick,
    Quit,
}

#[derive(Debug, Default)]
pub struct App {
    pub state: AppState,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }
}
