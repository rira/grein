use grein_tui::{
    handle_key_event, AppState, ComposerAction, ComposerInput, FocusDirection, PaneId, TuiAction,
    TuiEvent,
};

#[test]
fn exports_pane_id_and_cycles_focus_Across_four_panes() {
    let mut state = AppState::default();
    assert_eq!(state.focused_pane(), PaneId::WorkspaceTree);

    state.cycle_focus(FocusDirection::Next);
    assert_eq!(state.focused_pane(), PaneId::SessionStream);

    state.cycle_focus(FocusDirection::Next);
    assert_eq!(state.focused_pane(), PaneId::DiffInspector);

    state.cycle_focus(FocusDirection::Next);
    assert_eq!(state.focused_pane(), PaneId::PromptComposer);

    state.cycle_focus(FocusDirection::Next);
    assert_eq!(state.focused_pane(), PaneId::WorkspaceTree);

    state.cycle_focus(FocusDirection::Previous);
    assert_eq!(state.focused_pane(), PaneId::PromptComposer);

    state.cycle_focus(FocusDirection::Previous);
    assert_eq!(state.focused_pane(), PaneId::DiffInspector);
}

#[test]
fn routes_composer_input_only_when_prompt_composer_is_focused() {
    let mut state = AppState::default();
    state.set_focus(PaneId::WorkspaceTree);

    let action = handle_key_event(&mut state, TuiEvent::Char('a'));
    assert_eq!(action, TuiAction::Ignored);
    assert_eq!(state.composer().buffer(), "");

    let tab_action = handle_key_event(&mut state, TuiEvent::Tab);
    assert_eq!(tab_action, TuiAction::FocusChanged(PaneId::SessionStream));

    let backtab_action = handle_key_event(&mut state, TuiEvent::BackTab);
    assert_eq!(
        backtab_action,
        TuiAction::FocusChanged(PaneId::WorkspaceTree)
    );

    state.set_focus(PaneId::PromptComposer);
    let char_action = handle_key_event(&mut state, TuiEvent::Char('h'));
    assert_eq!(
        char_action,
        TuiAction::Composer(ComposerAction::Updated("h".to_string()))
    );

    let char_action_2 = handle_key_event(&mut state, TuiEvent::Char('i'));
    assert_eq!(
        char_action_2,
        TuiAction::Composer(ComposerAction::Updated("hi".to_string()))
    );
    assert_eq!(state.composer().buffer(), "hi");

    let submit_action = handle_key_event(&mut state, TuiEvent::Submit);
    assert_eq!(
        submit_action,
        TuiAction::Composer(ComposerAction::Submitted("hi".to_string()))
    );
    assert_eq!(state.composer().buffer(), "");
}

#[test]
fn composer_input_supports_editing_and_rejection_of_empty_submission() {
    let mut composer = ComposerInput::new();
    assert_eq!(composer.buffer(), "");

    composer.insert_char('x');
    composer.insert_char('y');
    assert_eq!(composer.buffer(), "xy");

    composer.backspace();
    assert_eq!(composer.buffer(), "x");

    composer.clear();
    assert_eq!(composer.buffer(), "");
    assert_eq!(composer.submit(), None);
}
