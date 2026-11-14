use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, FocusedPane};
use grein_tui::layout::{compute_layout, compute_layout_with_composer_lines};
use grein_tui::runner::{draw_frame, handle_key_event};
use ratatui::backend::TestBackend;
use ratatui::layout::{Position, Rect};
use ratatui::Terminal;

#[test]
fn compute_layout_allocates_34_46_20_percentages_when_expanded_and_34_66_when_collapsed() {
    let area = Rect::new(0, 0, 120, 40);
    let expanded = compute_layout(area, false);
    let collapsed = compute_layout(area, true);

    assert_eq!(expanded.tree_area.width, 41);
    assert_eq!(expanded.transcript_area.width, 55);
    assert_eq!(expanded.blackboard_area.expect("blackboard area").width, 24);
    assert!(expanded.transcript_area.width > expanded.tree_area.width);

    assert_eq!(collapsed.tree_area.width, 41);
    assert_eq!(collapsed.transcript_area.width, 79);
    assert!(collapsed.blackboard_area.is_none());
    assert_eq!(collapsed.tree_area.width, expanded.tree_area.width);
    assert_eq!(expanded.composer_area.height, 3);
}

#[test]
fn multiline_composer_auto_grows_height_and_tracks_2d_cursor() {
    let area = Rect::new(0, 0, 120, 40);
    assert_eq!(
        compute_layout_with_composer_lines(area, false, 1)
            .composer_area
            .height,
        3
    );
    assert_eq!(
        compute_layout_with_composer_lines(area, false, 2)
            .composer_area
            .height,
        4
    );
    assert_eq!(
        compute_layout_with_composer_lines(area, false, 3)
            .composer_area
            .height,
        5
    );
    assert_eq!(
        compute_layout_with_composer_lines(area, false, 12)
            .composer_area
            .height,
        10
    );

    let mut app = App::new();
    app.set_focus(FocusedPane::Composer);
    for ch in "Line 1".chars() {
        let _ = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        );
    }
    let _ = handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
    );
    for ch in "Line 2".chars() {
        let _ = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        );
    }
    let _ = handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    for ch in "Line 3".chars() {
        let _ = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        );
    }

    assert_eq!(app.composer_input(), "Line 1\nLine 2\nLine 3");
    assert_eq!(app.last_submitted_prompt(), None);
    assert_eq!(app.composer_line_count(), 3);
    assert_eq!(app.composer_cursor_2d(), (6, 2));

    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    draw_frame(&mut terminal, &app).expect("draw_frame");

    let layout = compute_layout_with_composer_lines(area, false, app.composer_line_count());
    assert_eq!(layout.composer_area.height, 5);
    let cursor_pos = terminal.get_cursor_position().expect("cursor");
    assert_eq!(
        cursor_pos,
        Position::new(layout.composer_area.x + 1 + 6, layout.composer_area.y + 1 + 2)
    );
}
