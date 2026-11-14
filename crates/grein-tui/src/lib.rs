#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaneId {
    #[default]
    WorkspaceTree,
    SessionStream,
    DiffInspector,
    PromptComposer,
}

impl PaneId {
    pub const ALL: [PaneId; 4] = [
        PaneId::WorkspaceTree,
        PaneId::SessionStream,
        PaneId::DiffInspector,
        PaneId::PromptComposer,
    ];

    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::SessionStream,
            Self::SessionStream => Self::DiffInspector,
            Self::DiffInspector => Self::PromptComposer,
            Self::PromptComposer => Self::WorkspaceTree,
        }
    }

    #[must_use]
    pub fn previous(self) -> Self {
        match self {
            Self::WorkspaceTree => Self::PromptComposer,
            Self::SessionStream => Self::WorkspaceTree,
            Self::DiffInspector => Self::SessionStream,
            Self::PromptComposer => Self::DiffInspector,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Next,
    Previous,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerInput {
    buffer: String,
}

impl ComposerInput {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    #[must_use]
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn insert_char(&mut self, ch: char) {
        self.buffer.push(ch);
    }

    pub fn backspace(&mut self) {
        self.buffer.pop();
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    pub fn submit(&mut self) -> Option<String> {
        if self.buffer.trim().is_empty() {
            return None;
        }
        let submitted = std::mem::take(&mut self.buffer);
        Some(submitted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    Updated(String),
    Submitted(String),
    Cleared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiEvent {
    Tab,
    BackTab,
    Char(char),
    Backspace,
    Submit,
    Clear,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiAction {
    FocusChanged(PaneId),
    Composer(ComposerAction),
    Quit,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppState {
    focused_pane: PaneId,
    composer: ComposerInput,
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    pub fn set_focus(&mut self, pane: PaneId) {
        self.focused_pane = pane;
    }

    pub fn cycle_focus(&mut self, direction: FocusDirection) -> PaneId {
        self.focused_pane = match direction {
            FocusDirection::Next => self.focused_pane.next(),
            FocusDirection::Previous => self.focused_pane.previous(),
        };
        self.focused_pane
    }

    #[must_use]
    pub fn composer(&self) -> &ComposerInput {
        &self.composer
    }

    pub fn composer_mut(&mut self) -> &mut ComposerInput {
        &mut self.composer
    }
}

pub fn handle_key_event(state: &mut AppState, event: TuiEvent) -> TuiAction {
    match event {
        TuiEvent::Tab => {
            let next_pane = state.cycle_focus(FocusDirection::Next);
            TuiAction::FocusChanged(next_pane)
        }
        TuiEvent::BackTab => {
            let prev_pane = state.cycle_focus(FocusDirection::Previous);
            TuiAction::FocusChanged(prev_pane)
        }
        TuiEvent::Quit => TuiAction::Quit,
        TuiEvent::Char(ch) => {
            if state.focused_pane() == PaneId::PromptComposer {
                state.composer_mut().insert_char(ch);
                TuiAction::Composer(ComposerAction::Updated(
                    state.composer().buffer().to_string(),
                ))
            } else {
                TuiAction::Ignored
            }
        }
        TuiEvent::Backspace => {
            if state.focused_pane() == PaneId::PromptComposer {
                state.composer_mut().backspace();
                TuiAction::Composer(ComposerAction::Updated(
                    state.composer().buffer().to_string(),
                ))
            } else {
                TuiAction::Ignored
            }
        }
        TuiEvent::Clear => {
            if state.focused_pane() == PaneId::PromptComposer {
                state.composer_mut().clear();
                TuiAction::Composer(ComposerAction::Cleared)
            } else {
                TuiAction::Ignored
            }
        }
        TuiEvent::Submit => {
            if state.focused_pane() == PaneId::PromptComposer {
                match state.composer_mut().submit() {
                    Some(submitted) => TuiAction::Composer(ComposerAction::Submitted(submitted)),
                    None => TuiAction::Ignored,
                }
            } else {
                TuiAction::Ignored
            }
        }
    }
}

pub async fn run() -> std::io::Result<()> {
    Ok(())
}
