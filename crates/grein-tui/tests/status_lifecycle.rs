use std::sync::{Arc, Mutex};

// Domain status enum representing thread lifecycle stages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    Streaming,
    ExecutingTool,
    PermissionRequired,
    AwaitingInput,
    Idle,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadIndicatorState {
    pub status: ThreadStatus,
    pub is_dirty: bool,
    pub has_unread_events: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Unicode,
    Ascii,
}

// Function contract for rendering status glyphs and indicators
pub fn render_status_glyph(status: ThreadStatus, mode: RenderMode) -> &'static str {
    match (status, mode) {
        (ThreadStatus::Streaming, RenderMode::Unicode) => "⠋",
        (ThreadStatus::Streaming, RenderMode::Ascii) => "[*]",
        (ThreadStatus::ExecutingTool, RenderMode::Unicode) => "⚙",
        (ThreadStatus::ExecutingTool, RenderMode::Ascii) => "[>]",
        (ThreadStatus::PermissionRequired, RenderMode::Unicode) => "⚠️",
        (ThreadStatus::PermissionRequired, RenderMode::Ascii) => "[!]",
        (ThreadStatus::AwaitingInput, RenderMode::Unicode) => "💬",
        (ThreadStatus::AwaitingInput, RenderMode::Ascii) => "[?]",
        (ThreadStatus::Idle, RenderMode::Unicode) => "✓",
        (ThreadStatus::Idle, RenderMode::Ascii) => "[-]",
        (ThreadStatus::Error, RenderMode::Unicode) => "✖",
        (ThreadStatus::Error, RenderMode::Ascii) => "[x]",
    }
}

pub fn render_thread_indicator(state: &ThreadIndicatorState, mode: RenderMode) -> String {
    let prefix = if state.has_unread_events {
        match mode {
            RenderMode::Unicode => "● ",
            RenderMode::Ascii => "* ",
        }
    } else {
        ""
    };

    let glyph = render_status_glyph(state.status, mode);
    let suffix = if state.is_dirty { "*" } else { "" };

    format!("{prefix}{glyph}{suffix}")
}

// Concurrency priority scheduler contract
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub task_id: u64,
    pub thread_id: u64,
    pub payload: String,
}

pub struct PriorityScheduler {
    focused_thread_id: Arc<Mutex<Option<u64>>>,
    queue: Arc<Mutex<Vec<Task>>>,
}

impl PriorityScheduler {
    pub fn new() -> Self {
        Self {
            focused_thread_id: Arc::new(Mutex::new(None)),
            queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_focused_thread(&self, thread_id: Option<u64>) {
        let mut focused = self.focused_thread_id.lock().unwrap();
        *focused = thread_id;
    }

    pub fn schedule(&self, task: Task) {
        let mut q = self.queue.lock().unwrap();
        q.push(task);
    }

    pub fn pop_next(&self) -> Option<Task> {
        let mut q = self.queue.lock().unwrap();
        let focused = *self.focused_thread_id.lock().unwrap();

        if let Some(target_thread) = focused {
            if let Some(pos) = q.iter().position(|t| t.thread_id == target_thread) {
                return Some(q.remove(pos));
            }
        }

        if !q.is_empty() {
            Some(q.remove(0))
        } else {
            None
        }
    }
}

// Focus non-stealing UI state contract
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    InputComposer,
    ThreadNavigator,
    PermissionModal,
}

#[derive(Debug, Clone)]
pub enum BackgroundEvent {
    StreamCompleted { thread_id: u64 },
    ToolFinished { thread_id: u64 },
    PermissionAlert { thread_id: u64, prompt: String },
}

pub struct UiFocusController {
    pub current_focus: FocusTarget,
    pub is_user_typing: bool,
    pub pending_permissions: Vec<(u64, String)>,
}

impl UiFocusController {
    pub fn new() -> Self {
        Self {
            current_focus: FocusTarget::ThreadNavigator,
            is_user_typing: false,
            pending_permissions: Vec::new(),
        }
    }

