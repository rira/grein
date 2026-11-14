use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    Tree,
    Transcript,
    Diff,
    Composer,
}

impl FocusPane {
    pub fn next(self) -> Self {
        match self {
            Self::Tree => Self::Transcript,
            Self::Transcript => Self::Diff,
            Self::Diff => Self::Composer,
            Self::Composer => Self::Tree,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Tree => Self::Composer,
            Self::Transcript => Self::Tree,
            Self::Diff => Self::Transcript,
            Self::Composer => Self::Diff,
        }
    }

    pub fn previous(self) -> Self {
        self.prev()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct App {
    pub focus: FocusPane,
    pub composer_input: String,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            focus: FocusPane::Tree,
            composer_input: String::new(),
            should_quit: false,
        }
    }

    pub fn focus_next(&mut self) {
        self.focus = self.focus.next();
    }

    pub fn focus_prev(&mut self) {
        self.focus = self.focus.prev();
    }

    pub fn focus_previous(&mut self) {
        self.focus = self.focus.prev();
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => {
                self.focus_next();
            }
            KeyCode::BackTab => {
                self.focus_prev();
            }
            KeyCode::Esc => {
                if self.focus == FocusPane::Composer {
                    self.focus = FocusPane::Tree;
                }
            }
            KeyCode::Backspace => {
                if self.focus == FocusPane::Composer {
                    self.composer_input.pop();
                }
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                    self.should_quit = true;
                    return;
                }
                if self.focus == FocusPane::Composer {
                    self.composer_input.push(c);
                } else if c == 'q' {
                    self.should_quit = true;
                }
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
    fn test_composer_esc_returns_to_tree_focus() {
        let mut app = App::new();
        app.focus = FocusPane::Composer;

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.focus, FocusPane::Tree);
    }
}
