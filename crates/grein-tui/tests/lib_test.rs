use std::future::Future;
use std::io::Result as IoResult;

// Assert all required public modules and re-exports are publicly accessible
#[allow(unused_imports)]
use grein_tui::{
    app, composer, diff_card, editor, events, focus, layout, pager, panes, state, status,
};
use grein_tui::{App, AppConfig, AppEvent, AppState};

#[test]
fn test_public_modules_and_reexports_visibility() {
    // Verify re-exported types match app module types
    assert_eq!(
        std::any::type_name::<App>(),
        std::any::type_name::<app::App>()
    );
    assert_eq!(
        std::any::type_name::<AppConfig>(),
        std::any::type_name::<app::AppConfig>()
    );
    assert_eq!(
        std::any::type_name::<AppEvent>(),
        std::any::type_name::<app::AppEvent>()
    );
    assert_eq!(
        std::any::type_name::<AppState>(),
        std::any::type_name::<app::AppState>()
    );
}

#[test]
fn test_run_entrypoint_signature_and_future() {
    fn assert_async_run<F, Fut>(f: F)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = IoResult<()>>,
    {
        let _ = f;
    }

    assert_async_run(grein_tui::run);
}
