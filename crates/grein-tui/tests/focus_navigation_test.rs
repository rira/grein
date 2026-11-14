use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, FocusPane};

#[test]
fn test_tab_cycles_focus_forward() {
    let mut app = App::default();
    assert_eq!(app.focused_pane(), FocusPane::Tree);

    let tab_event = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

    app.handle_key_event(tab_event);
    assert_eq!(app.focused_pane(), FocusPane::Transcript);

    app.handle_key_event(tab_event);
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(tab_event);
    assert_eq!(app.focused_pane(), FocusPane::Blackboard);

    app.handle_key_event(tab_event);
    assert_eq!(app.focused_pane(), FocusPane::Tree);
}
