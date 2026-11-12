use crate::focus::FocusManager;
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

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    Streaming,
    ExecutingTool,
    PermissionRequired,
    AwaitingInput,
    Idle,
    Error,
}

impl ThreadStatus {
    pub fn glyph(&self) -> &'static str {
        match self {
            Self::Streaming => "⠋",
            Self::ExecutingTool => "⚙",
            Self::PermissionRequired => "⚠️",
            Self::AwaitingInput => "💬",
            Self::Idle => "✓",
            Self::Error => "✖",
        }
    }

    pub fn ascii_fallback(&self) -> &'static str {
        match self {
            Self::Streaming => "[*]",
            Self::ExecutingTool => "[>]",
            Self::PermissionRequired => "[!]",
            Self::AwaitingInput => "[?]",
            Self::Idle => "[-]",
            Self::Error => "[x]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundEvent {
    StreamStarted {
        thread_id: ThreadId,
    },
    StreamCompleted {
        thread_id: ThreadId,
    },
    ToolExecutionStarted {
        thread_id: ThreadId,
        tool_name: String,
    },
    ToolExecutionFinished {
        thread_id: ThreadId,
        tool_name: String,
    },
    PermissionRequired {
        thread_id: ThreadId,
        request_id: String,
    },
    AwaitingInput {
        thread_id: ThreadId,
    },
    Error {
        thread_id: ThreadId,
        message: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ThreadStatusRegistry {
    statuses: HashMap<ThreadId, ThreadStatus>,
}

impl ThreadStatusRegistry {
    pub fn new() -> Self {
        Self {
            statuses: HashMap::new(),
        }
    }

    pub fn set_status(&mut self, thread_id: ThreadId, status: ThreadStatus) {
        self.statuses.insert(thread_id, status);
    }

    pub fn status_for(&self, thread_id: &ThreadId) -> Option<ThreadStatus> {
        self.statuses.get(thread_id).copied()
    }

    pub fn handle_event(&mut self, event: &BackgroundEvent, _focus_manager: &mut FocusManager) {
        // Invariant: Background stream completions, finished tool runs, or alerts
        // must NEVER preempt keyboard focus from the active input composer.
        match event {
            BackgroundEvent::StreamStarted { thread_id } => {
                self.statuses
                    .insert(thread_id.clone(), ThreadStatus::Streaming);
            }
            BackgroundEvent::StreamCompleted { thread_id } => {
                self.statuses.insert(thread_id.clone(), ThreadStatus::Idle);
            }
            BackgroundEvent::ToolExecutionStarted { thread_id, .. } => {
                self.statuses
                    .insert(thread_id.clone(), ThreadStatus::ExecutingTool);
            }
            BackgroundEvent::ToolExecutionFinished { thread_id, .. } => {
                self.statuses.insert(thread_id.clone(), ThreadStatus::Idle);
            }
            BackgroundEvent::PermissionRequired { thread_id, .. } => {
                self.statuses
                    .insert(thread_id.clone(), ThreadStatus::PermissionRequired);
            }
            BackgroundEvent::AwaitingInput { thread_id } => {
                self.statuses
                    .insert(thread_id.clone(), ThreadStatus::AwaitingInput);
            }
            BackgroundEvent::Error { thread_id, .. } => {
                self.statuses.insert(thread_id.clone(), ThreadStatus::Error);
            }
        }
    }
}
