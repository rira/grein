pub mod app;
pub mod composer;
pub mod diff_card;
pub mod editor;
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

pub use app::{
    AnchorItem, App, AppAction, AppConfig, AppEvent, AppState, FocusDirection, FocusPane,
    FocusedPane, PaneFocus, ThreadItem, TreeAction, TuiComponent, TuiError,
};
pub use runner::{draw_frame, handle_event, handle_key_event, TerminalGuard};
pub use transcript::{TranscriptMessage, TranscriptPane};

pub async fn run() -> std::io::Result<()> {
    Ok(())
}
