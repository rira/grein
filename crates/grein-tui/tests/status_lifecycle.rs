//! Black-box tests for thread status indicators and focus non-stealing invariant.

use grein_tui::focus::{FocusManager, FocusTarget};
use grein_tui::status::{BackgroundEvent, ThreadId, ThreadStatus, ThreadStatusRegistry};

#[test]
fn test_focus_non_stealing_on_background_stream_completion() {
    let mut focus_manager = FocusManager::new();
    let mut registry = ThreadStatusRegistry::new();

    let active_thread = ThreadId::new("thread-active");
    let background_thread = ThreadId::new("thread-bg");

    // Simulate user actively typing in the input composer of the active thread
    focus_manager.set_focus(FocusTarget::Composer(active_thread.clone()));
    assert_eq!(
        focus_manager.current_focus(),
        FocusTarget::Composer(active_thread.clone()),
        "Composer must initially hold focus"
    );

    // Dispatch a background stream completion event
    let completion_event = BackgroundEvent::StreamCompleted {
        thread_id: background_thread.clone(),
    };
    registry.handle_event(&completion_event, &mut focus_manager);

    // Focus must NOT be stolen from the active composer
    assert_eq!(
        focus_manager.current_focus(),
        FocusTarget::Composer(active_thread.clone()),
        "Invariant violated: background stream completion stole focus from active composer"
    );
    assert_eq!(
        registry.status_for(&background_thread),
        Some(ThreadStatus::Idle),
        "Background thread status should transition to Idle"
    );
}

#[test]
fn test_focus_non_stealing_on_background_tool_run_finished() {
    let mut focus_manager = FocusManager::new();
    let mut registry = ThreadStatusRegistry::new();

    let active_thread = ThreadId::new("thread-active");
    let background_thread = ThreadId::new("thread-bg");

    focus_manager.set_focus(FocusTarget::Composer(active_thread.clone()));

    // Dispatch background tool execution finish event
    let tool_event = BackgroundEvent::ToolExecutionFinished {
        thread_id: background_thread.clone(),
        tool_name: "bash".to_string(),
    };
    registry.handle_event(&tool_event, &mut focus_manager);

    assert_eq!(
        focus_manager.current_focus(),
        FocusTarget::Composer(active_thread.clone()),
        "Invariant violated: finished background tool run stole focus from active composer"
    );
}

#[test]
fn test_focus_non_stealing_on_permission_alert() {
    let mut focus_manager = FocusManager::new();
    let mut registry = ThreadStatusRegistry::new();

    let active_thread = ThreadId::new("thread-active");
    let background_thread = ThreadId::new("thread-bg");

    focus_manager.set_focus(FocusTarget::Composer(active_thread.clone()));

    // Dispatch permission required alert for background task
    let permission_event = BackgroundEvent::PermissionRequired {
        thread_id: background_thread.clone(),
        request_id: "perm-123".to_string(),
    };
    registry.handle_event(&permission_event, &mut focus_manager);

    // Active input composer focus must be preserved while background thread marks permission needed
    assert_eq!(
        focus_manager.current_focus(),
        FocusTarget::Composer(active_thread.clone()),
        "Invariant violated: permission alert stole focus from active input composer"
    );
    assert_eq!(
        registry.status_for(&background_thread),
        Some(ThreadStatus::PermissionRequired),
        "Background thread status should indicate PermissionRequired without stealing focus"
    );
}
