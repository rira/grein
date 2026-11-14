pub mod app;

pub use app::{App, FocusPane, FocusedPane, TuiError};

pub async fn run() -> std::io::Result<()> {
    Ok(())
}
