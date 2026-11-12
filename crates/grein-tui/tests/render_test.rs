use grein_tui::app::{AnchorItem, AppState, PaneFocus, ThreadItem};
use grein_tui::render::{render_app, render_markdown_lines, render_tree_view};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

#[test]
fn test_render_tree_navigator_displays_hierarchy() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("failed to initialize test terminal");

    let mut state = AppState::default();
    state.threads = vec![ThreadItem {
        id: "t-1".to_string(),
        title: "Root Thread".to_string(),
        anchors: vec![
            AnchorItem {
                id: "a-1".to_string(),
                label: "Anchor 1".to_string(),
            },
            AnchorItem {
                id: "a-2".to_string(),
                label: "Anchor 2".to_string(),
            },
        ],
    }];

    terminal
        .draw(|f| {
            let area = Rect::new(0, 0, 25, 20);
            render_tree_view(f, area, &state);
        })
        .expect("draw tree view should succeed");

    let buffer = terminal.backend().buffer();
    let rendered = buffer
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect::<Vec<_>>()
        .join("");

    assert!(
        rendered.contains("Root Thread"),
        "Tree must render thread title"
    );
    assert!(
        rendered.contains("Anchor 1"),
        "Tree must render nested anchor items"
    );
}

#[test]
fn test_render_transcript_markdown_formatting() {
    let markdown = "# Header 1\n\n**Bold text** and `code`\n\n- item 1\n- item 2";
    let lines = render_markdown_lines(markdown);

    assert!(!lines.is_empty(), "Markdown output lines must not be empty");

    let plain_text: String = lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
        .collect();

    assert!(
        plain_text.contains("Header 1"),
        "Rendered text should retain header"
    );
    assert!(
        plain_text.contains("Bold text"),
        "Rendered text should retain bold content"
    );
    assert!(
        plain_text.contains("code"),
        "Rendered text should retain inline code"
    );
    assert!(
        plain_text.contains("item 1"),
        "Rendered text should retain list items"
    );
}

#[test]
fn test_render_full_layout_contains_all_four_panes() {
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("failed to initialize test terminal");

    let mut state = AppState::default();
    state.active_focus = PaneFocus::Transcript;
    state.active_transcript_md = "## Active Discussion\nSystem prompt initialized.".to_string();
    state.blackboard_notes = "Shared blackboard state: OK".to_string();
    state.input_buffer = "echo test input".to_string();

    terminal
        .draw(|f| {
            render_app(f, &state);
        })
        .expect("draw app should succeed");

    let buffer = terminal.backend().buffer();
    let rendered_text = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<Vec<_>>()
        .join("");

    assert!(
        rendered_text.contains("Active Discussion"),
        "Transcript pane must be rendered"
    );
    assert!(
        rendered_text.contains("Shared blackboard"),
        "Blackboard pane must be rendered"
    );
    assert!(
        rendered_text.contains("echo test input"),
        "Input composer pane must be rendered"
    );
}

#[test]
fn test_render_collapsible_blackboard() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("failed to initialize test terminal");

    let mut state = AppState::default();
    state.blackboard_collapsed = true;
    state.blackboard_notes = "Secret blackboard content".to_string();

    terminal
        .draw(|f| {
            render_app(f, &state);
        })
        .expect("draw app should succeed with collapsed blackboard");

    let buffer = terminal.backend().buffer();
    let rendered_text = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<Vec<_>>()
        .join("");

    assert!(
        !rendered_text.contains("Secret blackboard content"),
        "Collapsed blackboard notes must not be rendered"
    );
}
