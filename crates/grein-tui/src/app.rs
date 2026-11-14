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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum FocusedPane {
    #[default]
    Tree,
    Transcript,
    Composer,
    Blackboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    Scrolled { delta: i32 },
    None,
}

pub trait TuiComponent {
    fn is_focused(&self) -> bool;
    fn handle_event(&mut self, event: &AppEvent) -> bool;
    fn dispatch_key(&mut self, event: &AppEvent) -> Option<AppAction>;
}

#[allow(non_upper_case_globals)]
impl FocusedPane {
    pub const Sidebar: Self = Self::Tree;
    pub const Inspector: Self = Self::Blackboard;
    pub const Stream: Self = Self::Transcript;
    pub const Chat: Self = Self::Transcript;
    pub const Governance: Self = Self::Blackboard;
    pub const Details: Self = Self::Blackboard;
    pub const Sessions: Self = Self::Tree;
    pub const Diff: Self = Self::Blackboard;
    pub const Workspace: Self = Self::Tree;
    pub const Output: Self = Self::Transcript;

    pub fn next(&self, direction: FocusDirection) -> Self {
        match direction {
            FocusDirection::Forward => match self {
                Self::Tree => Self::Transcript,
                Self::Transcript => Self::Composer,
                Self::Composer => Self::Blackboard,
                Self::Blackboard => Self::Tree,
            },
            FocusDirection::Backward => match self {
                Self::Tree => Self::Blackboard,
                Self::Blackboard => Self::Composer,
                Self::Composer => Self::Transcript,
                Self::Transcript => Self::Tree,
            },
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::Tree => Self::Blackboard,
            Self::Blackboard => Self::Composer,
            Self::Composer => Self::Transcript,
            Self::Transcript => Self::Tree,
        }
    }
}

impl From<crate::focus::PaneFocus> for FocusedPane {
    fn from(p: crate::focus::PaneFocus) -> Self {
        match p {
            crate::focus::PaneFocus::Composer => Self::Composer,
            crate::focus::PaneFocus::Sidebar | crate::focus::PaneFocus::Sessions => Self::Tree,
            crate::focus::PaneFocus::Transcript | crate::focus::PaneFocus::Chat => Self::Transcript,
            crate::focus::PaneFocus::Inspector
            | crate::focus::PaneFocus::Diff
            | crate::focus::PaneFocus::Details => Self::Blackboard,
        }
    }
}

impl PartialEq<crate::focus::PaneFocus> for FocusedPane {
    fn eq(&self, other: &crate::focus::PaneFocus) -> bool {
        let other_fp: FocusedPane = (*other).into();
        *self == other_fp
    }
}

impl PartialEq<FocusedPane> for crate::focus::PaneFocus {
    fn eq(&self, other: &FocusedPane) -> bool {
        let self_fp: FocusedPane = (*self).into();
        self_fp == *other
    }
}

pub type FocusPane = FocusedPane;
pub type PaneFocus = FocusedPane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAction {
    ActivateAnchor(String),
}

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

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub poll_interval: std::time::Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    Quit,
    FocusChanged(FocusedPane),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub focused_pane: FocusedPane,
    pub focus: FocusedPane,
    pub active_focus: FocusedPane,
    pub blackboard_collapsed: bool,
    pub blackboard_notes: String,
    pub input_buffer: String,
    pub cursor_position: usize,
    pub should_quit: bool,
    pub active_transcript_md: String,
    pub threads: Vec<ThreadItem>,
    pub tree: crate::ui::tree::TreeState,
    pub blackboard_pane: crate::panes::blackboard::BlackboardPane,
    pub last_submitted_prompt: Option<String>,
}

pub type AppState = App;

