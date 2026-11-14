//! Black-box tests for `PaneFocus::Blackboard`.
//! Verifies pane collapse/expand state transitions, focus delegation, and event routing invariants.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, AppConfig, AppEvent};
use grein_tui::layout::PaneFocus;
use grein_tui::panes::blackboard::{BlackboardAction, BlackboardPane, BlackboardState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn test_blackboard_collapse_expand_transitions() {
    let mut blackboard = BlackboardPane::new();
    assert_eq!(blackboard.state(), BlackboardState::Expanded);
    assert!(!blackboard.is_collapsed());

    // Transition from Expanded to Collapsed
    blackboard.handle_action(BlackboardAction::ToggleCollapse);
    assert_eq!(blackboard.state(), BlackboardState::Collapsed);
    assert!(blackboard.is_collapsed());

    // Explicit Collapse (idempotent)
    blackboard.handle_action(BlackboardAction::Collapse);
    assert_eq!(blackboard.state(), BlackboardState::Collapsed);

    // Transition from Collapsed to Expanded
    blackboard.handle_action(BlackboardAction::Expand);
    assert_eq!(blackboard.state(), BlackboardState::Expanded);
    assert!(!blackboard.is_collapsed());
}

#[test]
fn test_focus_delegation_when_focused_blackboard_collapses() {
    let config = AppConfig::default();
    let mut app = App::new();

    // Focus blackboard pane
    app.set_focus(PaneFocus::Blackboard);
    assert_eq!(app.focused_pane(), PaneFocus::Blackboard);

    // Collapsing blackboard pane while focused must delegate focus away to Composer or Workspace
    app.handle_event(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('b'),
        KeyModifiers::CONTROL,
    )));

    assert!(app.blackboard_pane().is_collapsed());
    assert_ne!(
        app.focused_pane(),
        PaneFocus::Blackboard,
        "Focus must be delegated away from Blackboard when it is collapsed"
    );
    assert!(
        app.focused_pane() == PaneFocus::Composer || app.focused_pane() == PaneFocus::Workspace,
        "Focus must fall back to primary operational pane"
    );
}

#[test]
fn test_tab_navigation_skips_collapsed_blackboard_pane() {
    let config = AppConfig::default();
    let mut app = App::new();

    // Collapse the blackboard pane
    app.blackboard_pane_mut().collapse();
    assert!(app.blackboard_pane().is_collapsed());

    // Cycle focus through Tab key multiple times and ensure Blackboard is never selected
    for _ in 0..10 {
        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        assert_ne!(
            app.focused_pane(),
            PaneFocus::Blackboard,
            "Collapsed blackboard pane must be skipped during forward Tab focus navigation"
        );
    }

    // Cycle focus backwards with BackTab
    for _ in 0..10 {
        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
        )));
        assert_ne!(
            app.focused_pane(),
            PaneFocus::Blackboard,
            "Collapsed blackboard pane must be skipped during BackTab focus navigation"
        );
    }
}

#[test]
fn test_event_routing_invariants_focused_vs_collapsed() {
    let config = AppConfig::default();
    let mut app = App::new();

    // When expanded and focused, blackboard consumes inspector navigation events
    app.blackboard_pane_mut().expand();
    app.set_focus(PaneFocus::Blackboard);

    let nav_event = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let handled = app.handle_key_event(nav_event);
    assert!(
        handled,
        "Focused expanded blackboard must consume arrow key events"
    );

    // When collapsed, blackboard must reject event routing and not consume events
    app.blackboard_pane_mut().collapse();
    let unhandled = app.blackboard_pane_mut().handle_key(nav_event);
    assert!(
        !unhandled,
        "Collapsed blackboard pane must never consume navigation events"
    );
}

#[test]
fn test_blackboard_pane_render_snapshot_layout() {
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("Failed to initialize TestBackend");
    let config = AppConfig::default();
    let mut app = App::new();

    // Render expanded
    app.blackboard_pane_mut().expand();
    terminal
        .draw(|f| {
            app.render(f, f.size());
        })
        .expect("Failed to render expanded app");

    let expanded_rect = app.blackboard_pane().rendered_rect();
    assert!(
        expanded_rect.width > 0,
        "Expanded blackboard must have non-zero render width"
    );

    // Render collapsed
    app.blackboard_pane_mut().collapse();
    terminal
        .draw(|f| {
            app.render(f, f.size());
        })
        .expect("Failed to render collapsed app");

    let collapsed_rect = app.blackboard_pane().rendered_rect();
    assert_eq!(
        collapsed_rect.width, 0,
        "Fully collapsed blackboard pane must have width 0 in primary split layout"
    );
}
