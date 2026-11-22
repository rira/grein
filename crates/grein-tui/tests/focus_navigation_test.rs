use grein_tui::app::{App, FocusPane};

#[test]
fn test_focus_next_toggles_composer_and_tree() {
    let mut app = App::default();
    assert_eq!(app.focus(), FocusPane::Composer);

    app.focus_next();
    assert_eq!(app.focus(), FocusPane::Tree);

    app.focus_next();
    assert_eq!(app.focus(), FocusPane::Composer);
}
