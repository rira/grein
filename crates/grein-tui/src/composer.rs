//! Non-blocking prompt input composer pane.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadId(String);

impl From<&str> for ThreadId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ThreadId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadDraftStore {
    drafts: std::collections::HashMap<ThreadId, String>,
}

impl ThreadDraftStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn save_draft(&mut self, thread_id: &ThreadId, draft: &str) {
        self.drafts.insert(thread_id.clone(), draft.to_string());
    }

    #[must_use]
    pub fn get_draft<'a>(&'a self, thread_id: &ThreadId) -> Option<&'a str> {
        self.drafts.get(thread_id).map(|s| s.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ComposerMode {
    #[default]
    Normal,
    DiffPreviewActive {
        diff_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorHook(String);

impl EditorHook {
    #[must_use]
    pub fn initial_content(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPagerHook(String);

impl DiffPagerHook {
    #[must_use]
    pub fn target_diff_id(&self) -> &str {
        &self.0
    }
}

pub trait IsComposerFocused {
    fn is_composer_focused(&self) -> bool;
}

impl IsComposerFocused for crate::focus::PaneFocus {
    fn is_composer_focused(&self) -> bool {
        matches!(self, crate::focus::PaneFocus::Composer)
    }
}

impl IsComposerFocused for crate::app::FocusedPane {
    fn is_composer_focused(&self) -> bool {
        matches!(self, crate::app::FocusedPane::Composer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerEvent {
    InsertChar(char),
    InsertText(String),
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
    Backspace,
    Delete,
    Clear,
    Submit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComposerState {
    #[default]
    Idle,
    Editing,
    Submitting,
}

/// Actions resulting from composer event handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    None,
    /// Submitted message payload.
    Submit(String),
    Submitted(String),
    Cleared,
    DispatchTurn(String),
    SpawnExternalEditor(EditorHook),
    ClearReplyAnchor,
    SpawnDiffPager(DiffPagerHook),
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
    reply_anchor: Option<String>,
    mode: ComposerMode,
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

    #[must_use]
    pub fn cursor_position(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub fn state(&self) -> ComposerState {
        if self.text.is_empty() {
            ComposerState::Idle
        } else {
            ComposerState::Editing
        }
    }

    #[must_use]
    pub fn has_content(&self) -> bool {
        !self.text.is_empty()
    }

    pub fn handle_event(&mut self, event: ComposerEvent) -> ComposerAction {
        match event {
            ComposerEvent::InsertChar(c) => {
                self.insert_char(c);
                ComposerAction::None
            }
            ComposerEvent::InsertText(s) => {
                self.insert_str(&s);
                ComposerAction::None
            }
            ComposerEvent::CursorLeft => {
                self.move_cursor_left();
                ComposerAction::None
            }
            ComposerEvent::CursorRight => {
                self.move_cursor_right();
                ComposerAction::None
            }
            ComposerEvent::CursorHome => {
                self.move_cursor_home();
                ComposerAction::None
            }
            ComposerEvent::CursorEnd => {
                self.move_cursor_end();
                ComposerAction::None
            }
            ComposerEvent::Backspace => {
                self.delete_char_before_cursor();
                ComposerAction::None
            }
            ComposerEvent::Delete => {
                self.delete_char_after_cursor();
                ComposerAction::None
            }
            ComposerEvent::Clear => {
                self.clear();
                ComposerAction::Cleared
            }
            ComposerEvent::Submit => {
                if self.text.is_empty() {
                    ComposerAction::None
                } else {
                    let msg = std::mem::take(&mut self.text);
                    self.chars.clear();
                    self.cursor = 0;
                    ComposerAction::Submitted(msg)
                }
            }
        }
    }

    /// Returns the total character count in the buffer.
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    /// Returns whether the text buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Returns the current draft text.
    #[must_use]
    pub fn draft_text(&self) -> &str {
        &self.text
    }

    /// Sets the draft text and resets cursor to the end.
    pub fn set_draft_text(&mut self, s: &str) {
        self.text = s.to_string();
        self.chars = s.chars().collect();
        self.cursor = self.chars.len();
    }

    /// Appends text to the buffer.
    pub fn insert_text(&mut self, s: &str) {
        self.insert_str(s);
    }

    /// Applies external editor result to draft buffer.
    pub fn apply_editor_result(&mut self, s: &str) {
        self.set_draft_text(s);
    }

    /// Sets or clears the contextual reply anchor.
    pub fn set_reply_anchor(&mut self, anchor: Option<String>) {
        self.reply_anchor = anchor;
    }

    /// Clears the contextual reply anchor.
    pub fn clear_reply_anchor(&mut self) {
        self.reply_anchor = None;
    }

    /// Formats the reply badge text if active.
    #[must_use]
    pub fn reply_badge_text(&self) -> Option<String> {
        self.reply_anchor
            .as_ref()
            .map(|a| format!("[Replying to: {a}]"))
    }

    /// Sets the current composer operational mode.
    pub fn set_mode(&mut self, mode: ComposerMode) {
        self.mode = mode;
    }

    /// Handles key events directly for ergonomics testing.
    pub fn handle_key(&mut self, key: KeyEvent) -> ComposerAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('e') {
            return ComposerAction::SpawnExternalEditor(EditorHook(self.text.clone()));
        }

        if (key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::Enter)
            || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Enter)
        {
            self.insert_char('\n');
            return ComposerAction::None;
        }

        if key.code == KeyCode::Enter {
            let submitted = self.text.clone();
            self.clear();
            return ComposerAction::DispatchTurn(submitted);
        }

        if key.code == KeyCode::Esc && self.reply_anchor.is_some() {
            return ComposerAction::ClearReplyAnchor;
        }

        if let ComposerMode::DiffPreviewActive { ref diff_id } = self.mode {
            if key.code == KeyCode::Char('o') {
                return ComposerAction::SpawnDiffPager(DiffPagerHook(diff_id.clone()));
            }
        }

        ComposerAction::None
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
    pub fn handle_key_event(
        &mut self,
        key: KeyEvent,
        focus: impl IsComposerFocused,
    ) -> ComposerEventResult {
        if !focus.is_composer_focused() {
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

pub type ComposerPane = Composer;
