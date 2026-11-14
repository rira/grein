use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_tui::layout::{compute_layout, AppLayout, LayoutManager, PaneFocus, WorkspaceLayout};
use grein_tui::{
    draw_frame, handle_event, handle_key_event, AnchorItem, App, AppAction, AppConfig, AppEvent,
    AppState, FocusDirection, FocusPane, FocusedPane, TerminalGuard, ThreadItem, TranscriptMessage,
    TranscriptPane, TreeAction, TuiComponent, TuiError,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

#[test]
fn test_public_module_exports_and_reexports() {
    let _ = LayoutManager;
    let mut app = App::new();
    let focus: PaneFocus = FocusedPane::Tree;
    assert_eq!(focus, app.focused_pane);

    let area = Rect::new(0, 0, 100, 40);
    let layout_expanded: AppLayout = compute_layout(area, false);
    assert!(layout_expanded.blackboard_area.is_some());
    assert_eq!(layout_expanded.composer_area.height, 3);

    let layout_collapsed: WorkspaceLayout = compute_layout(area, true);
    assert!(layout_collapsed.blackboard_area.is_none());

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal should initialize");
    draw_frame(&mut terminal, &app).expect("draw_frame should succeed");

    let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let tree_res = handle_key_event(&mut app, key);
    assert!(tree_res.is_ok());

    let quit_event = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    let should_quit = handle_event(&mut app, quit_event).expect("handle_event should succeed");
    assert!(should_quit);

    let _guard = TerminalGuard::new().expect("TerminalGuard should initialize in headless mode");

    let _ = std::mem::size_of::<(
        AnchorItem,
        AppAction,
        AppConfig,
        AppEvent,
        AppState,
        FocusDirection,
        FocusPane,
        ThreadItem,
        TranscriptMessage,
        TranscriptPane,
        TreeAction,
        TuiError,
    )>();

    fn _assert_component<T: TuiComponent>() {}
}
