use grein_tui::status::{RenderMode, Spinner, ThreadLifecycleStatus, ThreadStatusIndicator};

#[test]
fn test_unicode_glyphs_for_lifecycle_statuses() {
    let spinner = Spinner::new();
    assert_eq!(
        ThreadLifecycleStatus::Streaming.glyph(RenderMode::Unicode, &spinner),
        "⠋"
    );
    assert_eq!(
        ThreadLifecycleStatus::ExecutingTool.glyph(RenderMode::Unicode, &spinner),
        "⚙"
    );
    assert_eq!(
        ThreadLifecycleStatus::PermissionRequired.glyph(RenderMode::Unicode, &spinner),
        "⚠️"
    );
    assert_eq!(
        ThreadLifecycleStatus::AwaitingInput.glyph(RenderMode::Unicode, &spinner),
        "💬"
    );
    assert_eq!(
        ThreadLifecycleStatus::IdleCompleted.glyph(RenderMode::Unicode, &spinner),
        "✓"
    );
    assert_eq!(
        ThreadLifecycleStatus::Error.glyph(RenderMode::Unicode, &spinner),
        "✖"
    );
}

#[test]
fn test_ascii_fallback_indicators_for_lifecycle_statuses() {
    let spinner = Spinner::new();
    assert_eq!(
        ThreadLifecycleStatus::Streaming.glyph(RenderMode::Ascii, &spinner),
        "[*]"
    );
    assert_eq!(
        ThreadLifecycleStatus::ExecutingTool.glyph(RenderMode::Ascii, &spinner),
        "[>]"
    );
    assert_eq!(
        ThreadLifecycleStatus::PermissionRequired.glyph(RenderMode::Ascii, &spinner),
        "[!]"
    );
    assert_eq!(
        ThreadLifecycleStatus::AwaitingInput.glyph(RenderMode::Ascii, &spinner),
        "[?]"
    );
    assert_eq!(
        ThreadLifecycleStatus::IdleCompleted.glyph(RenderMode::Ascii, &spinner),
        "[-]"
    );
    assert_eq!(
        ThreadLifecycleStatus::Error.glyph(RenderMode::Ascii, &spinner),
        "[x]"
    );
}

#[test]
fn test_animated_spinner_cycles_frames() {
    let mut spinner = Spinner::new();
    let expected_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    for expected in expected_frames {
        assert_eq!(
            ThreadLifecycleStatus::Streaming.glyph(RenderMode::Unicode, &spinner),
            expected
        );
        spinner.tick();
    }

    assert_eq!(
        ThreadLifecycleStatus::Streaming.glyph(RenderMode::Unicode, &spinner),
        expected_frames[0]
    );
}

#[test]
fn test_thread_status_indicator_formatting_with_badges() {
    let spinner = Spinner::new();

    let default_indicator = ThreadStatusIndicator::new(ThreadLifecycleStatus::IdleCompleted);
    assert_eq!(default_indicator.render(RenderMode::Unicode, &spinner), "✓");
    assert_eq!(default_indicator.render(RenderMode::Ascii, &spinner), "[-]");

    let dirty_indicator =
        ThreadStatusIndicator::new(ThreadLifecycleStatus::IdleCompleted).with_dirty_workspace(true);
    assert_eq!(dirty_indicator.render(RenderMode::Unicode, &spinner), "✓*");
    assert_eq!(dirty_indicator.render(RenderMode::Ascii, &spinner), "[-]*");

    let unread_indicator =
        ThreadStatusIndicator::new(ThreadLifecycleStatus::Streaming).with_unread_events(true);
    assert_eq!(
        unread_indicator.render(RenderMode::Unicode, &spinner),
        "● ⠋"
    );
    assert_eq!(
        unread_indicator.render(RenderMode::Ascii, &spinner),
        "* [*]"
    );

    let fully_decorated = ThreadStatusIndicator::new(ThreadLifecycleStatus::ExecutingTool)
        .with_unread_events(true)
        .with_dirty_workspace(true);
    assert_eq!(
        fully_decorated.render(RenderMode::Unicode, &spinner),
        "● ⚙*"
    );
    assert_eq!(
        fully_decorated.render(RenderMode::Ascii, &spinner),
        "* [>]*"
    );
}