impl Default for App {
    fn default() -> Self {
        Self {
            focused_pane: FocusedPane::Tree,
            focus: FocusedPane::Tree,
            active_focus: FocusedPane::Tree,
            blackboard_collapsed: false,
            blackboard_notes: String::new(),
            input_buffer: String::new(),
            cursor_position: 0,
            should_quit: false,
            active_transcript_md: String::new(),
            threads: Vec::new(),
            tree: crate::ui::tree::TreeState::default(),
            blackboard_pane: crate::panes::blackboard::BlackboardPane::new(),
            last_submitted_prompt: None,
        }
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(_config: AppConfig) -> Self {
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
    }

    fn sync_focus_out(&mut self) {
        self.focus = self.focused_pane;
        self.active_focus = self.focused_pane;
    }

    pub fn focused_pane(&self) -> FocusedPane {
        self.focused_pane
    }

    pub fn focus(&self) -> FocusedPane {
        self.focus
    }

    pub fn active_focus(&self) -> FocusedPane {
        self.active_focus
    }

    pub fn set_focus(&mut self, pane: impl Into<FocusedPane>) {
        self.focused_pane = pane.into();
        self.sync_focus_out();
    }

    pub fn set_focused_pane(&mut self, pane: impl Into<FocusedPane>) {
        self.set_focus(pane);
    }

    pub fn is_running(&self) -> bool {
        !self.should_quit
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn composer_input(&self) -> &str {
        &self.input_buffer
    }

    pub fn composer_content(&self) -> &str {
        &self.input_buffer
    }

    pub fn composer_line_count(&self) -> u16 {
        self.input_buffer.split('\n').count().max(1) as u16
    }

    pub fn composer_cursor_2d(&self) -> (u16, u16) {
        let clamped_pos = self.cursor_position.min(self.input_buffer.len());
        let prefix = &self.input_buffer[..clamped_pos];
        let row = prefix.bytes().filter(|&b| b == b'\n').count() as u16;
        let col = match prefix.rfind('\n') {
            Some(idx) => prefix[idx + 1..].chars().count() as u16,
            None => prefix.chars().count() as u16,
        };
        (col, row)
    }

    pub fn last_submitted_prompt(&self) -> Option<String> {
        self.last_submitted_prompt.clone()
    }

    pub fn poll_events(&mut self, _d: std::time::Duration) -> Result<(), TuiError> {
        Ok(())
    }

    pub fn is_blackboard_collapsed(&self) -> bool {
        self.blackboard_collapsed || self.blackboard_pane.is_collapsed()
    }

    pub fn set_blackboard_collapsed(&mut self, collapsed: bool) {
        self.blackboard_collapsed = collapsed;
        if collapsed {
            self.blackboard_pane.collapse();
            if self.focused_pane == FocusedPane::Blackboard {
                self.focused_pane = FocusedPane::Tree;
            }
        } else {
            self.blackboard_pane.expand();
        }
        self.sync_focus_out();
    }

    pub fn blackboard_pane(&self) -> &crate::panes::blackboard::BlackboardPane {
        &self.blackboard_pane
    }

    pub fn blackboard_pane_mut(&mut self) -> &mut crate::panes::blackboard::BlackboardPane {
        &mut self.blackboard_pane
    }

    pub fn cycle_focus(&mut self) {
        self.focus_next();
    }

    pub fn cycle_focus_reverse(&mut self) {
        self.focus_prev();
    }

    pub fn focus_next(&mut self) {
        self.sync_focus_in();
        let is_collapsed = self.blackboard_collapsed || self.blackboard_pane.is_collapsed();
        self.focused_pane = match self.focused_pane {
            FocusedPane::Tree => FocusedPane::Transcript,
            FocusedPane::Transcript => FocusedPane::Composer,
            FocusedPane::Composer => {
                if is_collapsed {
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
        let is_collapsed = self.blackboard_collapsed || self.blackboard_pane.is_collapsed();
        self.focused_pane = match self.focused_pane {
            FocusedPane::Tree => {
                if is_collapsed {
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
        let collapsed = !self.is_blackboard_collapsed();
        self.set_blackboard_collapsed(collapsed);
    }

    pub fn handle_event(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Key(k) => self.handle_key_event(k),
            AppEvent::Tick => false,
            AppEvent::Quit => {
                self.should_quit = true;
                true
            }
            AppEvent::FocusChanged(pane) => {
                self.set_focus(pane);
                true
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.handle_key_event(key)
    }

    pub fn handle_key_tree(&mut self, key: KeyEvent) -> Option<TreeAction> {
        self.sync_focus_in();
        if self.focused_pane != FocusedPane::Tree {
            return None;
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.tree.nodes.is_empty() {
                    let curr = self.tree.selected_index().unwrap_or(0);
                    if curr + 1 < self.tree.nodes.len() {
                        self.tree.select_index(curr + 1);
                    }
                }
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.tree.nodes.is_empty() {
                    let curr = self.tree.selected_index().unwrap_or(0);
                    if curr > 0 {
                        self.tree.select_index(curr - 1);
                    }
                }
                None
            }
            KeyCode::Enter => {
                if let Some(idx) = self.tree.selected_index() {
                    if let Some(crate::ui::tree::TreeNode::Anchor(anchor)) =
                        self.tree.nodes.get(idx)
                    {
                        let id = anchor.id.clone();
                        self.tree.set_active_anchor_id(Some(id.clone()));
                        return Some(TreeAction::ActivateAnchor(id));
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        self.sync_focus_in();

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('b') => {
                    self.toggle_blackboard();
                    return true;
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
                return true;
            }
            KeyCode::BackTab => {
                self.focus_prev();
                return true;
            }
            KeyCode::Char('[') if self.focused_pane != FocusedPane::Composer => {
                self.set_blackboard_collapsed(true);
                return true;
            }
            KeyCode::Char(']') if self.focused_pane != FocusedPane::Composer => {
                self.set_blackboard_collapsed(false);
                return true;
            }
            KeyCode::Esc => {
                if self.focused_pane == FocusedPane::Composer {
                    self.focused_pane = FocusedPane::Tree;
                    self.sync_focus_out();
                    return true;
                }
            }
            KeyCode::Enter if self.focused_pane == FocusedPane::Composer => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    self.input_buffer.insert(self.cursor_position, '\n');
                    self.cursor_position += 1;
                    self.sync_focus_out();
                    return true;
                }
                let prompt = std::mem::take(&mut self.input_buffer);
                self.cursor_position = 0;
                self.last_submitted_prompt = Some(prompt.clone());
                self.submit_composer_prompt(prompt);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Char('j') | KeyCode::Char('J')
                if self.focused_pane == FocusedPane::Composer
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.input_buffer.insert(self.cursor_position, '\n');
                self.cursor_position += 1;
                self.sync_focus_out();
                return true;
            }
            KeyCode::Char('q') if self.focused_pane != FocusedPane::Composer => {
                self.should_quit = true;
                self.sync_focus_out();
                return true;
            }
            KeyCode::Char(c) if self.focused_pane == FocusedPane::Composer => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    self.input_buffer.insert(self.cursor_position, c);
                    self.cursor_position += c.len_utf8();
                    self.sync_focus_out();
                    return true;
                }
            }
            KeyCode::Backspace
                if self.focused_pane == FocusedPane::Composer && self.cursor_position > 0 =>
            {
                if let Some((idx, _)) = self.input_buffer[..self.cursor_position]
                    .char_indices()
                    .next_back()
                {
                    self.input_buffer.remove(idx);
                    self.cursor_position = idx;
                    self.sync_focus_out();
                    return true;
                }
            }
            KeyCode::Delete
                if self.focused_pane == FocusedPane::Composer
                    && self.cursor_position < self.input_buffer.len() =>
            {
                self.input_buffer.remove(self.cursor_position);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Left
                if self.focused_pane == FocusedPane::Composer && self.cursor_position > 0 =>
            {
                if let Some((idx, _)) = self.input_buffer[..self.cursor_position]
                    .char_indices()
                    .next_back()
                {
                    self.cursor_position = idx;
                    return true;
                }
            }
            KeyCode::Right
                if self.focused_pane == FocusedPane::Composer
                    && self.cursor_position < self.input_buffer.len() =>
            {
                if let Some(ch) = self.input_buffer[self.cursor_position..].chars().next() {
                    self.cursor_position += ch.len_utf8();
                    return true;
                }
            }
            KeyCode::Home if self.focused_pane == FocusedPane::Composer => {
                self.cursor_position = 0;
                return true;
            }
            KeyCode::End if self.focused_pane == FocusedPane::Composer => {
                self.cursor_position = self.input_buffer.len();
                return true;
            }
            _ => {
                if self.focused_pane == FocusedPane::Tree {
                    let _ = self.handle_key_tree(key);
                    self.sync_focus_out();
                    return true;
                }
                if self.focused_pane == FocusedPane::Blackboard {
                    return self.blackboard_pane.handle_key(key);
                }
            }
        }

        self.sync_focus_out();
        false
    }

    pub fn render(&mut self, _frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        if self.is_blackboard_collapsed() {
            self.blackboard_pane
                .set_rendered_rect(ratatui::layout::Rect::default());
        } else {
            let layout = crate::layout::compute_layout(area, false);
            if let Some(r) = layout.blackboard_area {
                self.blackboard_pane.set_rendered_rect(r);
            } else {
                self.blackboard_pane.set_rendered_rect(area);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_focus_is_tree() {
        let app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        assert_eq!(app.focus, FocusedPane::Tree);
    }

    #[test]
    fn test_tab_cycles_focus_forward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Blackboard);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
    }

    #[test]
    fn test_backtab_cycles_focus_backward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Blackboard);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
    }

    #[test]
    fn test_shift_tab_cycles_focus_backward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane(), FocusedPane::Blackboard);
    }

    #[test]
    fn test_collapsed_blackboard_skips_focus() {
        let mut app = App::default();
        app.set_blackboard_collapsed(true);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
    }

    #[test]
    fn test_ctrl_b_toggles_blackboard_collapse() {
        let mut app = App::default();
        assert!(!app.is_blackboard_collapsed());

        app.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(app.is_blackboard_collapsed());

        app.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(!app.is_blackboard_collapsed());
    }

    #[test]
    fn test_composer_typing_immunity_for_q_key() {
        let mut app = App::new();
        app.set_focus(FocusPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        assert!(!app.should_quit());
        assert_eq!(app.composer_content(), "q");
    }

    #[test]
    fn test_composer_typing_immunity_for_bracket_keys() {
        let mut app = App::new();
        app.set_focus(FocusPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));

        assert!(!app.is_blackboard_collapsed());
        assert_eq!(app.composer_content(), "[]");
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
        assert_eq!(app.composer_input(), "");

        // Focus Composer
        app.focused_pane = FocusedPane::Composer;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "hi");
        assert_eq!(app.cursor_position, 2);

        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "h");
        assert_eq!(app.cursor_position, 1);
    }
}
