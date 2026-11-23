use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, FocusPane};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn default_focus_starts_on_composer_and_toggles_cleanly_with_tree() {
    let mut app = App::default();
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusPane::Sidebar);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    // Idle Esc stays in WRITE mode; Ctrl+G enters MANAGE (F6/Alt+G dropped), and Esc in MANAGE returns to WRITE
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    assert!(!app.handle_key_event(key(KeyCode::F(6))));
    assert!(!app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT)));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusPane::Sidebar);

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.focused_pane(), FocusPane::Composer);
}

#[test]
fn tab_preserves_composer_focus_and_switches_sidebar_views_in_manage() {
    let mut app = App::default();
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.set_focused_pane(FocusPane::Sidebar);
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusPane::Sidebar);
}

#[test]
fn character_input_routes_to_composer_only_when_composer_focused() {
    let mut app = App::default();
    app.set_focused_pane(FocusPane::Sidebar);

    // Typing while Sidebar is focused does not mutate composer buffer
    app.handle_key_event(key(KeyCode::Char('x')));
    assert_eq!(app.composer_input(), "");

    // Focus Composer via Ctrl+G (Sidebar <-> Composer)
    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusPane::Composer);

    app.handle_key_event(key(KeyCode::Char('h')));
    app.handle_key_event(key(KeyCode::Char('i')));
    assert_eq!(app.composer_input(), "hi");

    app.handle_key_event(key(KeyCode::Backspace));
    assert_eq!(app.composer_input(), "h");
}

#[test]
fn quit_key_stops_app_only_in_manage_and_ctrl_q_does_not_hijack_composer() {
    let mut app = App::default();
    app.set_focused_pane(FocusPane::Composer);

    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.is_running());
    assert_eq!(app.composer_input(), "q");

    app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(app.is_running());

    let mut app2 = App::default();
    app2.set_focused_pane(FocusPane::Sidebar);
    app2.handle_key_event(key(KeyCode::Char('q')));
    assert!(!app2.is_running());
}
