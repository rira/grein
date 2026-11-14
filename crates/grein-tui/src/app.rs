use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    Tree,
    Chat,
    Composer,
    Terminal,
}

impl FocusPane {
    pub fn next(self) -> Self {
        match self {
            Self::Tree => Self::Chat,
            Self::Chat => Self::Composer,
            Self::Composer => Self::Terminal,
            Self::Terminal => Self::Tree,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Tree => Self::Terminal,
            Self::Chat => Self::Tree,
            Self::Composer => Self::Chat,
            Self::Terminal => Self::Composer,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct App {
    pub focus: FocusPane,
    pub composer_input: String,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => {
                self.focus = self.focus.next();
                return;
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev();
                return;
            }
            _ => {}
        }

        match self.focus {
            FocusPane::Composer => match key.code {
                KeyCode::Esc => {
                    self.focus = FocusPane::Tree;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.composer_input.push(c);
                }
                KeyCode::Backspace => {
                    self.composer_input.pop();
                }
                _ => {}
            },
            _ => match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                }
                KeyCode::Char('i') => {
                    self.focus = FocusPane::Composer;
                }
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_composer_esc_returns_to_tree_focus() {
        let mut app = App::new();
        app.focus = FocusPane::Composer;

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.focus, FocusPane::Tree);
    }
}
