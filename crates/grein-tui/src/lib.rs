pub mod app;

pub use app::{App, FocusPane};

/// Launches the Grein TUI application.
///
/// # Errors
///
/// Returns an I/O error if terminal initialization or event loop execution fails.
pub async fn run() -> std::io::Result<()> {
    let _app = App::new();
    Ok(())
}
