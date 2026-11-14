use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use grein_tui::ui::tree::{AnchorNode, ThreadNode, TreeNode, TreeState};
use grein_tui::{handle_key_event, AppState, PaneFocus, TreeAction};

fn make_key_event(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn sample_tree_state() -> TreeState {
    let nodes = vec![
        TreeNode::Thread(ThreadNode {
            id: "thread-1".to_string(),
            title: "Initial Thread".to_string(),
        }),
        TreeNode::Anchor(AnchorNode {
            id: "anchor-1".to_string(),
            thread_id: "thread-1".to_string(),
            label: "Anchor 1: Setup Task".to_string(),
        }),
        TreeNode::Anchor(AnchorNode {
            id: "anchor-2".to_string(),
            thread_id: "thread-1".to_string(),
            label: "Anchor 2: Verification".to_string(),
        }),
    ];
    TreeState::new(nodes)
}

#[test]
fn test_tree_navigation_down_traversal_when_focused() {
    let mut state = AppState::new();
    state.focus = PaneFocus::Tree;
    state.tree = sample_tree_state();
    state.tree.select_index(0);

    let key_down = make_key_event(KeyCode::Down);
    let result = handle_key_event(&mut state, key_down);

    assert!(
        result.is_ok(),
        "Input handling should succeed without error"
    );
    assert_eq!(
        state.tree.selected_index(),
        Some(1),
        "Down key should advance selection to the next item when Tree is focused"
    );

    let key_j = make_key_event(KeyCode::Char('j'));
    let result_j = handle_key_event(&mut state, key_j);
    assert!(result_j.is_ok());
    assert_eq!(
        state.tree.selected_index(),
        Some(2),
        "'j' key should advance selection when Tree is focused"
    );
}

#[test]
fn test_tree_navigation_up_traversal_when_focused() {
    let mut state = AppState::new();
    state.focus = PaneFocus::Tree;
    state.tree = sample_tree_state();
    state.tree.select_index(2);

    let key_up = make_key_event(KeyCode::Up);
    let result = handle_key_event(&mut state, key_up);

    assert!(result.is_ok());
    assert_eq!(
        state.tree.selected_index(),
        Some(1),
        "Up key should move selection to the previous item when Tree is focused"
    );

    let key_k = make_key_event(KeyCode::Char('k'));
    let result_k = handle_key_event(&mut state, key_k);
    assert!(result_k.is_ok());
    assert_eq!(
        state.tree.selected_index(),
        Some(0),
        "'k' key should move selection to the previous item when Tree is focused"
    );
}

#[test]
fn test_tree_navigation_boundary_clamping() {
    let mut state = AppState::new();
    state.focus = PaneFocus::Tree;
    state.tree = sample_tree_state();
    state.tree.select_index(0);

    let key_up = make_key_event(KeyCode::Up);
    let result = handle_key_event(&mut state, key_up);
    assert!(result.is_ok());
    assert_eq!(
        state.tree.selected_index(),
        Some(0),
        "Up key at root/first element should remain clamped at 0"
    );

    state.tree.select_index(2);
    let key_down = make_key_event(KeyCode::Down);
    let result_down = handle_key_event(&mut state, key_down);
    assert!(result_down.is_ok());
    assert_eq!(
        state.tree.selected_index(),
        Some(2),
        "Down key at terminal element should remain clamped at max index"
    );
}

#[test]
fn test_anchor_activation_on_enter() {
    let mut state = AppState::new();
    state.focus = PaneFocus::Tree;
    state.tree = sample_tree_state();
    state.tree.select_index(1); // Anchor 1

    let key_enter = make_key_event(KeyCode::Enter);
    let action = handle_key_event(&mut state, key_enter).expect("Enter should route cleanly");

    assert_eq!(
        action,
        Some(TreeAction::ActivateAnchor("anchor-1".to_string())),
        "Enter key on an anchor node must emit ActivateAnchor action"
    );
    assert_eq!(
        state.tree.active_anchor_id(),
        Some("anchor-1"),
        "Tree state should record active anchor on activation"
    );
}

#[test]
fn test_tree_ignores_traversal_when_not_focused() {
    let mut state = AppState::new();
    state.focus = PaneFocus::Composer;
    state.tree = sample_tree_state();
    state.tree.select_index(0);

    let key_down = make_key_event(KeyCode::Down);
    let _ = handle_key_event(&mut state, key_down);

    assert_eq!(
        state.tree.selected_index(),
        Some(0),
        "Tree traversal must NOT occur when PaneFocus is not Tree"
    );
}