    pub fn handle_background_event(&mut self, event: BackgroundEvent) {
        match event {
            BackgroundEvent::StreamCompleted { .. } | BackgroundEvent::ToolFinished { .. } => {
                // Background completions never steal focus regardless of state
            }
            BackgroundEvent::PermissionAlert { thread_id, prompt } => {
                self.pending_permissions.push((thread_id, prompt));
                // Focus non-stealing invariant: If user is actively typing in composer, do not divert focus
                if !self.is_user_typing && self.current_focus != FocusTarget::InputComposer {
                    self.current_focus = FocusTarget::PermissionModal;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbolic_status_glyph_renderings_unicode_and_ascii() {
        let statuses = [
            (ThreadStatus::Streaming, "⠋", "[*]"),
            (ThreadStatus::ExecutingTool, "⚙", "[>]"),
            (ThreadStatus::PermissionRequired, "⚠️", "[!]"),
            (ThreadStatus::AwaitingInput, "💬", "[?]"),
            (ThreadStatus::Idle, "✓", "[-]"),
            (ThreadStatus::Error, "✖", "[x]"),
        ];

        for (status, expected_unicode, expected_ascii) in statuses {
            assert_eq!(
                render_status_glyph(status, RenderMode::Unicode),
                expected_unicode,
                "Failed unicode rendering for status {status:?}"
            );
            assert_eq!(
                render_status_glyph(status, RenderMode::Ascii),
                expected_ascii,
                "Failed ascii fallback rendering for status {status:?}"
            );
        }
    }

    #[test]
    fn test_thread_indicator_with_dirty_suffix_and_unread_prefix() {
        let clean_idle = ThreadIndicatorState {
            status: ThreadStatus::Idle,
            is_dirty: false,
            has_unread_events: false,
        };
        assert_eq!(
            render_thread_indicator(&clean_idle, RenderMode::Unicode),
            "✓"
        );
        assert_eq!(
            render_thread_indicator(&clean_idle, RenderMode::Ascii),
            "[-]"
        );

        let dirty_running = ThreadIndicatorState {
            status: ThreadStatus::ExecutingTool,
            is_dirty: true,
            has_unread_events: false,
        };
        assert_eq!(
            render_thread_indicator(&dirty_running, RenderMode::Unicode),
            "⚙*"
        );
        assert_eq!(
            render_thread_indicator(&dirty_running, RenderMode::Ascii),
            "[>]*"
        );

        let unread_and_dirty = ThreadIndicatorState {
            status: ThreadStatus::PermissionRequired,
            is_dirty: true,
            has_unread_events: true,
        };
        assert_eq!(
            render_thread_indicator(&unread_and_dirty, RenderMode::Unicode),
            "● ⚠️*"
        );
        assert_eq!(
            render_thread_indicator(&unread_and_dirty, RenderMode::Ascii),
            "* [!]*"
        );
    }

    #[test]
    fn test_concurrency_priority_scheduler_prefers_focused_thread() {
        let scheduler = PriorityScheduler::new();

        // Schedule background tasks from thread 100 and 200
        scheduler.schedule(Task {
            task_id: 1,
            thread_id: 100,
            payload: "bg_task_1".to_string(),
        });
        scheduler.schedule(Task {
            task_id: 2,
            thread_id: 200,
            payload: "bg_task_2".to_string(),
        });
        // Thread 200 becomes the focused active thread
        scheduler.schedule(Task {
            task_id: 3,
            thread_id: 200,
            payload: "focused_task_3".to_string(),
        });

        scheduler.set_focused_thread(Some(200));

        // First popped task must be from focused thread (task_id 2)
        let first = scheduler.pop_next().expect("Task should be popped");
        assert_eq!(first.task_id, 2);
        assert_eq!(first.thread_id, 200);

        // Second popped task must also be from focused thread (task_id 3)
        let second = scheduler.pop_next().expect("Task should be popped");
        assert_eq!(second.task_id, 3);
        assert_eq!(second.thread_id, 200);

        // Remaining background task is finally popped
        let third = scheduler.pop_next().expect("Task should be popped");
        assert_eq!(third.task_id, 1);
        assert_eq!(third.thread_id, 100);

        assert!(scheduler.pop_next().is_none());
    }

    #[test]
    fn test_focus_non_stealing_invariant_while_user_typing() {
        let mut ui = UiFocusController::new();
        ui.current_focus = FocusTarget::InputComposer;
        ui.is_user_typing = true;

        // Stream completion event arrives in background
        ui.handle_background_event(BackgroundEvent::StreamCompleted { thread_id: 99 });
        assert_eq!(
            ui.current_focus,
            FocusTarget::InputComposer,
            "Stream completion must not steal focus from active composer"
        );

        // Tool run finished arrives in background
        ui.handle_background_event(BackgroundEvent::ToolFinished { thread_id: 99 });
        assert_eq!(
            ui.current_focus,
            FocusTarget::InputComposer,
            "Tool execution completion must not steal focus from active composer"
        );

        // Permission alert arrives while user is actively typing
        ui.handle_background_event(BackgroundEvent::PermissionAlert {
            thread_id: 99,
            prompt: "Grant fs:write lease?".to_string(),
        });
        assert_eq!(
            ui.current_focus,
            FocusTarget::InputComposer,
            "Permission alert MUST NOT steal focus while user is typing"
        );
        assert_eq!(ui.pending_permissions.len(), 1);
    }

    #[test]
    fn test_focus_transitions_when_user_not_typing() {
        let mut ui = UiFocusController::new();
        ui.current_focus = FocusTarget::ThreadNavigator;
        ui.is_user_typing = false;

        // Permission alert when idle navigates to modal
        ui.handle_background_event(BackgroundEvent::PermissionAlert {
            thread_id: 42,
            prompt: "Execute command?".to_string(),
        });
        assert_eq!(
            ui.current_focus,
            FocusTarget::PermissionModal,
            "Permission alert may focus modal when user is idle/navigating"
        );
    }
}
