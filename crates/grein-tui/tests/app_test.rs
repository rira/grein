use grein_tui::app::{App, Pane};

#[test]
fn test_focus_cycling_forward_and_backward() {
    let mut app = App::new();
    assert_eq!(app.focused_pane(), Pane::TreeNavigator);

    // Forward cycle (Tab)
    app.focus_next();
    assert_eq!(app.focused_pane(), Pane::Transcript);

    app.focus_next();
    assert_eq!(app.focused_pane(), Pane::Blackboard);

    app.focus_next();
    assert_eq!(app.focused_pane(), Pane::Composer);

    // Wrap around to start
    app.focus_next();
    assert_eq!(app.focused_pane(), Pane::TreeNavigator);

    // Backward cycle (Shift+Tab)
    app.focus_prev();
    assert_eq!(app.focused_pane(), Pane::Composer);

    app.focus_prev();
    assert_eq!(app.focused_pane(), Pane::Blackboard);

    app.focus_prev();
    assert_eq!(app.focused_pane(), Pane::Transcript);

    app.focus_prev();
    assert_eq!(app.focused_pane(), Pane::TreeNavigator);
}

#[test]
fn test_collapsible_blackboard_toggling_and_focus_skipping() {
    let mut app = App::new();
    assert!(
        app.is_blackboard_visible(),
        "Blackboard should be visible by default"
    );

    // Toggle blackboard visibility to hidden
    app.toggle_blackboard();
    assert!(!app.is_blackboard_visible());

    // When blackboard is collapsed, focus cycling should skip it
    assert_eq!(app.focused_pane(), Pane::TreeNavigator);
    app.focus_next();
    assert_eq!(app.focused_pane(), Pane::Transcript);
    app.focus_next();
    assert_eq!(
        app.focused_pane(),
        Pane::Composer,
        "Collapsed blackboard must be skipped during focus cycling"
    );
    app.focus_next();
    assert_eq!(app.focused_pane(), Pane::TreeNavigator);

    // Reverse cycling should also skip collapsed blackboard
    app.focus_prev();
    assert_eq!(app.focused_pane(), Pane::Composer);
    app.focus_prev();
    assert_eq!(app.focused_pane(), Pane::Transcript);

    // If blackboard is focused when collapsed, focus must safely re-target
    app.toggle_blackboard();
    assert!(app.is_blackboard_visible());
    app.focus_next(); // to Blackboard
    assert_eq!(app.focused_pane(), Pane::Blackboard);
    app.toggle_blackboard(); // collapse while focused
    assert_ne!(app.focused_pane(), Pane::Blackboard);
    assert!(matches!(
        app.focused_pane(),
        Pane::Transcript | Pane::TreeNavigator
    ));
}

#[test]
fn test_alt_anchor_jumps() {
    let mut app = App::new();
    app.register_anchor(1, "thread-1", 10);
    app.register_anchor(2, "thread-2", 25);
    app.register_anchor(9, "thread-3", 50);

    // Jump to anchor 1
    let jumped = app.jump_to_anchor(1);
    assert!(jumped);
    assert_eq!(app.active_thread_id(), "thread-1");
    assert_eq!(app.active_thread_scrollback(), 10);

    // Jump to anchor 2
    let jumped = app.jump_to_anchor(2);
    assert!(jumped);
    assert_eq!(app.active_thread_id(), "thread-2");
    assert_eq!(app.active_thread_scrollback(), 25);

    // Jump to non-existent anchor within 1..9 range returns false and does not change state
    let jumped_missing = app.jump_to_anchor(5);
    assert!(!jumped_missing);
    assert_eq!(app.active_thread_id(), "thread-2");

    // Invalid anchor out of 1..9 range
    let jumped_out_of_range = app.jump_to_anchor(0);
    assert!(!jumped_out_of_range);
    let jumped_above_range = app.jump_to_anchor(10);
    assert!(!jumped_above_range);
}

#[test]
fn test_isolated_per_thread_scrollback_buffers() {
    let mut app = App::new();
    app.select_thread("thread-alpha");
    assert_eq!(app.active_thread_scrollback(), 0);

    // Scroll down 15 lines in thread-alpha
    app.scroll_active_thread(15);
    assert_eq!(app.active_thread_scrollback(), 15);

    // Switch to thread-beta
    app.select_thread("thread-beta");
    assert_eq!(
        app.active_thread_scrollback(),
        0,
        "New thread starts with its own scroll position"
    );

    // Scroll down 42 lines in thread-beta
    app.scroll_active_thread(42);
    assert_eq!(app.active_thread_scrollback(), 42);

    // Switch back to thread-alpha; position should be preserved
    app.select_thread("thread-alpha");
    assert_eq!(
        app.active_thread_scrollback(),
        15,
        "Scrollback position in thread-alpha must be preserved"
    );

    // Scroll up in thread-alpha with clamping to 0
    app.scroll_active_thread(-10);
    assert_eq!(app.active_thread_scrollback(), 5);

    app.scroll_active_thread(-20);
    assert_eq!(
        app.active_thread_scrollback(),
        0,
        "Scrollback cannot be negative"
    );

    // Switch back to thread-beta and verify unchanged
    app.select_thread("thread-beta");
    assert_eq!(
        app.active_thread_scrollback(),
        42,
        "Thread-beta scrollback must not be disturbed"
    );
}
