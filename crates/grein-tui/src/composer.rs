//! Non-blocking prompt input composer state machine.
//! Enforces Universal Two-Tier Architecture and zero-null invariants.

/// Lifecycle state of the input composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComposerState {
    #[default]
    Idle,
    Editing,
}

/// Events processed by the input composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerEvent {
    InsertChar(char),
    InsertText(String),
    Backspace,
    Delete,
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
    Submit,
    Clear,
}

/// Action resulting from handling a composer event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ComposerAction {
    #[default]
    None,
    Submitted(String),
    Cleared,
}

/// Non-blocking prompt input composer.
#[derive(Debug, Clone, Default)]
pub struct Composer {
    chars: Vec<char>,
    text: String,
    cursor: usize,
    state: ComposerState,
}

impl Composer {
    /// Creates a new empty `Composer` in the `Idle` state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chars: Vec::new(),
            text: String::new(),
            cursor: 0,
            state: ComposerState::Idle,
        }
    }

    /// Returns a reference to the current composed text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the current cursor position as a character offset.
    #[must_use]
    pub fn cursor_position(&self) -> usize {
        self.cursor
    }

    /// Returns the number of Unicode characters in the composer.
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    /// Returns the current state of the composer.
    #[must_use]
    pub fn state(&self) -> ComposerState {
        self.state
    }

    /// Returns whether the composer currently contains any text.
    #[must_use]
    pub fn has_content(&self) -> bool {
        !self.chars.is_empty()
    }

    /// Handles an incoming composer event and transitions internal state.
    pub fn handle_event(&mut self, event: ComposerEvent) -> ComposerAction {
        match event {
            ComposerEvent::InsertChar(c) => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
                self.sync_state();
                ComposerAction::None
            }
            ComposerEvent::InsertText(text) => {
                for c in text.chars() {
                    self.chars.insert(self.cursor, c);
                    self.cursor += 1;
                }
                self.sync_state();
                ComposerAction::None
            }
            ComposerEvent::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                    self.sync_state();
                }
                ComposerAction::None
            }
            ComposerEvent::Delete => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                    self.sync_state();
                }
                ComposerAction::None
            }
            ComposerEvent::CursorLeft => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ComposerAction::None
            }
            ComposerEvent::CursorRight => {
                if self.cursor < self.chars.len() {
                    self.cursor += 1;
                }
                ComposerAction::None
            }
            ComposerEvent::CursorHome => {
                self.cursor = 0;
                ComposerAction::None
            }
            ComposerEvent::CursorEnd => {
                self.cursor = self.chars.len();
                ComposerAction::None
            }
            ComposerEvent::Submit => {
                if self.chars.is_empty() {
                    ComposerAction::None
                } else {
                    let submitted = std::mem::take(&mut self.text);
                    self.chars.clear();
                    self.cursor = 0;
                    self.state = ComposerState::Idle;
                    ComposerAction::Submitted(submitted)
                }
            }
            ComposerEvent::Clear => {
                self.chars.clear();
                self.text.clear();
                self.cursor = 0;
                self.state = ComposerState::Idle;
                ComposerAction::Cleared
            }
        }
    }

    fn sync_state(&mut self) {
        self.text = self.chars.iter().collect();
        self.state = if self.chars.is_empty() {
            ComposerState::Idle
        } else {
            ComposerState::Editing
        };
    }
}
