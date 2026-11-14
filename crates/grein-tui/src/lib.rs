pub mod app;
pub mod composer;
pub mod diff_card;
pub mod editor;
pub mod events;
pub mod focus;
pub mod layout;
pub mod pager;
#[path = "panes/mod.rs"]
pub mod panes;
pub mod state;
pub mod status;

pub use app::{App, AppConfig, AppEvent, AppState};

pub async fn run() -> std::io::Result<()> {
    Ok(())
}
