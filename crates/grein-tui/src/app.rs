//! Application state and navigation state machine.

/// Identifies focusable panes in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pane {
    #[default]
    Workspace,
    Blackboard,
    Chat,
    Composer,
}

/// Application runtime state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppState {
    pub focus: Pane,
    pub blackboard_collapsed: bool,
}

impl AppState {
    /// Returns the currently focusable panes in logical tab order.
    pub fn focusable_panes(&self) -> Vec<Pane> {
        if self.blackboard_collapsed {
            vec![Pane::Workspace, Pane::Chat, Pane::Composer]
        } else {
            vec![
                Pane::Workspace,
                Pane::Blackboard,
                Pane::Chat,
                Pane::Composer,
            ]
        }
    }

    /// Cycles focus to the next available pane, skipping collapsed panes.
    pub fn focus_next(&mut self) {
        let focusable = self.focusable_panes();
        if focusable.is_empty() {
            return;
        }

        const ALL_PANES: [Pane; 4] = [
            Pane::Workspace,
            Pane::Blackboard,
            Pane::Chat,
            Pane::Composer,
        ];

        let current_idx = ALL_PANES.iter().position(|&p| p == self.focus).unwrap_or(0);

        let total = ALL_PANES.len();
        for step in 1..=total {
            let candidate = ALL_PANES[(current_idx + step) % total];
            if focusable.contains(&candidate) {
                self.focus = candidate;
                return;
            }
        }
    }

    /// Cycles focus to the previous available pane, skipping collapsed panes.
    pub fn focus_prev(&mut self) {
        let focusable = self.focusable_panes();
        if focusable.is_empty() {
            return;
        }

        const ALL_PANES: [Pane; 4] = [
            Pane::Workspace,
            Pane::Blackboard,
            Pane::Chat,
            Pane::Composer,
        ];

        let current_idx = ALL_PANES.iter().position(|&p| p == self.focus).unwrap_or(0);

        let total = ALL_PANES.len();
        for step in 1..=total {
            let candidate = ALL_PANES[(current_idx + total - (step % total)) % total];
            if focusable.contains(&candidate) {
                self.focus = candidate;
                return;
            }
        }
    }
}

/// Configuration for the TUI application.
#[derive(Debug, Clone, Default)]
pub struct AppConfig {}

/// Events processed by the TUI application.
#[derive(Debug, Clone)]
pub enum AppEvent {}

/// Top-level application container.
pub struct App {
    pub state: AppState,
    pub config: AppConfig,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        Self {
            state: AppState::default(),
            config,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new(AppConfig::default())
    }
}
