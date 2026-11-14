use grein_tui::app::{AnchorItem, AppState, PaneFocus, ThreadItem};
use grein_tui::render::{render_app, render_markdown_lines, render_tree_view};
use grein_tui::ui::tree::{AnchorNode, ThreadNode, TreeNode, TreeState};
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

#[test]
fn test_render_tree_view_highlights_selected_row() {
    let mut state = AppState::new();
    state.active_focus = PaneFocus::Tree;
    state.threads = vec![ThreadItem {
        id: "t1".to_string(),
        title: "Main Thread".to_string(),
        anchors: vec![AnchorItem {
            id: "a1".to_string(),
            label: "Decision Anchor".to_string(),
        }],
    }];
    state.tree = TreeState::new(vec![
        TreeNode::Thread(ThreadNode {
            id: "t1".to_string(),
            title: "Main Thread".to_string(),
        }),
        TreeNode::Anchor(AnchorNode {
            id: "a1".to_string(),
            thread_id: "t1".to_string(),
            label: "Decision Anchor".to_string(),
        }),
    ]);
    state.tree.select_index(1);

    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).expect("terminal initialization should succeed");
    terminal
        .draw(|f| {
            let area = f.area();
            render_tree_view(f, area, &state);
        })
        .expect("drawing tree view should succeed");

    let buffer = terminal.backend().buffer();
    // Row 1 inside border (y = 1) is the thread row (unselected: BOLD, not REVERSED)
    // Row 2 inside border (y = 2) is the selected anchor row (selected_index = 1: Cyan, BOLD | REVERSED)
    let mut found_highlighted_anchor = false;
    for x in 1..59 {
        let cell = &buffer[(x, 2)];
        if cell.symbol().contains('D') || cell.fg == Color::Cyan {
            if cell.fg == Color::Cyan && cell.modifier.contains(Modifier::REVERSED) {
                found_highlighted_anchor = true;
                break;
            }
        }
    }
    assert!(
        found_highlighted_anchor,
        "Selected anchor row at index 1 must be highlighted with Cyan and REVERSED modifier"
    );
}

#[test]
fn test_render_markdown_lines_and_app_panes() {
    let lines = render_markdown_lines("# Heading 1\n- **Bold** and `code`");
    assert_eq!(lines.len(), 2);

    let mut state = AppState::new();
    state.active_focus = PaneFocus::Composer;
    state.input_buffer = "draft prompt".to_string();
    state.blackboard_notes = "Active Thread: test".to_string();
    state.active_transcript_md = "# User: hello".to_string();

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal initialization should succeed");
    terminal
        .draw(|f| render_app(f, &state))
        .expect("render_app should succeed");

    let buffer = terminal.backend().buffer();
    let rendered: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(rendered.contains("Tree Navigator"));
    assert!(rendered.contains("Transcript"));
    assert!(rendered.contains("Blackboard"));
    assert!(rendered.contains("Composer"));
    assert!(rendered.contains("draft prompt"));
}
