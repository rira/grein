use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::composer::{Composer, ComposerAction, ComposerEventResult};
use grein_tui::focus::PaneFocus;

#[test]
fn test_pane_focus_four_way_cycle_forward() {
    let initial = PaneFocus::Composer;
    let second = initial.next();
    let third = second.next();
    let fourth = third.next();
    let looped = fourth.next();

    assert_ne!(initial, second, "Focus must change on forward navigation");
    assert_ne!(second, third, "Focus must change to third pane");
    assert_ne!(third, fourth, "Focus must change to fourth pane");
    assert_eq!(
        initial, looped,
        "Focus cycle must return to initial pane after four steps"
    );
}

#[test]
fn test_pane_focus_four_way_cycle_backward() {
    let initial = PaneFocus::Composer;
    let prev = initial.prev();
    let prev_prev = prev.prev();
    let prev_prev_prev = prev_prev.prev();
    let looped = prev_prev_prev.prev();

    assert_ne!(initial, prev, "Focus must change on backward navigation");
    assert_eq!(
        initial, looped,
        "Focus reverse cycle must return to initial pane after four steps"
    );
    assert_eq!(
        initial.next().prev(),
        initial,
        "next() followed by prev() must be an identity"
    );
}

#[test]
fn test_pane_focus_default_and_variants() {
    let default_focus = PaneFocus::default();
    assert_eq!(
        default_focus,
        PaneFocus::Composer,
        "Composer must be default active pane"
    );

    let all_variants = [
        PaneFocus::Composer,
        PaneFocus::Sessions,
        PaneFocus::Chat,
        PaneFocus::Diff,
    ];

    for i in 0..all_variants.len() {
        for j in (i + 1)..all_variants.len() {
            assert_ne!(
                all_variants[i], all_variants[j],
                "All 4 pane focus variants must be distinct"
            );
        }
    }
}

#[test]
fn test_composer_input_buffer_insert_and_cursor_tracking() {
    let mut composer = Composer::new();
    assert_eq!(composer.text(), "");
    assert_eq!(composer.cursor(), 0);

    composer.insert_char('h');
    composer.insert_char('e');
    composer.insert_char('l');
    composer.insert_char('l');
    composer.insert_char('o');

    assert_eq!(composer.text(), "hello");
    assert_eq!(composer.cursor(), 5);

    composer.move_cursor_left();
    composer.move_cursor_left();
    assert_eq!(composer.cursor(), 3);

    composer.insert_char('p');
    assert_eq!(composer.text(), "helplo");
    assert_eq!(composer.cursor(), 4);
}

#[test]
fn test_composer_input_buffer_deletion_boundaries() {
    let mut composer = Composer::new();
    composer.insert_str("abc");
    assert_eq!(composer.cursor(), 3);

    // Delete backward
    composer.delete_char_before_cursor();
    assert_eq!(composer.text(), "ab");
    assert_eq!(composer.cursor(), 2);

    // Move to start and delete backward (must be safe no-op)
    composer.move_cursor_home();
    assert_eq!(composer.cursor(), 0);
    composer.delete_char_before_cursor();
    assert_eq!(composer.text(), "ab");
    assert_eq!(composer.cursor(), 0);

    // Forward delete at start
    composer.delete_char_after_cursor();
    assert_eq!(composer.text(), "b");
    assert_eq!(composer.cursor(), 0);

    // Move to end and forward delete (must be safe no-op)
    composer.move_cursor_end();
    assert_eq!(composer.cursor(), 1);
    composer.delete_char_after_cursor();
    assert_eq!(composer.text(), "b");
    assert_eq!(composer.cursor(), 1);
}

#[test]
fn test_composer_clear_and_multibyte_utf8() {
    let mut composer = Composer::new();
    composer.insert_str("🚀🦀");
    assert_eq!(composer.text(), "🚀🦀");
    assert_eq!(composer.char_count(), 2);

    composer.delete_char_before_cursor();
    assert_eq!(composer.text(), "🚀");
    assert_eq!(composer.char_count(), 1);

    composer.clear();
    assert_eq!(composer.text(), "");
    assert_eq!(composer.cursor(), 0);
    assert_eq!(composer.char_count(), 0);
}

#[test]
fn test_composer_event_routing_when_focused() {
    let mut composer = Composer::new();
    let key_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);

    let result = composer.handle_key_event(key_a, PaneFocus::Composer);
    assert_eq!(result, ComposerEventResult::Consumed);
    assert_eq!(composer.text(), "a");

    // Enter key submits and clears buffer
    let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let submit_result = composer.handle_key_event(key_enter, PaneFocus::Composer);

    assert_eq!(
        submit_result,
        ComposerEventResult::Action(ComposerAction::Submit("a".to_string()))
    );
    assert_eq!(composer.text(), "", "Buffer must clear after submission");
    assert_eq!(composer.cursor(), 0);
}

#[test]
fn test_composer_event_routing_ignored_when_unfocused() {
    let mut composer = Composer::new();
    composer.insert_str("existing");

    let key_char = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE);
    let result = composer.handle_key_event(key_char, PaneFocus::Chat);

    assert_eq!(result, ComposerEventResult::Ignored);
    assert_eq!(
        composer.text(),
        "existing",
        "Composer buffer must remain unmodified when pane is unfocused"
    );

    let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let enter_result = composer.handle_key_event(key_enter, PaneFocus::Diff);
    assert_eq!(enter_result, ComposerEventResult::Ignored);
    assert_eq!(composer.text(), "existing");
}

#[test]
fn test_composer_tab_and_backtab_delegation() {
    let mut composer = Composer::new();

    // Tab in composer must yield focus switch rather than inserting tab literal
    let key_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let tab_result = composer.handle_key_event(key_tab, PaneFocus::Composer);
    assert_eq!(
        tab_result,
        ComposerEventResult::Action(ComposerAction::NextPane)
    );
    assert_eq!(
        composer.text(),
        "",
        "Tab key must not be inserted into composer buffer"
    );

    // BackTab (Shift+Tab) must trigger backward focus switch
    let key_backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    let backtab_result = composer.handle_key_event(key_backtab, PaneFocus::Composer);
    assert_eq!(
        backtab_result,
        ComposerEventResult::Action(ComposerAction::PrevPane)
    );
    assert_eq!(composer.text(), "");
}
