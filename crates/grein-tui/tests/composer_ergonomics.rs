use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::composer::{ComposerAction, InputComposer};

#[test]
fn test_single_enter_dispatches_input() {
    let mut composer = InputComposer::new();
    let thread_id = "thread-alpha";
    composer.set_active_thread(thread_id);

    // Input text "Deploy to staging"
    for ch in "Deploy to staging".chars() {
        composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    assert_eq!(composer.get_text(), "Deploy to staging");

    // Press Enter to dispatch
    let action = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        action,
        ComposerAction::Submit("Deploy to staging".to_string())
    );
    assert_eq!(composer.get_text(), "");
}

#[test]
fn test_ctrl_and_shift_enter_insert_newline() {
    let mut composer = InputComposer::new();
    composer.set_active_thread("thread-1");

    for ch in "First line".chars() {
        composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    // Shift+Enter should insert a newline without dispatching
    let shift_enter_action =
        composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(shift_enter_action, ComposerAction::None);

    for ch in "Second line".chars() {
        composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    // Ctrl+Enter should also insert a newline without dispatching
    let ctrl_enter_action =
        composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
    assert_eq!(ctrl_enter_action, ComposerAction::None);

    for ch in "Third line".chars() {
        composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    assert_eq!(composer.get_text(), "First line\nSecond line\nThird line");

    // Regular Enter dispatches the full multiline text
    let submit_action =
        composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        submit_action,
        ComposerAction::Submit("First line\nSecond line\nThird line".to_string())
    );
    assert_eq!(composer.get_text(), "");
}

#[test]
fn test_thread_isolated_unsubmitted_draft_persistence() {
    let mut composer = InputComposer::new();

    let thread_a = "thread-alpha";
    let thread_b = "thread-beta";

    // Draft text in Thread A
    composer.set_active_thread(thread_a);
    for ch in "Unfinished draft for Thread A".chars() {
        composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(composer.get_text(), "Unfinished draft for Thread A");

    // Switch to Thread B (should be empty initially)
    composer.set_active_thread(thread_b);
    assert_eq!(composer.get_text(), "");

    // Draft text in Thread B
    for ch in "Draft for Thread B".chars() {
        composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(composer.get_text(), "Draft for Thread B");

    // Switch back to Thread A (restores Thread A draft)
    composer.set_active_thread(thread_a);
    assert_eq!(composer.get_text(), "Unfinished draft for Thread A");

    // Submit Thread A's draft
    let action_a = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        action_a,
        ComposerAction::Submit("Unfinished draft for Thread A".to_string())
    );
    assert_eq!(composer.get_text(), "");

    // Switch back to Thread B (Thread B's draft is still preserved)
    composer.set_active_thread(thread_b);
    assert_eq!(composer.get_text(), "Draft for Thread B");
}
