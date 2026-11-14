use grein_tui::app::{ActivePane, AppState};
use grein_tui::event::TuiEvent;

#[test]
fn test_public_module_exports_and_4_pane_focus_navigation() {
    let mut state = AppState::default();
    assert_eq!(state.active_pane(), ActivePane::Composer);

    state.cycle_focus_next();
    assert_eq!(state.active_pane(), ActivePane::ThreadStream);

    state.cycle_focus_next();
    assert_eq!(state.active_pane(), ActivePane::SessionSidebar);

    state.cycle_focus_next();
    assert_eq!(state.active_pane(), ActivePane::WorkspaceInspector);

    state.cycle_focus_next();
    assert_eq!(state.active_pane(), ActivePane::Composer);

    state.cycle_focus_prev();
    assert_eq!(state.active_pane(), ActivePane::WorkspaceInspector);

    let _ = TuiEvent::Tick;
}
