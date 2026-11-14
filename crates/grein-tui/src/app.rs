use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    Tree,
    Transcript,
    Composer,
    Blackboard,
}

impl FocusPane {
    pub fn next(self) -> Self {
        match self {
            Self::Tree => Self::Transcript,
            Self::Transcript => Self::Composer,
            Self::Composer => Self::Blackboard,
            Self::Blackboard => Self::Tree,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Tree => Self::Blackboard,
            Self::Transcript => Self::Tree,
            Self::Composer => Self::Transcript,
            Self::Blackboard => Self::Composer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    focus: FocusPane,
    composer_input: String,
    should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            focus: FocusPane::Tree,
            composer_input: String::new(),
            should_quit: false,
        }
    }

    pub fn focus(&self) -> FocusPane {
        self.focus
    }

    pub fn set_focus(&mut self, focus: FocusPane) {
        self.focus = focus;
    }

    pub fn focus_next(&mut self) {
        self.focus = self.focus.next();
    }

    pub fn focus_prev(&mut self) {
        self.focus = self.focus.prev();
    }

    pub fn composer_input(&self) -> &str {
        &self.composer_input
    }

    pub fn take_composer_input(&mut self) -> String {
        std::mem::take(&mut self.composer_input)
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match key.code {
            KeyCode::Tab => {
                self.focus_next();
            }
            KeyCode::BackTab => {
                self.focus_prev();
            }
            KeyCode::Char(c) => {
                if self.focus == FocusPane::Composer {
                    self.composer_input.push(c);
                } else if c == 'q' {
                    self.should_quit = true;
                }
            }
            KeyCode::Backspace => {
                if self.focus == FocusPane::Composer {
                    self.composer_input.pop();
                }
            }
            KeyCode::Esc => {
                self.should_quit = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_cycles_focus_forward() {
        let mut app = App::default();
        assert_eq!(app.focus(), FocusPane::Tree);

        app.focus_next();
        assert_eq!(app.focus(), FocusPane::Transcript);

        app.focus_next();
        assert_eq!(app.focus(), FocusPane::Composer);

        app.focus_next();
        assert_eq!(app.focus(), FocusPane::Blackboard);

        app.focus_next();
        assert_eq!(app.focus(), FocusPane::Tree);
    }

    #[test]
    fn test_backtab_cycles_focus_backward() {
        let mut app = App::default();
        assert_eq!(app.focus(), FocusPane::Tree);

        app.focus_prev();
        assert_eq!(app.focus(), FocusPane::Blackboard);

        app.focus_prev();
        assert_eq!(app.focus(), FocusPane::Composer);

        app.focus_prev();
        assert_eq!(app.focus(), FocusPane::Transcript);

        app.focus_prev();
        assert_eq!(app.focus(), FocusPane::Tree);
    }
}
