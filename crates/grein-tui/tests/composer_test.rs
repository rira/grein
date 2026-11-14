//! Black-box integration tests for the non-blocking prompt input composer state machine.
//! Enforces Universal Two-Tier Architecture and zero-null invariants.

use grein_tui::composer::{Composer, ComposerAction, ComposerEvent, ComposerState};

#[test]
fn test_initial_composer_state() {
    let composer = Composer::new();
    assert_eq!(composer.text(), "");
    assert_eq!(composer.cursor_position(), 0);
    assert_eq!(composer.char_count(), 0);
    assert_eq!(composer.state(), ComposerState::Idle);
    assert!(!composer.has_content());
}

#[test]
fn test_character_insertion_and_cursor_advancement() {
    let mut composer = Composer::new();

    composer.handle_event(ComposerEvent::InsertChar('H'));
    composer.handle_event(ComposerEvent::InsertChar('i'));

    assert_eq!(composer.text(), "Hi");
    assert_eq!(composer.cursor_position(), 2);
    assert_eq!(composer.char_count(), 2);
    assert!(composer.has_content());
}

#[test]
fn test_insert_string_at_cursor() {
    let mut composer = Composer::new();

    composer.handle_event(ComposerEvent::InsertText("hello world".to_string()));
    assert_eq!(composer.text(), "hello world");
    assert_eq!(composer.cursor_position(), 11);

    // Move cursor left by 6 positions to "hello "
    for _ in 0..6 {
        composer.handle_event(ComposerEvent::CursorLeft);
    }
    assert_eq!(composer.cursor_position(), 5);

    // Insert " brave new"
    composer.handle_event(ComposerEvent::InsertText(" brave new".to_string()));
    assert_eq!(composer.text(), "hello brave new world");
    assert_eq!(composer.cursor_position(), 15);
}

#[test]
fn test_backspace_deletion() {
    let mut composer = Composer::new();

    // Backspace on empty composer is a no-op
    let action = composer.handle_event(ComposerEvent::Backspace);
    assert_eq!(action, ComposerAction::None);
    assert_eq!(composer.text(), "");
    assert_eq!(composer.cursor_position(), 0);

    composer.handle_event(ComposerEvent::InsertText("Rust".to_string()));
    assert_eq!(composer.cursor_position(), 4);

    composer.handle_event(ComposerEvent::Backspace);
    assert_eq!(composer.text(), "Rus");
    assert_eq!(composer.cursor_position(), 3);

    // Move to middle and backspace
    composer.handle_event(ComposerEvent::CursorLeft);
    assert_eq!(composer.cursor_position(), 2);
    composer.handle_event(ComposerEvent::Backspace);
    assert_eq!(composer.text(), "Rs");
    assert_eq!(composer.cursor_position(), 1);
}

#[test]
fn test_delete_forward_character() {
    let mut composer = Composer::new();
    composer.handle_event(ComposerEvent::InsertText("abcd".to_string()));

    // Delete at end of text is a no-op
    let action = composer.handle_event(ComposerEvent::Delete);
    assert_eq!(action, ComposerAction::None);
    assert_eq!(composer.text(), "abcd");
    assert_eq!(composer.cursor_position(), 4);

    // Move cursor to start
    composer.handle_event(ComposerEvent::CursorHome);
    assert_eq!(composer.cursor_position(), 0);

    // Delete first character 'a'
    composer.handle_event(ComposerEvent::Delete);
    assert_eq!(composer.text(), "bcd");
    assert_eq!(composer.cursor_position(), 0);

    // Move to index 1 and delete 'c'
    composer.handle_event(ComposerEvent::CursorRight);
    composer.handle_event(ComposerEvent::Delete);
    assert_eq!(composer.text(), "bd");
    assert_eq!(composer.cursor_position(), 1);
}

#[test]
fn test_cursor_navigation_boundaries() {
    let mut composer = Composer::new();
    composer.handle_event(ComposerEvent::InsertText("test".to_string()));

    // Move left past beginning
    for _ in 0..10 {
        composer.handle_event(ComposerEvent::CursorLeft);
    }
    assert_eq!(composer.cursor_position(), 0);

    // Move right past end
    for _ in 0..10 {
        composer.handle_event(ComposerEvent::CursorRight);
    }
    assert_eq!(composer.cursor_position(), 4);

    // Home and End navigation
    composer.handle_event(ComposerEvent::CursorHome);
    assert_eq!(composer.cursor_position(), 0);

    composer.handle_event(ComposerEvent::CursorEnd);
    assert_eq!(composer.cursor_position(), 4);
}

#[test]
fn test_unicode_character_handling() {
    let mut composer = Composer::new();

    // Multi-byte Unicode graphemes (e.g. crab emoji, German umlaut, CJK)
    composer.handle_event(ComposerEvent::InsertChar('🦀'));
    composer.handle_event(ComposerEvent::InsertChar('ä'));
    composer.handle_event(ComposerEvent::InsertChar('字'));

    assert_eq!(composer.text(), "🦀ä字");
    assert_eq!(composer.char_count(), 3);
    assert_eq!(composer.cursor_position(), 3);

    // Backspace properly removes single Unicode char, not partial bytes
    composer.handle_event(ComposerEvent::Backspace);
    assert_eq!(composer.text(), "🦀ä");
    assert_eq!(composer.char_count(), 2);
    assert_eq!(composer.cursor_position(), 2);
}

#[test]
fn test_submission_event_with_content() {
    let mut composer = Composer::new();
    composer.handle_event(ComposerEvent::InsertText("execute task 42".to_string()));

    let action = composer.handle_event(ComposerEvent::Submit);
    assert_eq!(
        action,
        ComposerAction::Submitted("execute task 42".to_string())
    );

    // Verify state machine resets cleanly after submission
    assert_eq!(composer.text(), "");
    assert_eq!(composer.cursor_position(), 0);
    assert_eq!(composer.char_count(), 0);
    assert_eq!(composer.state(), ComposerState::Idle);
    assert!(!composer.has_content());
}

#[test]
fn test_submission_event_when_empty() {
    let mut composer = Composer::new();

    // Submitting empty prompt should produce no action and maintain idle state
    let action = composer.handle_event(ComposerEvent::Submit);
    assert_eq!(action, ComposerAction::None);
    assert_eq!(composer.state(), ComposerState::Idle);
}

#[test]
fn test_clear_event() {
    let mut composer = Composer::new();
    composer.handle_event(ComposerEvent::InsertText("draft prompt".to_string()));
    assert!(composer.has_content());

    let action = composer.handle_event(ComposerEvent::Clear);
    assert_eq!(action, ComposerAction::Cleared);
    assert_eq!(composer.text(), "");
    assert_eq!(composer.cursor_position(), 0);
    assert_eq!(composer.state(), ComposerState::Idle);
}
