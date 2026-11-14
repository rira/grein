pub mod transcript;

pub use transcript::{TranscriptMessage, TranscriptPane};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneFocus {
    Sidebar,
    Transcript,
    Composer,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusDirection {
    Forward,
    Backward,
}

impl PaneFocus {
    pub fn next(&self, direction: FocusDirection) -> Self {
        match direction {
            FocusDirection::Forward => match self {
                Self::Sidebar => Self::Transcript,
                Self::Transcript => Self::Composer,
                Self::Composer => Self::Details,
                Self::Details => Self::Sidebar,
            },
            FocusDirection::Backward => match self {
                Self::Sidebar => Self::Details,
                Self::Transcript => Self::Sidebar,
                Self::Composer => Self::Transcript,
                Self::Details => Self::Composer,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    Scrolled { delta: i32 },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    FocusChanged(PaneFocus),
    Key(crossterm::event::KeyEvent),
}

pub trait TuiComponent {
    fn is_focused(&self) -> bool;
    fn handle_event(&mut self, event: &AppEvent) -> bool;
    fn dispatch_key(&mut self, event: &AppEvent) -> Option<AppAction>;
}

pub async fn run() -> std::io::Result<()> {
    Ok(())
}
