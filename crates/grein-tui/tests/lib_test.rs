//! Black-box companion integration tests verifying module exports and public API boundaries of grein-tui.

#[test]
fn test_public_module_exports_exist() {
    // Verify modules are accessible and exported publicly
    let _ = std::any::type_name::<grein_tui::app::App>();
    let _ = std::any::type_name::<grein_tui::app::AppConfig>();
    let _ = std::any::type_name::<grein_tui::app::AppEvent>();
    let _ = std::any::type_name::<grein_tui::app::AppState>();

    // Verify top-level re-exports from app module
    let _ = std::any::type_name::<grein_tui::App>();
    let _ = std::any::type_name::<grein_tui::AppConfig>();
    let _ = std::any::type_name::<grein_tui::AppEvent>();
    let _ = std::any::type_name::<grein_tui::AppState>();
}

#[tokio::test]
async fn test_run_function_is_exported_and_callable() {
    // Verify run() is an exported async function returning io::Result<()>
    let result: std::io::Result<()> = grein_tui::run().await;
    assert!(result.is_ok(), "expected run() to return Ok(())");
}

#[test]
fn test_all_declared_modules_are_public() {
    // Type assertions across all requested public modules
    fn assert_module_exists<T>() {}

    // Ensure submodules are reachable via public paths
    assert_module_exists::<grein_tui::composer::ComposerPane>();
    assert_module_exists::<grein_tui::diff_card::DiffCard>();
    assert_module_exists::<grein_tui::editor::EditorPane>();
    assert_module_exists::<grein_tui::events::Event>();
    assert_module_exists::<grein_tui::focus::FocusManager>();
    assert_module_exists::<grein_tui::pager::Pager>();
    assert_module_exists::<grein_tui::status::StatusBar>();
    assert_module_exists::<grein_tui::panes::PaneId>();
    assert_module_exists::<grein_tui::layout::LayoutManager>();
    assert_module_exists::<grein_tui::state::TuiState>();
}
