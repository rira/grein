use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::layout::PaneFocus;
use crate::panes::blackboard::{BlackboardAction, BlackboardPane};

#[derive(Debug, Clone, Default)]
pub struct AppConfig {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
}

#[derive(Debug, Clone, Default)]
pub struct AppState {}

#[derive(Debug)]
pub struct App {
    config: AppConfig,
    state: AppState,
    focused: PaneFocus,
    blackboard: BlackboardPane,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            state: AppState::default(),
            focused: PaneFocus::Workspace,
            blackboard: BlackboardPane::new(),
        }
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn focused_pane(&self) -> PaneFocus {
        self.focused
    }

    pub fn set_focus(&mut self, focus: PaneFocus) {
        self.focused = focus;
    }

    pub fn blackboard_pane(&self) -> &BlackboardPane {
        &self.blackboard
    }

    pub fn blackboard_pane_mut(&mut self) -> &mut BlackboardPane {
        &mut self.blackboard
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if self.focused == PaneFocus::Blackboard {
            self.blackboard.handle_key(key)
        } else {
            false
        }
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('b') {
                    self.blackboard
                        .handle_action(BlackboardAction::ToggleCollapse);
                    if self.blackboard.is_collapsed() && self.focused == PaneFocus::Blackboard {
                        self.focused = PaneFocus::Composer;
                    }
                    return;
                }

                if key.code == KeyCode::Tab && !key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.cycle_focus_forward();
                    return;
                }

                if key.code == KeyCode::BackTab
                    || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
                {
                    self.cycle_focus_backward();
                    return;
                }

                self.handle_key_event(key);
            }
        }
    }

    fn active_panes(&self) -> Vec<PaneFocus> {
        if self.blackboard.is_collapsed() {
            vec![PaneFocus::Workspace, PaneFocus::Composer]
        } else {
            vec![
                PaneFocus::Workspace,
                PaneFocus::Composer,
                PaneFocus::Blackboard,
            ]
        }
    }

    fn cycle_focus_forward(&mut self) {
        let panes = self.active_panes();
        let idx = panes.iter().position(|&p| p == self.focused).unwrap_or(0);
        let next_idx = (idx + 1) % panes.len();
        self.focused = panes[next_idx];
    }

    fn cycle_focus_backward(&mut self) {
        let panes = self.active_panes();
        let idx = panes.iter().position(|&p| p == self.focused).unwrap_or(0);
        let prev_idx = if idx == 0 { panes.len() - 1 } else { idx - 1 };
        self.focused = panes[prev_idx];
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if self.blackboard.is_collapsed() {
            self.blackboard.set_rendered_rect(Rect::new(
                area.x + area.width,
                area.y,
                0,
                area.height,
            ));
        } else {
            let width = if area.width > 30 { 30 } else { area.width / 2 };
            let x = area.x + area.width.saturating_sub(width);
            let blackboard_area = Rect::new(x, area.y, width, area.height);
            self.blackboard.render(frame, blackboard_area);
        }
    }
}
