pub mod app;
pub mod event;

pub use app::{ActivePane, AppState};
pub use event::TuiEvent;

pub async fn run() -> std::io::Result<()> {
    let _state = AppState::default();
    Ok(())
}
