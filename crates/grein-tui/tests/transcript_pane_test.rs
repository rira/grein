use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::transcript::{TranscriptMessage, TranscriptPane};
use grein_tui::{AppAction, AppEvent, FocusDirection, PaneFocus};

#[test]
fn test_transcript_pane_exports_and_initial_state() {
    let pane = TranscriptPane::new();
    assert!(!pane.is_focused());
    assert_eq!(pane.message_count(), 0);
    assert_eq!(pane.scroll_offset(), 0);
}

#[test]
fn test_transcript_pane_focus_transitions() {
    let mut pane = TranscriptPane::new();

    // Initial state: not focused
    assert!(!pane.is_focused());

    // Route focus event entering Transcript pane
    let focus_event = AppEvent::FocusChanged(PaneFocus::Transcript);
    let handled = pane.handle_event(&focus_event);
    assert!(handled);
    assert!(pane.is_focused());

    // Route focus event transitioning away to Composer
    let unfocus_event = AppEvent::FocusChanged(PaneFocus::Composer);
    let handled = pane.handle_event(&unfocus_event);
    assert!(handled);
    assert!(!pane.is_focused());
}

#[test]
fn test_pane_focus_tab_and_backtab_navigation_order() {
    // 4-Pane navigation: Sidebar -> Transcript -> Composer -> Details -> Sidebar
    assert_eq!(
        PaneFocus::Sidebar.next(FocusDirection::Forward),
        PaneFocus::Transcript
    );
    assert_eq!(
        PaneFocus::Transcript.next(FocusDirection::Forward),
        PaneFocus::Composer
    );
    assert_eq!(
        PaneFocus::Composer.next(FocusDirection::Forward),
        PaneFocus::Details
    );
    assert_eq!(
        PaneFocus::Details.next(FocusDirection::Forward),
        PaneFocus::Sidebar
    );

    // Reverse (BackTab): Sidebar <- Transcript <- Composer <- Details <- Sidebar
    assert_eq!(
        PaneFocus::Transcript.next(FocusDirection::Backward),
        PaneFocus::Sidebar
    );
    assert_eq!(
        PaneFocus::Composer.next(FocusDirection::Backward),
        PaneFocus::Transcript
    );
    assert_eq!(
        PaneFocus::Details.next(FocusDirection::Backward),
        PaneFocus::Composer
    );
    assert_eq!(
        PaneFocus::Sidebar.next(FocusDirection::Backward),
        PaneFocus::Details
    );
}

#[test]
fn test_transcript_pane_event_routing_when_focused() {
    let mut pane = TranscriptPane::new();
    pane.handle_event(&AppEvent::FocusChanged(PaneFocus::Transcript));
    assert!(pane.is_focused());

    // Add sample messages
    pane.add_message(TranscriptMessage::user("Hello world"));
    pane.add_message(TranscriptMessage::assistant("Hello! How can I assist you?"));
    assert_eq!(pane.message_count(), 2);

    // When focused, scroll events (Up/Down) must be handled by TranscriptPane
    let scroll_down = AppEvent::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let action = pane.dispatch_key(&scroll_down);
    assert_eq!(action, Some(AppAction::Scrolled { delta: 1 }));

    let scroll_up = AppEvent::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let action = pane.dispatch_key(&scroll_up);
    assert_eq!(action, Some(AppAction::Scrolled { delta: -1 }));
}

#[test]
fn test_transcript_pane_ignores_key_events_when_unfocused() {
    let mut pane = TranscriptPane::new();
    // Ensure pane is not focused
    assert!(!pane.is_focused());

    pane.add_message(TranscriptMessage::user("Message"));

    // Unfocused pane should ignore navigation keys and return None
    let scroll_down = AppEvent::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let action = pane.dispatch_key(&scroll_down);
    assert_eq!(action, None);
}
