use grein_tui::app::{AnchorItem, AppState, FocusPane, ThreadItem};
use grein_tui::layout::compute_layout;
use grein_tui::render::render_app;
use grein_tui::runner::draw_frame;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

#[test]
fn compute_layout_splits_four_panes_when_blackboard_expanded() {
    let area = Rect::new(0, 0, 120, 40);
    let layout = compute_layout(area, false);

    assert_eq!(layout.composer_area.height, 3);
    assert_eq!(layout.composer_area.y, 37);
    assert_eq!(layout.tree_area.height, 37);
    assert_eq!(layout.transcript_area.height, 37);
    assert!(layout.blackboard_area.is_some());

    let bb = layout.blackboard_area.unwrap();
    assert_eq!(bb.height, 37);
    assert!(layout.tree_area.width > 0);
    assert!(layout.transcript_area.width > layout.tree_area.width);
    assert!(bb.width > 0);
}

#[test]
fn compute_layout_hides_blackboard_when_collapsed() {
    let area = Rect::new(0, 0, 120, 40);
    let expanded = compute_layout(area, false);
    let collapsed = compute_layout(area, true);

    assert!(collapsed.blackboard_area.is_none());
    assert_eq!(collapsed.composer_area.height, 3);
    assert_eq!(collapsed.tree_area.width, expanded.tree_area.width);
    assert!(collapsed.transcript_area.width > expanded.transcript_area.width);
}

#[test]
fn render_app_draws_four_panes_with_expanded_blackboard() {
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("terminal init");

    let mut app = AppState::new();
    app.threads = vec![ThreadItem {
        id: "t-1".to_string(),
        title: "feature/tui".to_string(),
        anchors: vec![AnchorItem {
            id: "a-1".to_string(),
            label: "Pinned architecture invariant".to_string(),
        }],
    }];
    app.focused_pane = FocusPane::Composer;

    terminal
        .draw(|f| render_app(f, &app))
        .expect("render frame");

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();

    assert!(content.contains("feature/tui"));
    assert!(content.contains("Pinned architecture invariant"));
}

#[test]
fn draw_frame_renders_collapsed_blackboard_state() {
    let backend_expanded = TestBackend::new(120, 40);
    let mut terminal_expanded = Terminal::new(backend_expanded).expect("terminal init");
    let mut app_expanded = AppState::new();
    app_expanded.blackboard_collapsed = false;
    draw_frame(&mut terminal_expanded, &app_expanded).expect("draw_frame expanded");
    let expanded_content: String = terminal_expanded
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();

    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("terminal init");

    let mut app = AppState::new();
    app.blackboard_collapsed = true;
    app.threads = vec![ThreadItem {
        id: "t-2".to_string(),
        title: "collapsed-thread".to_string(),
        anchors: vec![],
    }];

    draw_frame(&mut terminal, &app).expect("draw_frame succeeds");

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();

    assert!(content.contains("collapsed-thread"));
    assert_ne!(content, expanded_content);
}
