use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use grein_tui::app::App;
use grein_tui::focus::PaneFocus;
use std::time::Duration;

#[test]
fn test_routes_key_events_to_composer_when_focused() {
    let mut app = App::new();
    app.set_focus(PaneFocus::Composer);
    assert_eq!(app.focus(), PaneFocus::Composer);

    // Send characters 'h', 'i'
    let key_h = KeyEvent {
        code: KeyCode::Char('h'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    let key_i = KeyEvent {
        code: KeyCode::Char('i'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };

    app.handle_key(key_h);
    app.handle_key(key_i);

    assert_eq!(app.composer_content(), "hi");
}

#[test]
fn test_does_not_route_text_keys_to_composer_when_inactive() {
    let mut app = App::new();
    // Set focus to a pane other than Composer (e.g. Output)
    app.set_focus(PaneFocus::Output);
    assert_ne!(app.focus(), PaneFocus::Composer);

    let key_a = KeyEvent {
        code: KeyCode::Char('a'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };

    app.handle_key(key_a);

    // Composer buffer must remain empty when not focused
    assert_eq!(app.composer_content(), "");
}

#[test]
fn test_composer_backspace_and_enter_handling() {
    let mut app = App::new();
    app.set_focus(PaneFocus::Composer);

    for c in "prompt".chars() {
        app.handle_key(KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
    }
    assert_eq!(app.composer_content(), "prompt");

    // Backspace removes character
    app.handle_key(KeyEvent {
        code: KeyCode::Backspace,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert_eq!(app.composer_content(), "promp");

    // Enter submits the prompt and clears the buffer
    app.handle_key(KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert_eq!(app.composer_content(), "");
    assert_eq!(app.last_submitted_prompt(), Some("promp".to_string()));
}

#[test]
fn test_non_blocking_event_polling_and_tick() {
    let mut app = App::new();
    app.set_focus(PaneFocus::Composer);

    // Non-blocking tick should succeed immediately without waiting or hanging
    let tick_result = app.poll_events(Duration::from_millis(0));
    assert!(tick_result.is_ok());

    // Composer state remains intact and responsive
    assert_eq!(app.composer_content(), "");
}
