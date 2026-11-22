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
async fn test_runner_event_loop_quits_on_q_key_outside_composer_and_ctrl_q_anywhere() {
    let backend = TestBackend::new(80, 24);
    let _terminal = Terminal::new(backend).expect("failed to create test terminal");
    let mut app_state = grein_tui::state::AppState::new();
    assert_eq!(app_state.focused_pane(), FocusedPane::Composer);

    let q_in_composer =
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    let exit_in_composer = grein_tui::runner::handle_event(&mut app_state, q_in_composer)
        .expect("handling event should succeed");
    assert!(
        !exit_in_composer,
        "'q' inside Composer must type 'q' rather than quitting"
    );

    let ctrl_q =
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    let exit_ctrl_q = grein_tui::runner::handle_event(&mut app_state, ctrl_q)
        .expect("handling event should succeed");
    assert!(exit_ctrl_q, "Ctrl+Q must signal event loop termination from Composer");

    let mut tree_app = grein_tui::state::AppState::new();
    tree_app.set_focus(FocusedPane::Tree);
    let quit_event =
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

    let exit_requested = grein_tui::runner::handle_event(&mut tree_app, quit_event)
        .expect("handling event should succeed");

    assert!(
        exit_requested,
        "'q' key outside Composer must signal event loop termination"
    );
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
fn test_pane_focus_round_trip_conversions_across_focus_modules() {
    use grein_tui::focus::PaneFocus as CorePaneFocus;
    use grein_tui::pane::focus::PaneFocus as PanePaneFocus;
    use grein_tui::ui::focus::PaneFocus as UiPaneFocus;

    // ui::focus::PaneFocus <-> app::FocusedPane
    let ui_pairs = [
        (UiPaneFocus::Tree, FocusedPane::Tree),
        (UiPaneFocus::Chat, FocusedPane::Transcript),
        (UiPaneFocus::Composer, FocusedPane::Composer),
    ];
    for (ui, app_pane) in ui_pairs {
        assert_eq!(FocusedPane::from(ui), app_pane);
        assert_eq!(UiPaneFocus::from(app_pane), ui);
        assert_eq!(UiPaneFocus::from(FocusedPane::from(ui)), ui);
    }

    // pane::focus::PaneFocus <-> app::FocusedPane
    let pane_pairs = [
        (PanePaneFocus::Workspace, FocusedPane::Tree),
        (PanePaneFocus::Logs, FocusedPane::Transcript),
        (PanePaneFocus::Composer, FocusedPane::Composer),
    ];
    for (pane, app_pane) in pane_pairs {
        assert_eq!(FocusedPane::from(pane), app_pane);
        assert_eq!(PanePaneFocus::from(app_pane), pane);
        assert_eq!(PanePaneFocus::from(FocusedPane::from(pane)), pane);
    }

    // app::FocusedPane -> focus::PaneFocus -> app::FocusedPane
    let core_pairs = [
        (FocusedPane::Tree, CorePaneFocus::Sidebar),
        (FocusedPane::Transcript, CorePaneFocus::Transcript),
        (FocusedPane::Composer, CorePaneFocus::Composer),
    ];
    for (app_pane, core) in core_pairs {
        assert_eq!(CorePaneFocus::from(app_pane), core);
        assert_eq!(FocusedPane::from(core), app_pane);
    }

    // Cross-module round trips through app::FocusedPane
    for ui in [UiPaneFocus::Tree, UiPaneFocus::Chat, UiPaneFocus::Composer] {
        let via_app: FocusedPane = ui.into();
        let pane: PanePaneFocus = via_app.into();
        let back: FocusedPane = pane.into();
        let core: CorePaneFocus = back.into();
        let final_app: FocusedPane = core.into();
        assert_eq!(final_app, via_app);
        assert_eq!(UiPaneFocus::from(final_app), ui);
    }
}

#[test]
fn test_golden_e2e_two_column_integration_and_multiline_sqlite() {
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let mut app = grein_tui::state::AppState::new();

    // 1. Focus Composer and type prompt including brackets '[' and ']', then submit with Enter
    app.set_focus(FocusedPane::Composer);
    for ch in "Verify [2-col] slice".chars() {
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        )
        .expect("char input should succeed");
    }
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
    app.apply_agent_response("- Rendered 2-column layout cleanly");
    app.persist_state_to_sqlite(&db_path);

    let mut reloaded = grein_tui::state::AppState::new();
    reloaded.load_state_from_sqlite(&db_path);
    assert!(
        reloaded
            .active_transcript_md
            .contains("Rendered 2-column layout cleanly"),
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

    // 4. Render 2-column panes via draw_frame and verify non-empty expected text
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
        screen.contains("ᛇ Grein") && screen.contains("Verify [2-col] slice"),
        "Tree pane must render thread context title"
    );
    // Transcript pane assertions
    assert!(
        screen.contains("Transcript") && screen.contains("Rendered 2-column layout cleanly"),
        "Transcript pane must render markdown turns"
    );
    assert!(
        !screen.contains("Blackboard"),
        "Blackboard pane must not render in 2-column layout"
    );
    // Composer pane & status bar assertions
    assert!(
        screen.contains("Composer")
            && screen.contains("Draft line 1")
            && screen.contains("Draft line 2")
            && (screen.contains("Ctrl+G tree · Alt+T zsh · ? help")
                || screen.contains("Esc cancel · Ctrl+G tree · Alt+T zsh")),
        "Composer pane must auto-grow for multi-line draft and render bottom status bar"
    );
}
