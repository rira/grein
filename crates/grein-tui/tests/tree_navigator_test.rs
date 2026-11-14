use grein_tui::ui::navigator::{TreeNavigator, TreeNavigatorState, TreeNode};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

#[test]
fn test_tree_navigator_node_collapse_and_expand() {
    let root = TreeNode::branch(
        "thread-1",
        "Thread 1: Root Issue",
        vec![
            TreeNode::leaf("anchor-1-1", "Anchor: File diff line 42"),
            TreeNode::leaf("anchor-1-2", "Anchor: Diagnostic warning"),
        ],
    );

    let mut state = TreeNavigatorState::new(vec![root]);
    assert!(state.selected_id().is_some());
    assert_eq!(state.selected_id().unwrap(), "thread-1");
    assert!(state.is_expanded("thread-1"));

    // Toggle collapse
    state.toggle_expanded("thread-1");
    assert!(!state.is_expanded("thread-1"));

    // Visible items should only contain root when collapsed
    let visible = state.visible_nodes();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id(), "thread-1");

    // Toggle expand again
    state.toggle_expanded("thread-1");
    assert!(state.is_expanded("thread-1"));
    let visible = state.visible_nodes();
    assert_eq!(visible.len(), 3);
    assert_eq!(visible[1].id(), "anchor-1-1");
    assert_eq!(visible[2].id(), "anchor-1-2");
}

#[test]
fn test_tree_navigator_selection_navigation() {
    let nodes = vec![
        TreeNode::branch(
            "thread-1",
            "Thread 1",
            vec![TreeNode::leaf("anchor-1", "Anchor 1")],
        ),
        TreeNode::leaf("thread-2", "Thread 2"),
    ];

    let mut state = TreeNavigatorState::new(nodes);
    assert_eq!(state.selected_id().unwrap(), "thread-1");

    // Move next
    state.select_next();
    assert_eq!(state.selected_id().unwrap(), "anchor-1");

    state.select_next();
    assert_eq!(state.selected_id().unwrap(), "thread-2");

    // Reaching end should stay or wrap according to invariant
    state.select_next();
    assert_eq!(state.selected_id().unwrap(), "thread-2");

    // Move previous
    state.select_previous();
    assert_eq!(state.selected_id().unwrap(), "anchor-1");

    state.select_previous();
    assert_eq!(state.selected_id().unwrap(), "thread-1");
}

#[test]
fn test_tree_navigator_rendering_with_active_focus_borders() {
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();

    let nodes = vec![TreeNode::branch(
        "thread-1",
        "Thread: Login Flow",
        vec![TreeNode::leaf("anchor-1", "src/auth.rs:12")],
    )];
    let mut state = TreeNavigatorState::new(nodes);

    // Render when pane is active (focused = true)
    terminal
        .draw(|f| {
            let area = Rect::new(0, 0, 40, 10);
            let widget = TreeNavigator::default()
                .title("Threads & Anchors")
                .focused(true);
            f.render_stateful_widget(widget, area, &mut state);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    // Active focus should render title and focus indicator
    let content = buffer_to_string(buffer);
    assert!(content.contains("Threads & Anchors"));
    assert!(content.contains("Thread: Login Flow"));
    assert!(content.contains("src/auth.rs:12"));

    // Render when pane is inactive (focused = false)
    terminal
        .draw(|f| {
            let area = Rect::new(0, 0, 40, 10);
            let widget = TreeNavigator::default()
                .title("Threads & Anchors")
                .focused(false);
            f.render_stateful_widget(widget, area, &mut state);
        })
        .unwrap();

    let buffer_unfocused = terminal.backend().buffer();
    let content_unfocused = buffer_to_string(buffer_unfocused);
    assert!(content_unfocused.contains("Threads & Anchors"));
}

fn buffer_to_string(buffer: &Buffer) -> String {
    let mut result = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            result.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
        }
        result.push('\n');
    }
    result
}
