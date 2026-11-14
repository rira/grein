use grein_tui::app::{App, FocusPane};

#[test]
fn test_tab_cycles_focus_forward() {
    let mut app = App::default();
    assert_eq!(app.focus(), FocusPane::Tree);

    app.focus_next();
    assert_eq!(app.focus(), FocusPane::Transcript);

    app.focus_next();
    assert_eq!(app.focus(), FocusPane::Composer);

    app.focus_next();
    assert_eq!(app.focus(), FocusPane::Blackboard);

    app.focus_next();
    assert_eq!(app.focus(), FocusPane::Tree);
}
