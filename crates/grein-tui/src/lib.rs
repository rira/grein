//! Grein TUI crate.

pub mod focus;

pub use focus::PaneFocus;

/// Runs the Grein TUI application.
pub async fn run() -> std::io::Result<()> {
    Ok(())
}
