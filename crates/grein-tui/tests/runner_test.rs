use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::FocusedPane;
use grein_tui::runner::handle_key_event;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn test_terminal_guard_raii_lifecycle() {
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

#[test]
fn test_golden_e2e_four_pane_integration_and_multiline_sqlite() {
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let mut app = grein_tui::state::AppState::new();

    // 1. Focus Composer and type prompt including brackets '[' and ']', then submit with Enter
    app.set_focus(FocusedPane::Composer);
    for ch in "Verify [4-pane] slice".chars() {
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        )
        .expect("char input should succeed");
    }
    assert!(
        !app.is_blackboard_collapsed(),
        "Typing '[' or ']' in Composer must not collapse Blackboard"
    );
    handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("Enter submit should succeed");

    // 2. Stream multi-line agent response and verify SQLite persistence updates multi-line turn
    let db_path = std::env::temp_dir().join(format!(
        "grein-golden-e2e-{}.sqlite3",
        grein_core::model::ThreadId::now_v7()
    ));
    let _ = std::fs::remove_file(&db_path);

    app.apply_agent_response("### Step 1: Tree & Transcript");
    app.persist_state_to_sqlite(&db_path);
    app.apply_agent_response("- Rendered all 4 panes cleanly");
    app.persist_state_to_sqlite(&db_path);

    let mut reloaded = grein_tui::state::AppState::new();
    reloaded.load_state_from_sqlite(&db_path);
    assert!(
        reloaded
            .active_transcript_md
            .contains("Rendered all 4 panes cleanly"),
        "Multi-line streamed agent output must persist across SQLite saves without truncating after line 1"
    );
    let _ = std::fs::remove_file(&db_path);

    // 3. Populate multi-line draft in Composer via Shift+Enter
    for ch in "Draft line 1".chars() {
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        )
        .unwrap();
    }
    handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
    for ch in "Draft line 2".chars() {
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        )
        .unwrap();
    }

    // 4. Render all 4 panes via draw_frame and verify non-empty expected text in all 4 panes
    grein_tui::runner::draw_frame(&mut terminal, &app).expect("draw_frame must succeed");

    let buf = terminal.backend().buffer();
    let mut screen = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            screen.push_str(buf[(x, y)].symbol());
        }
        screen.push('\n');
    }

    // Tree pane assertions
    assert!(
        screen.contains("Tree") && screen.contains("Verify [4-pane] slice"),
        "Tree pane must render thread title and anchors"
    );
    // Transcript pane assertions
    assert!(
        screen.contains("Transcript") && screen.contains("Rendered all 4 panes cleanly"),
        "Transcript pane must render markdown turns"
    );
    // Blackboard pane assertions
    assert!(
        screen.contains("Blackboard") && screen.contains("Active Thread:"),
        "Blackboard pane must render active thread notes"
    );
    // Composer pane & status bar assertions
    assert!(
        screen.contains("Composer")
            && screen.contains("Draft line 1")
            && screen.contains("Draft line 2")
            && screen.contains("Enter: Submit"),
        "Composer pane must auto-grow for multi-line draft and display keybinding status bar"
    );
}
