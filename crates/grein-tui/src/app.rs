#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pane {
    #[default]
    Session,
    Conversation,
    Blackboard,
    Composer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub tick_rate_ms: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { tick_rate_ms: 250 }
    }
}

impl AppConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Tick,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub focused_pane: Pane,
    pub blackboard_collapsed: bool,
    pub should_quit: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            focused_pane: Pane::Session,
            blackboard_collapsed: false,
            should_quit: false,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focusable_panes(&self) -> Vec<Pane> {
        let mut panes = vec![Pane::Session, Pane::Conversation, Pane::Composer];
        if !self.blackboard_collapsed {
            panes.insert(2, Pane::Blackboard);
        }
        panes
    }

    pub fn focus_next(&mut self) {
        let panes = self.focusable_panes();
        if panes.is_empty() {
            return;
        }
        if let Some(pos) = panes.iter().position(|&p| p == self.focused_pane) {
            let next_idx = (pos + 1) % panes.len();
            self.focused_pane = panes[next_idx];
        } else {
            self.focused_pane = panes[0];
        }
    }

    pub fn focus_prev(&mut self) {
        let panes = self.focusable_panes();
        if panes.is_empty() {
            return;
        }
        if let Some(pos) = panes.iter().position(|&p| p == self.focused_pane) {
            let prev_idx = if pos == 0 { panes.len() - 1 } else { pos - 1 };
            self.focused_pane = panes[prev_idx];
        } else {
            self.focused_pane = panes[panes.len() - 1];
        }
    }
}

#[derive(Debug, Default)]
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

    pub fn with_state(state: AppState) -> Self {
        Self {
            state,
            config: AppConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focusable_panes_includes_blackboard_when_not_collapsed() {
        let mut app = AppState::default();
        app.blackboard_collapsed = false;

        let panes = app.focusable_panes();
        assert!(panes.contains(&Pane::Blackboard));
        assert_eq!(panes.len(), 4);
    }

    #[test]
    fn test_focusable_panes_excludes_blackboard_when_collapsed() {
        let mut app = AppState::default();
        app.blackboard_collapsed = true;

        let panes = app.focusable_panes();
        assert!(!panes.contains(&Pane::Blackboard));
        assert_eq!(panes.len(), 3);
    }

    #[test]
    fn test_focus_forward_navigation_with_blackboard_expanded() {
        let mut app = AppState::default();
        app.blackboard_collapsed = false;
        let panes = app.focusable_panes();
        assert_eq!(panes.len(), 4);

        app.focused_pane = panes[0];
        for expected in panes.iter().cycle().skip(1).take(panes.len()) {
            app.focus_next();
            assert_eq!(app.focused_pane, *expected);
        }

        // Complete full cycle back to starting pane
        app.focus_next();
        assert_eq!(app.focused_pane, panes[1]);
    }

    #[test]
    fn test_focus_reverse_navigation_with_blackboard_expanded() {
        let mut app = AppState::default();
        app.blackboard_collapsed = false;
        let panes = app.focusable_panes();
        assert_eq!(panes.len(), 4);

        app.focused_pane = panes[0];
        for expected in panes.iter().rev().cycle().take(panes.len()) {
            app.focus_prev();
            assert_eq!(app.focused_pane, *expected);
        }
    }

    #[test]
    fn test_focus_forward_navigation_with_blackboard_collapsed() {
        let mut app = AppState::default();
        app.blackboard_collapsed = true;
        let panes = app.focusable_panes();
        assert!(!panes.contains(&Pane::Blackboard));
        assert_eq!(panes.len(), 3);

        app.focused_pane = panes[0];
        for expected in panes.iter().cycle().skip(1).take(panes.len() * 2) {
            app.focus_next();
            assert_eq!(app.focused_pane, *expected);
            assert_ne!(app.focused_pane, Pane::Blackboard);
        }
    }

    #[test]
    fn test_focus_reverse_navigation_with_blackboard_collapsed() {
        let mut app = AppState::default();
        app.blackboard_collapsed = true;
        let panes = app.focusable_panes();
        assert!(!panes.contains(&Pane::Blackboard));
        assert_eq!(panes.len(), 3);

        app.focused_pane = panes[0];
        for expected in panes.iter().rev().cycle().take(panes.len() * 2) {
            app.focus_prev();
            assert_eq!(app.focused_pane, *expected);
            assert_ne!(app.focused_pane, Pane::Blackboard);
        }
    }

    #[test]
    fn test_focus_navigation_recovers_if_blackboard_collapsed_while_focused() {
        let mut app = AppState::default();
        app.blackboard_collapsed = false;
        app.focused_pane = Pane::Blackboard;

        // Collapse blackboard while it currently holds active focus
        app.blackboard_collapsed = true;

        // Calling focus_next must transition away from Blackboard to a valid focusable pane
        app.focus_next();
        assert_ne!(app.focused_pane, Pane::Blackboard);
        assert!(app.focusable_panes().contains(&app.focused_pane));

        // Set back to Blackboard and test reverse navigation recovery as well
        app.focused_pane = Pane::Blackboard;
        app.focus_prev();
        assert_ne!(app.focused_pane, Pane::Blackboard);
        assert!(app.focusable_panes().contains(&app.focused_pane));
    }
}
