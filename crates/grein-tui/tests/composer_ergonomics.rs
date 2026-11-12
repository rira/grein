use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use grein_tui::composer::{Composer, ComposerAction, ComposerMode, ThreadDraftStore, ThreadId};

fn make_key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

#[test]
fn test_single_enter_dispatches_turn_immediately() {
    let mut composer = Composer::new();
    composer.insert_text("Implement ingress router");

    let action = composer.handle_key(make_key_event(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        action,
        ComposerAction::DispatchTurn("Implement ingress router".to_string()),
        "Single Enter must dispatch turn immediately without multi-line newline insertion"
    );
    assert!(
        composer.is_empty(),
        "Composer draft must be cleared upon successful dispatch"
    );
}

#[test]
fn test_shift_enter_and_ctrl_enter_insert_newline() {
    let mut composer = Composer::new();
    composer.insert_text("Line 1");

    let action_shift = composer.handle_key(make_key_event(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(action_shift, ComposerAction::None);
    composer.insert_text("Line 2");

    let action_ctrl = composer.handle_key(make_key_event(KeyCode::Enter, KeyModifiers::CONTROL));
    assert_eq!(action_ctrl, ComposerAction::None);
    composer.insert_text("Line 3");

    assert_eq!(
        composer.draft_text(),
        "Line 1\nLine 2\nLine 3",
        "Shift+Enter and Ctrl+Enter must insert newlines for multi-line drafting"
    );
}

#[test]
fn test_draft_preservation_across_thread_switches() {
    let mut draft_store = ThreadDraftStore::new();
    let thread_a = ThreadId::from("thread-alpha");
    let thread_b = ThreadId::from("thread-beta");

    let mut composer = Composer::new();
    composer.insert_text("Uncommitted thought for thread A");
    draft_store.save_draft(&thread_a, composer.draft_text());

    composer.clear();
    composer.insert_text("Draft analysis for thread B");
    draft_store.save_draft(&thread_b, composer.draft_text());

    // Switch back to thread A
    composer.set_draft_text(draft_store.get_draft(&thread_a).unwrap_or_default());
    assert_eq!(
        composer.draft_text(),
        "Uncommitted thought for thread A",
        "Draft text must be preserved independently when switching back to thread A"
    );

    // Switch back to thread B
    composer.set_draft_text(draft_store.get_draft(&thread_b).unwrap_or_default());
    assert_eq!(
        composer.draft_text(),
        "Draft analysis for thread B",
        "Draft text must be preserved independently when switching back to thread B"
    );
}

#[test]
fn test_ctrl_e_triggers_external_editor_suspension_hook() {
    let mut composer = Composer::new();
    composer.insert_text("Initial outline");

    let action = composer.handle_key(make_key_event(KeyCode::Char('e'), KeyModifiers::CONTROL));
    match action {
        ComposerAction::SpawnExternalEditor(hook) => {
            assert_eq!(hook.initial_content(), "Initial outline");
            let updated_draft = "Initial outline\nRefined step 1\nRefined step 2";
            composer.apply_editor_result(updated_draft);
            assert_eq!(composer.draft_text(), updated_draft);
        }
        other => panic!("Expected SpawnExternalEditor action on Ctrl+E, got: {other:?}"),
    }
}

#[test]
fn test_contextual_reply_badge_display() {
    let mut composer = Composer::new();
    composer.set_reply_anchor(Some("#### 2. Ingress Routing".to_string()));

    assert_eq!(
        composer.reply_badge_text(),
        Some("[Replying to: #### 2. Ingress Routing]".to_string()),
        "Contextual reply badge must display targeted anchor formatted with prefix"
    );

    // Clear anchor on cancellation
    let action = composer.handle_key(make_key_event(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(action, ComposerAction::ClearReplyAnchor);
    composer.clear_reply_anchor();
    assert_eq!(composer.reply_badge_text(), None);
}

#[test]
fn test_o_key_hook_suspends_for_external_diff_pager() {
    let mut composer = Composer::new();
    composer.set_mode(ComposerMode::DiffPreviewActive {
        diff_id: "diff-turn-42".to_string(),
    });

    let action = composer.handle_key(make_key_event(KeyCode::Char('o'), KeyModifiers::NONE));
    match action {
        ComposerAction::SpawnDiffPager(hook) => {
            assert_eq!(hook.target_diff_id(), "diff-turn-42");
        }
        other => panic!(
            "Expected SpawnDiffPager action on 'o' key in DiffPreviewActive mode, got: {other:?}"
        ),
    }
}
