use grein_tui::app::{AppState, Pane};

#[test]
fn test_focusable_panes_when_blackboard_not_collapsed() {
    let mut state = AppState::default();
    state.blackboard_collapsed = false;

    let panes = state.focusable_panes();
    assert_eq!(
        panes,
        vec![
            Pane::Workspace,
            Pane::Blackboard,
            Pane::Chat,
            Pane::Composer,
        ]
    );
}

#[test]
fn test_focusable_panes_when_blackboard_collapsed() {
    let mut state = AppState::default();
    state.blackboard_collapsed = true;

    let panes = state.focusable_panes();
    assert_eq!(panes, vec![Pane::Workspace, Pane::Chat, Pane::Composer,]);
}

#[test]
fn test_focus_next_cycles_forward_with_blackboard() {
    let mut state = AppState::default();
    state.blackboard_collapsed = false;
    state.focus = Pane::Workspace;

    state.focus_next();
    assert_eq!(state.focus, Pane::Blackboard);

    state.focus_next();
    assert_eq!(state.focus, Pane::Chat);

    state.focus_next();
    assert_eq!(state.focus, Pane::Composer);

    state.focus_next();
    assert_eq!(state.focus, Pane::Workspace);
}

#[test]
fn test_focus_prev_cycles_backward_with_blackboard() {
    let mut state = AppState::default();
    state.blackboard_collapsed = false;
    state.focus = Pane::Workspace;

    state.focus_prev();
    assert_eq!(state.focus, Pane::Composer);

    state.focus_prev();
    assert_eq!(state.focus, Pane::Chat);

    state.focus_prev();
    assert_eq!(state.focus, Pane::Blackboard);

    state.focus_prev();
    assert_eq!(state.focus, Pane::Workspace);
}

#[test]
fn test_focus_next_skips_collapsed_blackboard() {
    let mut state = AppState::default();
    state.blackboard_collapsed = true;
    state.focus = Pane::Workspace;

    state.focus_next();
    assert_eq!(state.focus, Pane::Chat);

    state.focus_next();
    assert_eq!(state.focus, Pane::Composer);

    state.focus_next();
    assert_eq!(state.focus, Pane::Workspace);
}

#[test]
fn test_focus_prev_skips_collapsed_blackboard() {
    let mut state = AppState::default();
    state.blackboard_collapsed = true;
    state.focus = Pane::Chat;

    state.focus_prev();
    assert_eq!(state.focus, Pane::Workspace);

    state.focus_prev();
    assert_eq!(state.focus, Pane::Composer);

    state.focus_prev();
    assert_eq!(state.focus, Pane::Chat);
}

#[test]
fn test_focus_next_when_starting_on_collapsed_pane() {
    let mut state = AppState::default();
    state.focus = Pane::Blackboard;
    state.blackboard_collapsed = true;

    state.focus_next();
    assert_eq!(state.focus, Pane::Chat);
}
