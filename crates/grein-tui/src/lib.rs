pub mod app;
pub mod focus;
pub mod layout;
pub mod panes;
pub mod render;
pub mod runner;
pub mod state;
pub mod ui;

pub use app::{AnchorItem, App, AppState, FocusPane, FocusedPane, PaneFocus, ThreadItem, TuiError};
pub use runner::{draw_frame, handle_event, handle_key_event, TerminalGuard};

pub async fn run() -> std::io::Result<()> {
    let _guard = runner::TerminalGuard::new()?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let app = app::App::new();
    runner::run(&mut terminal, app)
}
