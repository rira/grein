use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, FocusPane};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn default_focus_starts_on_sidebar_and_cycles_forward_through_four_panes() {
    let mut app = App::default();
    assert_eq!(app.focused_pane(), FocusPane::Sidebar);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusPane::Transcript);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusPane::Inspector);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusPane::Sidebar);
}

#[test]
fn backtab_cycles_focus_backward_through_four_panes() {
    let mut app = App::default();
    assert_eq!(app.focused_pane(), FocusPane::Sidebar);

    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.focused_pane(), FocusPane::Inspector);

    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.focused_pane(), FocusPane::Transcript);

    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.focused_pane(), FocusPane::Sidebar);
}

#[test]
fn character_input_routes_to_composer_only_when_composer_focused() {
    let mut app = App::default();

    // Typing while Sidebar is focused does not mutate composer buffer
    app.handle_key_event(key(KeyCode::Char('x')));
    assert_eq!(app.composer_input(), "");

    // Focus Composer (Sidebar -> Transcript -> Composer)
    app.handle_key_event(key(KeyCode::Tab));
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(key(KeyCode::Char('h')));
    app.handle_key_event(key(KeyCode::Char('i')));
    assert_eq!(app.composer_input(), "hi");

    app.handle_key_event(key(KeyCode::Backspace));
    assert_eq!(app.composer_input(), "h");
}

#[test]
fn quit_key_stops_app_only_outside_composer_focus() {
    let mut app = App::default();
    app.set_focused_pane(FocusPane::Composer);

    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.is_running());
    assert_eq!(app.composer_input(), "q");

    app.set_focused_pane(FocusPane::Sidebar);
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(!app.is_running());
}
