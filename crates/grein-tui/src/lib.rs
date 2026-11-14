//! Terminal User Interface for Grein.

pub mod composer;

pub use composer::Composer;

/// Error type for TUI runtime operations.
#[derive(Debug)]
pub struct TuiError(pub String);

impl std::fmt::Display for TuiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TuiError {}

/// Starts the terminal user interface event loop.
pub async fn run() -> Result<(), TuiError> {
    Ok(())
}
