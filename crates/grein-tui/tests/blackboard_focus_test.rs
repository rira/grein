use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, PaneFocus};

#[test]
fn test_tab_focus_transition_into_blackboard() {
    let mut app = App::default();

    // Verify initial focus is on Composer or previous pane in focus cycle
    assert_ne!(app.focused_pane(), PaneFocus::Blackboard);

    // Cycle focus forward via Tab until reaching PaneFocus::Blackboard
    let mut reached_blackboard = false;
    for _ in 0..10 {
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        if app.focused_pane() == PaneFocus::Blackboard {
            reached_blackboard = true;
            break;
        }
    }
    assert!(
        reached_blackboard,
        "Tab key event must cycle focus into PaneFocus::Blackboard"
    );
}

#[test]
fn test_backtab_focus_transition_into_blackboard() {
    let mut app = App::default();

    // Cycle focus backward via BackTab (Shift+Tab or KeyCode::BackTab)
    let mut reached_blackboard = false;
    for _ in 0..10 {
        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        if app.focused_pane() == PaneFocus::Blackboard {
            reached_blackboard = true;
            break;
        }
    }
    assert!(
        reached_blackboard,
        "BackTab key event must cycle focus into PaneFocus::Blackboard in reverse"
    );
}

#[test]
fn test_blackboard_collapse_and_expand_keybindings() {
    let mut app = App::default();

    // Focus blackboard pane
    while app.focused_pane() != PaneFocus::Blackboard {
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    assert_eq!(app.focused_pane(), PaneFocus::Blackboard);

    // Initially, blackboard pane should be expanded
    assert!(
        !app.is_blackboard_collapsed(),
        "Blackboard pane should default to expanded"
    );

    // Trigger collapse key binding ('[' or Ctrl+B / toggle key)
    app.handle_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
    assert!(
        app.is_blackboard_collapsed(),
        "Blackboard pane must collapse upon toggle key press"
    );

    // Trigger expand key binding
    app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    assert!(
        !app.is_blackboard_collapsed(),
        "Blackboard pane must expand upon toggle key press"
    );
}

#[test]
fn test_collapsed_blackboard_focus_skip_or_retained() {
    let mut app = App::default();
    app.set_blackboard_collapsed(true);
    assert!(app.is_blackboard_collapsed());

    // When collapsed, Tab focus navigation handles transitioning into or cleanly skipping
    let initial = app.focused_pane();
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_ne!(app.focused_pane(), initial);
}
