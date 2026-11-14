use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::{
    AppEvent, AppState, FocusManager, InputEventResult, InputRouter, NavigationDirection,
    PaneFocus, TreeNode, TreeViewState,
};

#[test]
fn test_focus_cycle_transitions_forward_and_backward() {
    let mut focus_manager = FocusManager::new(PaneFocus::Tree);
    assert_eq!(focus_manager.current_focus(), PaneFocus::Tree);

    // Forward transition (Tab cycle)
    let next_focus = focus_manager.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_ne!(next_focus, PaneFocus::Tree);

    // Exhaustive cycle back to Tree
    let mut cycles = 1;
    while focus_manager.current_focus() != PaneFocus::Tree && cycles < 10 {
        focus_manager.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        cycles += 1;
    }
    assert_eq!(focus_manager.current_focus(), PaneFocus::Tree);
    assert_eq!(
        cycles, 4,
        "Focus cycle must transition deterministically across all 4 panes"
    );

    // Backward transition (BackTab / Shift+Tab)
    focus_manager.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_ne!(focus_manager.current_focus(), PaneFocus::Tree);

    // Reverse back to Tree
    focus_manager.focus_pane(PaneFocus::Tree);
    assert_eq!(focus_manager.current_focus(), PaneFocus::Tree);
}

#[test]
fn test_tree_pane_node_navigation_invariants() {
    let mut tree_state = TreeViewState::new();
    let root_nodes = vec![
        TreeNode::new("thread-1", "Main Discussion Thread"),
        TreeNode::new("thread-2", "Architecture Decision Review"),
        TreeNode::new("thread-3", "Implementation Notes"),
    ];
    tree_state.set_nodes(root_nodes);

    // Invariant: Initial selection defaults to root index 0
    assert_eq!(tree_state.selected_index(), Some(0));

    // Navigation invariant: Cannot navigate above upper boundary
    tree_state.navigate(NavigationDirection::Up);
    assert_eq!(tree_state.selected_index(), Some(0));

    // Navigation invariant: Sequential downward traversal
    tree_state.navigate(NavigationDirection::Down);
    assert_eq!(tree_state.selected_index(), Some(1));

    tree_state.navigate(NavigationDirection::Down);
    assert_eq!(tree_state.selected_index(), Some(2));

    // Navigation invariant: Cannot exceed lower boundary
    tree_state.navigate(NavigationDirection::Down);
    assert_eq!(tree_state.selected_index(), Some(2));

    // Navigation invariant: Sequential upward traversal
    tree_state.navigate(NavigationDirection::Up);
    assert_eq!(tree_state.selected_index(), Some(1));
}

#[test]
fn test_input_event_delegation_to_tree_when_focused() {
    let mut app_state = AppState::default();
    app_state.focus_manager.focus_pane(PaneFocus::Tree);

    let down_key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let result = InputRouter::route_key_event(&mut app_state, down_key);

    // When focused on Tree, arrow keys must be delegated and consumed by tree navigator
    assert_eq!(result, InputEventResult::Consumed);
    assert_eq!(app_state.tree_state.selected_index(), Some(1));
}

#[test]
fn test_input_event_delegation_bypasses_tree_when_unfocused() {
    let mut app_state = AppState::default();
    // Focus a pane other than Tree (e.g. Composer)
    app_state.focus_manager.focus_pane(PaneFocus::Composer);

    let initial_tree_selection = app_state.tree_state.selected_index();

    let key_event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let result = InputRouter::route_key_event(&mut app_state, key_event);

    // Keystroke must be delegated to the active Composer pane, leaving Tree state untouched
    assert_eq!(result, InputEventResult::Consumed);
    assert_eq!(
        app_state.tree_state.selected_index(),
        initial_tree_selection
    );
    assert!(app_state.composer_state.content().contains('j'));
}
