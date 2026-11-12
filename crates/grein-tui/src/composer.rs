use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadId(String);

impl ThreadId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
    drafts: HashMap<ThreadId, String>,
}

impl ThreadDraftStore {
    pub fn new() -> Self {
        Self {
            drafts: HashMap::new(),
        }
    }

    pub fn save_draft(&mut self, thread_id: &ThreadId, draft: impl Into<String>) {
        self.drafts.insert(thread_id.clone(), draft.into());
    }

    pub fn get_draft(&self, thread_id: &ThreadId) -> Option<&str> {
        self.drafts.get(thread_id).map(|s| s.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEditorHook {
    initial_content: String,
}

impl ExternalEditorHook {
    pub fn new(initial_content: impl Into<String>) -> Self {
        Self {
            initial_content: initial_content.into(),
        }
    }

    pub fn initial_content(&self) -> &str {
        &self.initial_content
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalPagerHook {
    target_diff_id: String,
}

impl ExternalPagerHook {
    pub fn new(target_diff_id: impl Into<String>) -> Self {
        Self {
            target_diff_id: target_diff_id.into(),
        }
    }

    pub fn target_diff_id(&self) -> &str {
        &self.target_diff_id
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
pub enum ComposerAction {
    None,
    DispatchTurn(String),
    SpawnExternalEditor(ExternalEditorHook),
    SpawnDiffPager(ExternalPagerHook),
    ClearReplyAnchor,
}

#[derive(Debug, Clone, Default)]
pub struct Composer {
    draft: String,
    reply_anchor: Option<String>,
    mode: ComposerMode,
}

impl Composer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_text(&mut self, text: &str) {
        self.draft.push_str(text);
    }

    pub fn draft_text(&self) -> &str {
        &self.draft
    }

    pub fn set_draft_text(&mut self, text: impl Into<String>) {
        self.draft = text.into();
    }

    pub fn is_empty(&self) -> bool {
        self.draft.is_empty()
    }

    pub fn clear(&mut self) {
        self.draft.clear();
    }

    pub fn set_reply_anchor(&mut self, anchor: Option<String>) {
        self.reply_anchor = anchor;
    }

    pub fn clear_reply_anchor(&mut self) {
        self.reply_anchor = None;
    }

    pub fn reply_badge_text(&self) -> Option<String> {
        self.reply_anchor
            .as_ref()
            .map(|anchor| format!("[Replying to: {anchor}]"))
    }

    pub fn set_mode(&mut self, mode: ComposerMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> &ComposerMode {
        &self.mode
    }

    pub fn apply_editor_result(&mut self, text: impl Into<String>) {
        self.draft = text.into();
    }

    pub fn handle_key(&mut self, event: KeyEvent) -> ComposerAction {
        if event.kind != KeyEventKind::Press {
            return ComposerAction::None;
        }

        if let ComposerMode::DiffPreviewActive { ref diff_id } = self.mode {
            if event.code == KeyCode::Char('o') && event.modifiers.is_empty() {
                return ComposerAction::SpawnDiffPager(ExternalPagerHook::new(diff_id.clone()));
            }
        }

        match event.code {
            KeyCode::Enter => {
                if event.modifiers.contains(KeyModifiers::SHIFT)
                    || event.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.draft.push('\n');
                    ComposerAction::None
                } else if event.modifiers.is_empty() {
                    let dispatched = std::mem::take(&mut self.draft);
                    ComposerAction::DispatchTurn(dispatched)
                } else {
                    ComposerAction::None
                }
            }
            KeyCode::Char('e') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                ComposerAction::SpawnExternalEditor(ExternalEditorHook::new(self.draft.clone()))
            }
            KeyCode::Esc => ComposerAction::ClearReplyAnchor,
            _ => ComposerAction::None,
        }
    }
}
