#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Tree,
    Transcript,
    Blackboard,
    Composer,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnchorItem {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadItem {
    pub id: String,
    pub title: String,
    pub anchors: Vec<AnchorItem>,
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub active_focus: PaneFocus,
    pub threads: Vec<ThreadItem>,
    pub active_transcript_md: String,
    pub blackboard_notes: String,
    pub blackboard_collapsed: bool,
    pub input_buffer: String,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
}
