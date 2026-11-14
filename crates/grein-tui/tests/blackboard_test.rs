use grein_tui::ui::focus::PaneFocus;
use grein_tui::views::blackboard::{BlackboardState, BlackboardView, DecisionItem};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

#[test]
fn test_blackboard_view_renders_inspector_and_decision_registry() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("failed to initialize test terminal");

    let mut state = BlackboardState::default();
    state.set_inspector_title("Global Inspector");
    state.set_inspector_detail("Active agent: autonomous-coder");
    state.add_decision(DecisionItem::new(
        "DEC-001",
        "Adopt Universal Two-Tier Architecture",
    ));
    state.add_decision(DecisionItem::new("DEC-002", "Enforce Zero-Null Policy"));

    let view = BlackboardView::new(&state).with_focus(PaneFocus::Blackboard);

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, 80, 24);
            frame.render_widget(view, area);
        })
        .expect("failed to render blackboard view");

    let buffer = terminal.backend().buffer();
    let content = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(
        content.contains("Global Inspector"),
        "Expected buffer to render inspector title"
    );
    assert!(
        content.contains("Active agent: autonomous-coder"),
        "Expected buffer to render inspector details"
    );
    assert!(
        content.contains("DEC-001"),
        "Expected buffer to render decision ID"
    );
    assert!(
        content.contains("Adopt Universal Two-Tier Architecture"),
        "Expected buffer to render decision description"
    );
    assert!(
        content.contains("DEC-002"),
        "Expected buffer to render second decision ID"
    );
}

#[test]
fn test_blackboard_view_collapsed_state_renders_indicator() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("failed to initialize test terminal");

    let mut state = BlackboardState::default();
    state.set_collapsed(true);
    state.set_inspector_title("Global Inspector");

    let view = BlackboardView::new(&state).with_focus(PaneFocus::Blackboard);

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, 80, 24);
            frame.render_widget(view, area);
        })
        .expect("failed to render collapsed blackboard view");

    let buffer = terminal.backend().buffer();
    let content = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(
        content.contains("Blackboard [Collapsed]") || content.contains("Blackboard (Collapsed)"),
        "Expected collapsed indicator when blackboard is collapsed"
    );
}
