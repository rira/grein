use grein_tui::app::App;
use grein_tui::render::render_app;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn test_render_app_auto_grows_composer_for_multiline_input() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new();
    app.input_buffer = "first_line\nsecond_line\nthird_line\nfourth_line".to_string();

    terminal.draw(|f| render_app(f, &app)).unwrap();

    let buf = terminal.backend().buffer();
    let mut rendered = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            rendered.push_str(buf[(x, y)].symbol());
        }
        rendered.push('\n');
    }

    assert!(
        rendered.contains("first_line")
            && rendered.contains("second_line")
            && rendered.contains("third_line")
            && rendered.contains("fourth_line"),
        "Composer must auto-grow to display all 4 lines of multi-line input_buffer (render.rs:40 render.rs:120 render.rs:200)"
    );
}
