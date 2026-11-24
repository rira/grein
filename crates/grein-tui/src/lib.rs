#![allow(clippy::nonminimal_bool)]

pub mod app;
pub mod autocomplete;
pub mod clipboard;
pub mod composer;
pub mod diff_card;
pub mod engine_port;
pub mod focus;
pub mod keymap;
pub mod layout;
pub mod markdown;
pub mod pager;
pub mod pane;
pub mod render;
pub mod runner;
pub mod search;
pub mod state;
pub mod status;
pub mod syntax;
pub mod theme;
pub mod transcript;
pub mod ui;
pub mod vt_input;

use std::sync::Arc;

pub use app::{
    AnchorItem, App, AppAction, AppConfig, AppEvent, AppState, FocusDirection, FocusPane,
    FocusedPane, PaneFocus, PendingPermission, PendingQuitConfirm, PendingThreadDelete,
    RenamePromptState, SidebarLayoutConfig, SidebarMode, ThreadItem, TreeAction, TuiComponent,
    TuiError,
};
pub use engine_port::{TuiEnginePort, UserPreferencesSnapshot};
pub use keymap::{BindableAction, KeyChord, Keymap};
pub use runner::{draw_frame, draw_frame_mut, handle_event, handle_key_event, TerminalGuard};
pub use theme::{ThemePalette, ThemePickerModal, ThemePreset};
pub use transcript::{TranscriptMessage, TranscriptPane};

pub async fn run_with_engine(engine: Arc<dyn TuiEnginePort>) -> std::io::Result<()> {
    let _guard = runner::TerminalGuard::new()?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let app = app::App::new();
    runner::run_with_engine(&mut terminal, app, engine).await
}
