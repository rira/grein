use grein_tui::app::{AppState, Pane};

#[test]
fn test_focusable_panes_includes_blackboard_when_not_collapsed() {
    let mut state = AppState::default();
    state.blackboard_collapsed = false;

    let panes = state.focusable_panes();
    assert!(
        panes.contains(&Pane::Blackboard),
        "Expected focusable_panes to include Pane::Blackboard when blackboard_collapsed is false"
    );
}

#[test]
fn test_focusable_panes_excludes_blackboard_when_collapsed() {
    let mut state = AppState::default();
    state.blackboard_collapsed = true;

    let panes = state.focusable_panes();
    assert!(
        !panes.contains(&Pane::Blackboard),
        "Expected focusable_panes to exclude Pane::Blackboard when blackboard_collapsed is true"
    );
}

#[test]
fn test_focusable_panes_preserves_relative_order_when_collapsed() {
    let mut state = AppState::default();
    state.blackboard_collapsed = false;
    let expanded_panes = state.focusable_panes();

    state.blackboard_collapsed = true;
    let collapsed_panes = state.focusable_panes();

    let expected: Vec<_> = expanded_panes
        .iter()
        .cloned()
        .filter(|pane| *pane != Pane::Blackboard)
        .collect();

    assert_eq!(
        collapsed_panes, expected,
        "Collapsed focusable panes must preserve the relative order of the remaining panes"
    );
}

#[test]
fn test_focusable_panes_non_empty_when_collapsed() {
    let mut state = AppState::default();
    state.blackboard_collapsed = true;

    let panes = state.focusable_panes();
    assert!(
        !panes.is_empty(),
        "Expected focusable_panes to not be empty even when blackboard is collapsed"
    );
}
