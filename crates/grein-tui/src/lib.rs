pub mod app;

pub use app::{App, FocusPane, TuiError};

/// Run the interactive terminal user interface.
///
/// # Errors
///
/// Returns `TuiError` if terminal initialization or event loop execution fails.
pub async fn run() -> Result<(), TuiError> {
    let _app = App::new();
    Ok(())
}
