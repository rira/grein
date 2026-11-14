//! Non-blocking prompt input composer pane.

use crate::focus::PaneFocus;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Actions resulting from composer event handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    /// Submitted message payload.
    Submit(String),
    /// Request focus cycle to the next pane.
    NextPane,
    /// Request focus cycle to the previous pane.
    PrevPane,
}

/// Outcome of evaluating a key event in the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerEventResult {
    /// Key was consumed internally by the composer.
    Consumed,
    /// Key was ignored (e.g. composer pane is not focused).
    Ignored,
    /// Key triggered an external lifecycle or navigation action.
    Action(ComposerAction),
}

/// Input buffer managing user prompt text and cursor manipulation.
#[derive(Debug, Clone, Default)]
pub struct Composer {
    chars: Vec<char>,
    cursor: usize,
    text: String,
}

impl Composer {
    /// Creates a new, empty composer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a borrowed slice of the current text buffer.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns current cursor position as a character index.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Returns the total character count in the buffer.
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    /// Clears the composer buffer and resets cursor position to zero.
    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.text.clear();
    }

    /// Inserts a single Unicode character at the current cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
        self.sync_text();
    }

    /// Inserts a string slice at the current cursor position.
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.sync_text();
    }

    /// Moves cursor one character to the left, bounded at zero.
    pub fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Moves cursor one character to the right, bounded at buffer length.
    pub fn move_cursor_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    /// Moves cursor to the beginning of the buffer.
    pub fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Moves cursor to the end of the buffer.
    pub fn move_cursor_end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Deletes the character directly preceding the cursor.
    pub fn delete_char_before_cursor(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            self.sync_text();
        }
    }

    /// Deletes the character at the current cursor position.
    pub fn delete_char_after_cursor(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            self.sync_text();
        }
    }

    /// Dispatches a key event according to the current focus state.
    pub fn handle_key_event(&mut self, key: KeyEvent, focus: PaneFocus) -> ComposerEventResult {
        if focus != PaneFocus::Composer {
            return ComposerEventResult::Ignored;
        }

        match key.code {
            KeyCode::BackTab => ComposerEventResult::Action(ComposerAction::PrevPane),
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    ComposerEventResult::Action(ComposerAction::PrevPane)
                } else {
                    ComposerEventResult::Action(ComposerAction::NextPane)
                }
            }
            KeyCode::Enter => {
                let submitted = self.text.clone();
                self.clear();
                ComposerEventResult::Action(ComposerAction::Submit(submitted))
            }
            KeyCode::Char(c) => {
                self.insert_char(c);
                ComposerEventResult::Consumed
            }
            KeyCode::Backspace => {
                self.delete_char_before_cursor();
                ComposerEventResult::Consumed
            }
            KeyCode::Delete => {
                self.delete_char_after_cursor();
                ComposerEventResult::Consumed
            }
            KeyCode::Left => {
                self.move_cursor_left();
                ComposerEventResult::Consumed
            }
            KeyCode::Right => {
                self.move_cursor_right();
                ComposerEventResult::Consumed
            }
            KeyCode::Home => {
                self.move_cursor_home();
                ComposerEventResult::Consumed
            }
            KeyCode::End => {
                self.move_cursor_end();
                ComposerEventResult::Consumed
            }
            _ => ComposerEventResult::Ignored,
        }
    }

    fn sync_text(&mut self) {
        self.text = self.chars.iter().collect();
    }
}
