pub mod args;
pub mod client;
pub mod daemon;
pub mod discovery;
pub mod doctor;
pub mod engine;
pub mod headless;
pub mod settings_cmd;
pub mod standalone;
pub mod subagent_watch;

pub use args::{
    AgentsArgs, AttachArgs, Cli, Command, DaemonArgs, DoctorArgs, InputFormatArg, OutputFormatArg,
    SettingsArgs, SettingsCommand, SettingsExportArgs, SettingsImportArgs, StatusArgs, ThemeArgs,
    ThemeCommand, ThemeExportArgs, ThemeImportArgs, ThreadsArgs,
};
pub use client::{connect_to_daemon, DaemonClient};
pub use daemon::{
    default_db_path, default_socket_path, resolve_state_dir, run_daemon_with_socket,
    run_daemon_with_socket_and_db,
};
pub use discovery::{
    discover_acp_agents, resolve_agents_configuration, AcpAgent, AcpAgentKind, DiscoveryError,
};
pub use doctor::{collect_doctor_report, handle_doctor, CheckStatus, DoctorCheck, DoctorReport};
pub use engine::{
    DaemonEngine, EngineClient, EngineError, EngineEvent, InProcessEngine,
    PermissionDecisionPayload, SessionSnapshot, ThreadSummary, TurnDispatchResult,
    UnixSocketEngineClient,
};
pub use headless::{handle_agents, handle_status, handle_threads, AgentOutput, StatusOutput};
pub use settings_cmd::{export_theme_string, handle_settings, handle_theme};
pub use standalone::{create_in_process_client, run_standalone, InProcessClient, StandaloneError};
