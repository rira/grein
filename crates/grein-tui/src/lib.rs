//! grein-tui: Ratatui interface, non-blocking input composer, and thread status indicators.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum TuiError {
    #[error("Terminal I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Ratatui render error: {0}")]
    Render(String),
}

pub struct TuiApp;

impl TuiApp {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TuiApp {
    fn default() -> Self {
        Self::new()
    }
}
