use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TuiError {
    #[error("Terminal I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for TuiError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedPane {
    #[default]
    Tree,
    Transcript,
    Composer,
    Blackboard,
}

pub type FocusPane = FocusedPane;
pub type PaneFocus = FocusedPane;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnchorItem {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadItem {
    pub id: String,
    pub title: String,
    pub anchors: Vec<AnchorItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct App {
    pub focused_pane: FocusedPane,
    pub focus: FocusedPane,
    pub active_focus: FocusedPane,
    pub blackboard_collapsed: bool,
    pub blackboard_notes: String,
    pub composer_input: String,
    pub input_buffer: String,
    pub cursor_position: usize,
    pub should_quit: bool,
    pub active_transcript_md: String,
    pub threads: Vec<ThreadItem>,
}

pub type AppState = App;

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    fn sync_focus_in(&mut self) {
        if self.focus != self.focused_pane {
            if self.focus != FocusedPane::Tree {
                self.focused_pane = self.focus;
            } else {
                self.focus = self.focused_pane;
            }
        }
        if self.active_focus != self.focused_pane {
            if self.active_focus != FocusedPane::Tree {
                self.focused_pane = self.active_focus;
            } else {
                self.active_focus = self.focused_pane;
            }
        }
        if self.input_buffer != self.composer_input {
            if !self.input_buffer.is_empty() && self.composer_input.is_empty() {
                self.composer_input = self.input_buffer.clone();
                self.cursor_position = self.composer_input.len();
            } else {
                self.input_buffer = self.composer_input.clone();
            }
        }
    }

    fn sync_focus_out(&mut self) {
        self.focus = self.focused_pane;
        self.active_focus = self.focused_pane;
        self.input_buffer = self.composer_input.clone();
    }

    pub fn focused_pane(&self) -> FocusedPane {
        self.focused_pane
    }

    pub fn active_focus(&self) -> FocusedPane {
        self.focused_pane
    }

    pub fn set_focus(&mut self, pane: FocusedPane) {
        self.focused_pane = pane;
        self.sync_focus_out();
    }

    pub fn composer_content(&self) -> &str {
        &self.composer_input
    }

    pub fn is_blackboard_collapsed(&self) -> bool {
        self.blackboard_collapsed
    }

    pub fn set_blackboard_collapsed(&mut self, collapsed: bool) {
        self.blackboard_collapsed = collapsed;
        if self.blackboard_collapsed && self.focused_pane == FocusedPane::Blackboard {
            self.focused_pane = FocusedPane::Tree;
        }
        self.sync_focus_out();
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn cycle_focus(&mut self) {
        self.focus_next();
    }

    pub fn cycle_focus_reverse(&mut self) {
        self.focus_prev();
    }

    pub fn focus_next(&mut self) {
        self.sync_focus_in();
        self.focused_pane = match self.focused_pane {
            FocusedPane::Tree => FocusedPane::Transcript,
            FocusedPane::Transcript => FocusedPane::Composer,
            FocusedPane::Composer => {
                if self.blackboard_collapsed {
                    FocusedPane::Tree
                } else {
                    FocusedPane::Blackboard
                }
            }
            FocusedPane::Blackboard => FocusedPane::Tree,
        };
        self.sync_focus_out();
    }

    pub fn next_pane(&mut self) {
        self.focus_next();
    }

    pub fn focus_prev(&mut self) {
        self.sync_focus_in();
        self.focused_pane = match self.focused_pane {
            FocusedPane::Tree => {
                if self.blackboard_collapsed {
                    FocusedPane::Composer
                } else {
                    FocusedPane::Blackboard
                }
            }
            FocusedPane::Blackboard => FocusedPane::Composer,
            FocusedPane::Composer => FocusedPane::Transcript,
            FocusedPane::Transcript => FocusedPane::Tree,
        };
        self.sync_focus_out();
    }

    pub fn prev_pane(&mut self) {
        self.focus_prev();
    }

    pub fn focus_previous(&mut self) {
        self.focus_prev();
    }

    pub fn toggle_blackboard(&mut self) {
        self.sync_focus_in();
        self.blackboard_collapsed = !self.blackboard_collapsed;
        if self.blackboard_collapsed && self.focused_pane == FocusedPane::Blackboard {
            self.focused_pane = FocusedPane::Tree;
        }
        self.sync_focus_out();
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        self.sync_focus_in();

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    self.sync_focus_out();
                    return;
                }
                KeyCode::Char('b') => {
                    self.toggle_blackboard();
                    return;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.focus_prev();
                } else {
                    self.focus_next();
                }
            }
            KeyCode::BackTab => {
                self.focus_prev();
            }
            KeyCode::Esc => {
                if self.focused_pane == FocusedPane::Composer {
                    self.focused_pane = FocusedPane::Tree;
                }
            }
            KeyCode::Char('q') if self.focused_pane != FocusedPane::Composer => {
                self.should_quit = true;
            }
            KeyCode::Char(c) if self.focused_pane == FocusedPane::Composer => {
                if !key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer_input.insert(self.cursor_position, c);
                    self.cursor_position += c.len_utf8();
                }
            }
            KeyCode::Backspace
                if self.focused_pane == FocusedPane::Composer && self.cursor_position > 0 =>
            {
                if let Some((idx, _)) = self.composer_input[..self.cursor_position]
                    .char_indices()
                    .next_back()
                {
                    self.composer_input.remove(idx);
                    self.cursor_position = idx;
                }
            }
            KeyCode::Delete
                if self.focused_pane == FocusedPane::Composer
                    && self.cursor_position < self.composer_input.len() =>
            {
                self.composer_input.remove(self.cursor_position);
            }
            KeyCode::Left
                if self.focused_pane == FocusedPane::Composer && self.cursor_position > 0 =>
            {
                if let Some((idx, _)) = self.composer_input[..self.cursor_position]
                    .char_indices()
                    .next_back()
                {
                    self.cursor_position = idx;
                }
            }
            KeyCode::Right
                if self.focused_pane == FocusedPane::Composer
                    && self.cursor_position < self.composer_input.len() =>
            {
                if let Some(ch) = self.composer_input[self.cursor_position..].chars().next() {
                    self.cursor_position += ch.len_utf8();
                }
            }
            KeyCode::Home if self.focused_pane == FocusedPane::Composer => {
                self.cursor_position = 0;
            }
            KeyCode::End if self.focused_pane == FocusedPane::Composer => {
                self.cursor_position = self.composer_input.len();
            }
            _ => {}
        }

        self.sync_focus_out();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_focus_is_tree() {
        let app = App::default();
        assert_eq!(app.focused_pane, FocusedPane::Tree);
        assert!(!app.blackboard_collapsed);
    }

    #[test]
    fn test_four_pane_focus_cycle_forward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane, FocusedPane::Tree);

        app.focus_next();
        assert_eq!(app.focused_pane, FocusedPane::Transcript);

        app.focus_next();
        assert_eq!(app.focused_pane, FocusedPane::Composer);

        app.focus_next();
        assert_eq!(app.focused_pane, FocusedPane::Blackboard);

        app.focus_next();
        assert_eq!(app.focused_pane, FocusedPane::Tree);
    }

    #[test]
    fn test_four_pane_focus_cycle_backward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane, FocusedPane::Tree);

        app.focus_prev();
        assert_eq!(app.focused_pane, FocusedPane::Blackboard);

        app.focus_prev();
        assert_eq!(app.focused_pane, FocusedPane::Composer);

        app.focus_prev();
        assert_eq!(app.focused_pane, FocusedPane::Transcript);

        app.focus_prev();
        assert_eq!(app.focused_pane, FocusedPane::Tree);
    }

    #[test]
    fn test_collapsed_blackboard_skips_focus() {
        let mut app = App::default();
        app.blackboard_collapsed = true;
        assert_eq!(app.focused_pane, FocusedPane::Tree);

        let tab_event = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        app.handle_key_event(tab_event);
        assert_eq!(app.focused_pane, FocusedPane::Transcript);

        app.handle_key_event(tab_event);
        assert_eq!(app.focused_pane, FocusedPane::Composer);

        app.handle_key_event(tab_event);
        assert_eq!(app.focused_pane, FocusedPane::Tree);

        // Reverse cycle: Tree -> Composer -> Transcript -> Tree (skips Blackboard)
        let backtab_event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        app.handle_key_event(backtab_event);
        assert_eq!(app.focused_pane, FocusedPane::Composer);

        app.handle_key_event(backtab_event);
        assert_eq!(app.focused_pane, FocusedPane::Transcript);

        app.handle_key_event(backtab_event);
        assert_eq!(app.focused_pane, FocusedPane::Tree);
    }

    #[test]
    fn test_toggle_blackboard_moves_focus_when_focused() {
        let mut app = App::default();
        app.focused_pane = FocusedPane::Blackboard;

        app.toggle_blackboard();
        assert!(app.blackboard_collapsed);
        assert_eq!(app.focused_pane, FocusedPane::Tree);
    }

    #[test]
    fn test_ctrl_b_toggles_blackboard_and_resets_focus() {
        let mut app = App::default();
        app.focused_pane = FocusedPane::Blackboard;

        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        app.handle_key_event(ctrl_b);

        assert!(app.blackboard_collapsed);
        assert_eq!(app.focused_pane, FocusedPane::Tree);

        // Toggle again to expand
        app.handle_key_event(ctrl_b);
        assert!(!app.blackboard_collapsed);
    }

    #[test]
    fn test_tab_and_backtab_key_events() {
        let mut app = App::default();
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane, FocusedPane::Transcript);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane, FocusedPane::Tree);
    }

    #[test]
    fn test_composer_esc_returns_to_tree_focus() {
        let mut app = App::new();
        app.focus = FocusPane::Composer;

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.focus, FocusPane::Tree);
    }

    #[test]
    fn test_composer_input_routing() {
        let mut app = App::default();

        // Input ignored when not in Composer
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.composer_input, "");

        // Focus Composer
        app.focused_pane = FocusedPane::Composer;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(app.composer_input, "hi");
        assert_eq!(app.cursor_position, 2);

        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.composer_input, "h");
        assert_eq!(app.cursor_position, 1);
    }
}
