use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn test_terminal_guard_raii_lifecycle() {
    // Verify that TerminalGuard can be instantiated and dropped cleanly without panicking.
    let guard = grein_tui::runner::TerminalGuard::new();
    assert!(guard.is_ok());
    drop(guard);
}

#[tokio::test]
async fn test_runner_event_loop_quits_on_q_key() {
    let backend = TestBackend::new(80, 24);
    let _terminal = Terminal::new(backend).expect("failed to create test terminal");
    let mut app_state = grein_tui::state::AppState::new();

    let quit_event =
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

    let exit_requested = grein_tui::runner::handle_event(&mut app_state, quit_event)
        .expect("handling event should succeed");

    assert!(exit_requested, "'q' key must signal event loop termination");
}

#[tokio::test]
async fn test_runner_event_loop_quits_on_ctrl_c() {
    let mut app_state = grein_tui::state::AppState::new();

    let ctrl_c_event =
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

    let exit_requested = grein_tui::runner::handle_event(&mut app_state, ctrl_c_event)
        .expect("handling event should succeed");

    assert!(exit_requested, "Ctrl+C must signal event loop termination");
}

#[tokio::test]
async fn test_runner_render_cycle_with_test_backend() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let app_state = grein_tui::state::AppState::new();

    let render_result = grein_tui::runner::draw_frame(&mut terminal, &app_state);
    assert!(
        render_result.is_ok(),
        "rendering frame on test backend must succeed"
    );

    let buffer = terminal.backend().buffer();
    assert!(buffer.area.width == 100);
    assert!(buffer.area.height == 30);
}
