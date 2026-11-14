use crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    Sidebar,
    Transcript,
    Composer,
    Inspector,
}

impl FocusPane {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::Transcript,
            Self::Transcript => Self::Composer,
            Self::Composer => Self::Inspector,
            Self::Inspector => Self::Sidebar,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Sidebar => Self::Inspector,
            Self::Transcript => Self::Sidebar,
            Self::Composer => Self::Transcript,
            Self::Inspector => Self::Composer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    focused_pane: FocusPane,
    composer_input: String,
    running: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            focused_pane: FocusPane::Sidebar,
            composer_input: String::new(),
            running: true,
        }
    }

    #[must_use]
    pub const fn focused_pane(&self) -> FocusPane {
        self.focused_pane
    }

    pub fn set_focused_pane(&mut self, pane: FocusPane) {
        self.focused_pane = pane;
    }

    #[must_use]
    pub fn composer_input(&self) -> &str {
        &self.composer_input
    }

    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    pub fn focus_prev(&mut self) {
        self.focused_pane = self.focused_pane.prev();
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Tab => self.focus_next(),
            KeyCode::BackTab => self.focus_prev(),
            KeyCode::Char(ch) if self.focused_pane == FocusPane::Composer => {
                self.composer_input.push(ch);
            }
            KeyCode::Backspace if self.focused_pane == FocusPane::Composer => {
                self.composer_input.pop();
            }
            KeyCode::Char('q') => {
                self.quit();
            }
            _ => {}
        }
    }
}
