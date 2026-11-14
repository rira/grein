pub mod app;

pub use app::{App, PaneFocus};

/// Entry point for running the Grein TUI interface.
pub async fn run() -> std::io::Result<()> {
    Ok(())
}
