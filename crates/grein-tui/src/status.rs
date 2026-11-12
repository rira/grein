#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Unicode,
    Ascii,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadLifecycleStatus {
    Streaming,
    ExecutingTool,
    PermissionRequired,
    AwaitingInput,
    IdleCompleted,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spinner {
    frame_index: usize,
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Spinner {
    pub const FRAMES: [&'static str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    #[must_use]
    pub fn new() -> Self {
        Self { frame_index: 0 }
    }

    pub fn tick(&mut self) {
        self.frame_index = (self.frame_index + 1) % Self::FRAMES.len();
    }

    #[must_use]
    pub fn frame(&self) -> &'static str {
        Self::FRAMES[self.frame_index % Self::FRAMES.len()]
    }
}

impl ThreadLifecycleStatus {
    #[must_use]
    pub fn glyph(&self, mode: RenderMode, spinner: &Spinner) -> &str {
        match mode {
            RenderMode::Unicode => match self {
                Self::Streaming => spinner.frame(),
                Self::ExecutingTool => "⚙",
                Self::PermissionRequired => "⚠️",
                Self::AwaitingInput => "💬",
                Self::IdleCompleted => "✓",
                Self::Error => "✖",
            },
            RenderMode::Ascii => match self {
                Self::Streaming => "[*]",
                Self::ExecutingTool => "[>]",
                Self::PermissionRequired => "[!]",
                Self::AwaitingInput => "[?]",
                Self::IdleCompleted => "[-]",
                Self::Error => "[x]",
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadStatusIndicator {
    status: ThreadLifecycleStatus,
    dirty_workspace: bool,
    unread_events: bool,
}

impl ThreadStatusIndicator {
    #[must_use]
    pub fn new(status: ThreadLifecycleStatus) -> Self {
        Self {
            status,
            dirty_workspace: false,
            unread_events: false,
        }
    }

    #[must_use]
    pub fn with_dirty_workspace(mut self, dirty: bool) -> Self {
        self.dirty_workspace = dirty;
        self
    }

    #[must_use]
    pub fn with_unread_events(mut self, unread: bool) -> Self {
        self.unread_events = unread;
        self
    }

    #[must_use]
    pub fn status(&self) -> ThreadLifecycleStatus {
        self.status
    }

    #[must_use]
    pub fn is_dirty_workspace(&self) -> bool {
        self.dirty_workspace
    }

    #[must_use]
    pub fn has_unread_events(&self) -> bool {
        self.unread_events
    }

    #[must_use]
    pub fn render(&self, mode: RenderMode, spinner: &Spinner) -> String {
        let mut out = String::new();
        if self.unread_events {
            match mode {
                RenderMode::Unicode => out.push_str("● "),
                RenderMode::Ascii => out.push_str("* "),
            }
        }
        out.push_str(self.status.glyph(mode, spinner));
        if self.dirty_workspace {
            out.push('*');
        }
        out
    }
}
