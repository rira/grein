use grein_tui::status::{
    render_status_glyph, render_thread_indicator, BackgroundEvent, FocusTarget, PriorityScheduler,
    RenderMode, Task, ThreadIndicatorState, ThreadStatus, UiFocusController,
};

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
