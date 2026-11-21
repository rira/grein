#![allow(clippy::nonminimal_bool)]

pub mod app;
pub mod composer;
pub mod diff_card;
pub mod editor;
pub mod engine_port;
pub mod events;
pub mod focus;
pub mod layout;
pub mod pager;
pub mod pane;
pub mod panes;
pub mod render;
pub mod runner;
pub mod state;
pub mod status;
pub mod transcript;
pub mod ui;
pub mod views;

use std::sync::Arc;

pub use app::{
    AnchorItem, App, AppAction, AppConfig, AppEvent, AppState, FocusDirection, FocusPane,
    FocusedPane, PaneFocus, PendingPermission, ThreadItem, TreeAction, TuiComponent, TuiError,
};
pub use engine_port::TuiEnginePort;
pub use runner::{draw_frame, draw_frame_mut, handle_event, handle_key_event, TerminalGuard};
pub use transcript::{TranscriptMessage, TranscriptPane};

pub async fn run_with_engine(engine: Arc<dyn TuiEnginePort>) -> std::io::Result<()> {
    let _guard = runner::TerminalGuard::new()?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let app = app::App::new();
    runner::run_with_engine(&mut terminal, app, engine).await
}
