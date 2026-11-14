pub mod app;
pub mod runner;
pub mod state;

pub use app::{AnchorItem, App, AppState, FocusPane, FocusedPane, PaneFocus, ThreadItem, TuiError};
pub use runner::{draw_frame, handle_event, handle_key_event, TerminalGuard};

pub async fn run() -> std::io::Result<()> {
    Ok(())
}
