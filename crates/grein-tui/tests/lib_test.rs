use grein_tui::{run, App, AppConfig, AppEvent, AppState};

#[tokio::test]
async fn test_run_entrypoint_returns_ok() {
    let result: std::io::Result<()> = run().await;
    assert!(result.is_ok());
}

#[test]
fn test_public_app_types_are_reexported() {
    fn assert_type_exists<T>() {}

    assert_type_exists::<App>();
    assert_type_exists::<AppConfig>();
    assert_type_exists::<AppEvent>();
    assert_type_exists::<AppState>();
}
