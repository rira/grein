use grein_tui::pane::blackboard::{BlackboardState, InspectionTarget};
use grein_tui::pane::focus::PaneFocus;

#[test]
fn test_pane_focus_blackboard_variant_exists() {
    let focus = PaneFocus::Blackboard;
    assert_eq!(focus, PaneFocus::Blackboard);
    assert_eq!(focus.name(), "Blackboard");
}

#[test]
fn test_blackboard_state_default_initialization() {
    let state = BlackboardState::default();

    assert!(
        state.is_visible(),
        "Blackboard pane should be visible by default"
    );
    assert_eq!(state.inspection_target(), &InspectionTarget::None);
    assert_eq!(state.decision_registry_selection(), None);
    assert_eq!(state.decision_registry_scroll_offset(), 0);
}

#[test]
fn test_blackboard_state_collapsible_visibility_toggle() {
    let mut state = BlackboardState::new();
    assert!(state.is_visible());

    state.toggle_visibility();
    assert!(
        !state.is_visible(),
        "Blackboard pane should be collapsed/hidden after toggle"
    );

    state.set_visible(true);
    assert!(state.is_visible());

    state.set_visible(false);
    assert!(!state.is_visible());
}

#[test]
fn test_blackboard_state_inspection_target_tracking() {
    let mut state = BlackboardState::new();
    assert_eq!(state.inspection_target(), &InspectionTarget::None);

    let ticket_target =
        InspectionTarget::Ticket("01a1d8a2-9617-7003-8945-0076ae34055e".to_string());
    state.set_inspection_target(ticket_target.clone());
    assert_eq!(state.inspection_target(), &ticket_target);

    let node_target = InspectionTarget::Node("worker-node-1".to_string());
    state.set_inspection_target(node_target.clone());
    assert_eq!(state.inspection_target(), &node_target);

    state.clear_inspection_target();
    assert_eq!(state.inspection_target(), &InspectionTarget::None);
}

#[test]
fn test_blackboard_decision_registry_scroll_and_selection() {
    let mut state = BlackboardState::new();

    // Populate decision item count
    state.set_decision_item_count(5);
    assert_eq!(state.decision_item_count(), 5);

    // Test initial selection
    state.select_decision(Some(0));
    assert_eq!(state.decision_registry_selection(), Some(0));

    // Test moving selection down
    state.select_next_decision();
    assert_eq!(state.decision_registry_selection(), Some(1));

    // Test moving selection up
    state.select_previous_decision();
    assert_eq!(state.decision_registry_selection(), Some(0));

    // Test bounds on previous
    state.select_previous_decision();
    assert_eq!(state.decision_registry_selection(), Some(0));

    // Test scroll offsets
    state.set_decision_registry_scroll_offset(3);
    assert_eq!(state.decision_registry_scroll_offset(), 3);

    state.scroll_decision_down(2);
    assert_eq!(state.decision_registry_scroll_offset(), 5);

    state.scroll_decision_up(4);
    assert_eq!(state.decision_registry_scroll_offset(), 1);
}
