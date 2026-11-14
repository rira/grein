use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Represents the four primary focusable panes in the TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedPane {
    #[default]
    Sessions,
    Stream,
    Inspector,
    Composer,
}

impl FocusedPane {
    /// Advances focus to the next pane in forward order:
    /// Sessions -> Stream -> Inspector -> Composer -> Sessions.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Sessions => Self::Stream,
            Self::Stream => Self::Inspector,
            Self::Inspector => Self::Composer,
            Self::Composer => Self::Sessions,
        }
    }

    /// Moves focus to the previous pane in reverse order:
    /// Sessions -> Composer -> Inspector -> Stream -> Sessions.
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Sessions => Self::Composer,
            Self::Composer => Self::Inspector,
            Self::Inspector => Self::Stream,
            Self::Stream => Self::Sessions,
        }
    }
}

/// Top-level application state for the Grein TUI.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct App {
    focused_pane: FocusedPane,
    composer_input: String,
    should_quit: bool,
}

impl App {
    /// Creates a new `App` instance with default initial state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the currently focused pane.
    #[must_use]
    pub const fn focused_pane(&self) -> FocusedPane {
        self.focused_pane
    }

    /// Returns the current text buffer in the composer pane.
    #[must_use]
    pub fn composer_input(&self) -> &str {
        &self.composer_input
    }

    /// Returns `true` if the application has been requested to quit.
    #[must_use]
    pub const fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Cycles focus forward to the next pane.
    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    /// Cycles focus backward to the previous pane.
    pub fn focus_prev(&mut self) {
        self.focused_pane = self.focused_pane.prev();
    }

    /// Handles an incoming terminal key event.
    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::BackTab => {
                self.focus_prev();
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_prev();
            }
            KeyCode::Tab => {
                self.focus_next();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char(ch) if self.focused_pane == FocusedPane::Composer => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    self.composer_input.push(ch);
                }
            }
            KeyCode::Backspace if self.focused_pane == FocusedPane::Composer => {
                self.composer_input.pop();
            }
            KeyCode::Char('q') | KeyCode::Esc if self.focused_pane != FocusedPane::Composer => {
                self.should_quit = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_initial_focus_is_sessions() {
        let app = App::new();
        assert_eq!(app.focused_pane(), FocusedPane::Sessions);
        assert_eq!(app.composer_input(), "");
        assert!(!app.should_quit());
    }

    #[test]
    fn test_tab_cycles_focus_forward() {
        let mut app = App::new();
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        app.handle_key_event(tab);
        assert_eq!(app.focused_pane(), FocusedPane::Stream);

        app.handle_key_event(tab);
        assert_eq!(app.focused_pane(), FocusedPane::Inspector);

        app.handle_key_event(tab);
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(tab);
        assert_eq!(app.focused_pane(), FocusedPane::Sessions);
    }

    #[test]
    fn test_backtab_and_shift_tab_cycles_focus_backward() {
        let mut app = App::new();
        assert_eq!(app.focused_pane(), FocusedPane::Sessions);

        // Reverse cycle via KeyCode::BackTab
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);

        app.handle_key_event(backtab);
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(backtab);
        assert_eq!(app.focused_pane(), FocusedPane::Inspector);

        app.handle_key_event(backtab);
        assert_eq!(app.focused_pane(), FocusedPane::Stream);

        app.handle_key_event(backtab);
        assert_eq!(app.focused_pane(), FocusedPane::Sessions);

        // Reverse cycle via KeyCode::Tab + KeyModifiers::SHIFT
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);

        app.handle_key_event(shift_tab);
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(shift_tab);
        assert_eq!(app.focused_pane(), FocusedPane::Inspector);

        app.handle_key_event(shift_tab);
        assert_eq!(app.focused_pane(), FocusedPane::Stream);

        app.handle_key_event(shift_tab);
        assert_eq!(app.focused_pane(), FocusedPane::Sessions);
    }

    #[test]
    fn test_composer_captures_character_input_and_backspace() {
        let mut app = App::new();
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        app.handle_key_event(backtab);
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "hi");

        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "h");
    }

    #[test]
    fn test_non_composer_panes_ignore_typing_and_allow_quit() {
        let mut app = App::new();
        assert_eq!(app.focused_pane(), FocusedPane::Sessions);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "");
        assert!(!app.should_quit());

        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit());
    }
}