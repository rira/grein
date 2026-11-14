use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, FocusPane};

#[test]
fn focus_pane_cycles_forward_through_all_four_panes() {
    let mut app = App::new();
    assert_eq!(app.focused_pane(), FocusPane::WorkspaceTree);

    app.focus_next();
    assert_eq!(app.focused_pane(), FocusPane::ChatStream);

    app.focus_next();
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.focus_next();
    assert_eq!(app.focused_pane(), FocusPane::ContextInspector);

    app.focus_next();
    assert_eq!(app.focused_pane(), FocusPane::WorkspaceTree);
}

#[test]
fn focus_pane_cycles_backward_through_all_four_panes() {
    let mut app = App::new();
    assert_eq!(app.focused_pane(), FocusPane::WorkspaceTree);

    app.focus_prev();
    assert_eq!(app.focused_pane(), FocusPane::ContextInspector);

    app.focus_prev();
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.focus_prev();
    assert_eq!(app.focused_pane(), FocusPane::ChatStream);

    app.focus_prev();
    assert_eq!(app.focused_pane(), FocusPane::WorkspaceTree);
}

#[test]
fn handle_key_event_tab_and_backtab_navigate_focus() {
    let mut app = App::new();
    assert_eq!(app.focused_pane(), FocusPane::WorkspaceTree);

    let tab_event = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    app.handle_key_event(tab_event);
    assert_eq!(app.focused_pane(), FocusPane::ChatStream);

    app.handle_key_event(tab_event);
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    let backtab_event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    app.handle_key_event(backtab_event);
    assert_eq!(app.focused_pane(), FocusPane::ChatStream);

    app.handle_key_event(backtab_event);
    assert_eq!(app.focused_pane(), FocusPane::WorkspaceTree);

    app.handle_key_event(backtab_event);
    assert_eq!(app.focused_pane(), FocusPane::ContextInspector);
}

#[test]
fn focus_pane_enum_next_and_prev_methods_are_symmetric() {
    let panes = [
        FocusPane::WorkspaceTree,
        FocusPane::ChatStream,
        FocusPane::Composer,
        FocusPane::ContextInspector,
    ];

    for pane in panes {
        assert_eq!(pane.next().prev(), pane);
        assert_eq!(pane.prev().next(), pane);
    }
}
