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
pub mod selection;
pub mod state;
pub mod status;
pub mod syntax;
pub mod terminal;
pub mod terminal_decoder;
pub mod theme;
pub mod transcript;
pub mod ui;
pub mod vt_input;

use std::sync::Arc;

pub use app::{
    AgentPickerModal, AnchorItem, App, AppAction, AppConfig, AppEvent, AppState, FocusDirection,
    FocusPane, FocusedPane, PaneFocus, PendingPermission, PendingQuitConfirm, PendingThreadDelete,
    RenamePromptState, SidebarLayoutConfig, SidebarMode, ThreadItem, TreeAction, TuiError,
    TuiServices,
};
pub use clipboard::ClipboardPort;
pub use engine_port::{
    AgentProviderInfo, PreferencesPort, TuiEnginePort, UserPreferencesSnapshot,
};
pub use keymap::{BindableAction, KeyChord, Keymap};
pub use runner::{
    draw_frame, draw_frame_mut, handle_event, handle_key_event, run_key_inspector,
    KeyInspectorOptions, TerminalGuard,
};
pub use theme::{ThemePalette, ThemePickerModal, ThemePreset};
pub use transcript::{TranscriptMessage, TranscriptPane};

pub async fn run_with_engine(engine: Arc<dyn TuiEnginePort>) -> std::io::Result<()> {
    run_with_engine_and_services(engine, TuiServices::default()).await
}

pub async fn run_with_engine_and_services(
    engine: Arc<dyn TuiEnginePort>,
    services: TuiServices,
) -> std::io::Result<()> {
    let _host = terminal::TerminalSessionHost::new(
        Arc::clone(&services.terminal),
        terminal::TerminalEnv::from_env(),
        terminal::TerminalSessionOptions::default(),
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let app = app::App::with_services(services);
    runner::run_with_engine(&mut terminal, app, engine).await
}
