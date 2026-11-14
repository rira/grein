pub mod app;
pub mod focus;

pub use app::App;
pub use focus::PaneFocus;

/// Entry point for running the TUI event loop asynchronously.
pub async fn run() -> std::io::Result<()> {
    let mut app = App::new();
    let _ = app.poll_events(std::time::Duration::from_millis(0));
    Ok(())
}
