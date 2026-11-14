//! Terminal User Interface (TUI) for Grein.

pub mod focus;

/// Runs the terminal user interface application.
///
/// # Errors
///
/// Returns an error if terminal initialization or runtime loop fails.
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}
