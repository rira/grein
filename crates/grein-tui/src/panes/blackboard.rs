use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlackboardState {
    #[default]
    Expanded,
    Collapsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackboardAction {
    ToggleCollapse,
    Collapse,
    Expand,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlackboardPane {
    state: BlackboardState,
    rendered_rect: Rect,
}

impl BlackboardPane {
    pub fn new() -> Self {
        Self {
            state: BlackboardState::Expanded,
            rendered_rect: Rect::default(),
        }
    }

    pub fn state(&self) -> BlackboardState {
        self.state
    }

    pub fn is_collapsed(&self) -> bool {
        self.state == BlackboardState::Collapsed
    }

    pub fn collapse(&mut self) {
        self.state = BlackboardState::Collapsed;
    }

    pub fn expand(&mut self) {
        self.state = BlackboardState::Expanded;
    }

    pub fn handle_action(&mut self, action: BlackboardAction) {
        match action {
            BlackboardAction::ToggleCollapse => {
                if self.is_collapsed() {
                    self.expand();
                } else {
                    self.collapse();
                }
            }
            BlackboardAction::Collapse => {
                self.collapse();
            }
            BlackboardAction::Expand => {
                self.expand();
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.is_collapsed() {
            return false;
        }
        matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
        )
    }

    pub fn rendered_rect(&self) -> Rect {
        self.rendered_rect
    }

    pub fn set_rendered_rect(&mut self, rect: Rect) {
        self.rendered_rect = rect;
    }

    pub fn render(&mut self, _frame: &mut Frame, area: Rect) {
        self.rendered_rect = area;
    }
}
