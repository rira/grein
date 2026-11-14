pub mod app;
pub mod layout;
pub mod panes;

pub use app::{App, AppConfig, AppEvent, AppState};

pub async fn run() -> std::io::Result<()> {
    Ok(())
}
