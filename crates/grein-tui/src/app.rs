use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Identifies the currently active pane in the 4-pane TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FocusPane {
    #[default]
    Tree,
    Transcript,
    Composer,
    Blackboard,
}

impl FocusPane {
    pub const ALL: [Self; 4] = [
        Self::Tree,
        Self::Transcript,
        Self::Composer,
        Self::Blackboard,
    ];

    /// Cycles focus forward: `Tree -> Transcript -> Composer -> Blackboard -> Tree`.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Tree => Self::Transcript,
            Self::Transcript => Self::Composer,
            Self::Composer => Self::Blackboard,
            Self::Blackboard => Self::Tree,
        }
    }

    /// Cycles focus backward: `Tree -> Blackboard -> Composer -> Transcript -> Tree`.
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Tree => Self::Blackboard,
            Self::Transcript => Self::Tree,
            Self::Composer => Self::Transcript,
            Self::Blackboard => Self::Composer,
        }
    }

    /// Alias for [`FocusPane::prev`].
    #[must_use]
    pub const fn previous(self) -> Self {
        self.prev()
    }
}

/// Configuration options for the TUI application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub tick_rate_ms: u64,
    pub title: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            tick_rate_ms: 250,
            title: String::from("Grein"),
        }
    }
}

/// Application-level events processed by the TUI state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    Resize(u16, u16),
    Quit,
}

/// Snapshot state of the TUI application.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppState {
    pub focused_pane: FocusPane,
    pub composer_input: String,
    pub should_quit: bool,
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Primary TUI application controller and state holder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct App {
    config: AppConfig,
    state: AppState,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_config(config: AppConfig) -> Self {
        Self {
            config,
            state: AppState::default(),
        }
    }

    #[must_use]
    pub const fn config(&self) -> &AppConfig {
        &self.config
    }

    #[must_use]
    pub const fn state(&self) -> &AppState {
        &self.state
    }

    #[must_use]
    pub const fn focused_pane(&self) -> FocusPane {
        self.state.focused_pane
    }

    pub fn set_focused_pane(&mut self, pane: FocusPane) {
        self.state.focused_pane = pane;
    }

    pub fn focus_next(&mut self) {
        self.state.focused_pane = self.state.focused_pane.next();
    }

    pub fn focus_prev(&mut self) {
        self.state.focused_pane = self.state.focused_pane.prev();
    }

    pub fn focus_previous(&mut self) {
        self.focus_prev();
    }

    #[must_use]
    pub fn composer_input(&self) -> &str {
        &self.state.composer_input
    }

    #[must_use]
    pub const fn should_quit(&self) -> bool {
        self.state.should_quit
    }

    pub fn quit(&mut self) {
        self.state.should_quit = true;
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key_event) => self.handle_key_event(key_event),
            AppEvent::Tick | AppEvent::Resize(_, _) => {}
            AppEvent::Quit => self.quit(),
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_prev();
            }
            KeyCode::Tab => {
                self.focus_next();
            }
            KeyCode::BackTab => {
                self.focus_prev();
            }
            KeyCode::Char(ch)
                if self.state.focused_pane == FocusPane::Composer
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.state.composer_input.push(ch);
            }
            KeyCode::Backspace if self.state.focused_pane == FocusPane::Composer => {
                self.state.composer_input.pop();
            }
            _ => {}
        }
    }
}
