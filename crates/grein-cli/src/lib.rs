pub mod args;
pub mod client;
pub mod daemon;
pub mod discovery;
pub mod engine;
pub mod headless;
pub mod standalone;
pub mod subagent_watch;

pub use args::{AgentsArgs, AttachArgs, Cli, Command, DaemonArgs, StatusArgs, ThreadsArgs};
pub use client::{connect_to_daemon, DaemonClient};
pub use daemon::{
    default_db_path, default_socket_path, resolve_state_dir, run_daemon_with_socket,
    run_daemon_with_socket_and_db,
};
pub use discovery::{
    discover_acp_agents, resolve_agents_configuration, AcpAgent, AcpAgentKind, DiscoveryError,
};
pub use engine::{
    DaemonEngine, EngineClient, EngineError, EngineEvent, InProcessEngine,
    PermissionDecisionPayload, SessionSnapshot, ThreadSummary, TurnDispatchResult,
    UnixSocketEngineClient,
};
pub use headless::{handle_agents, handle_status, handle_threads, AgentOutput, StatusOutput};
pub use standalone::{create_in_process_client, run_standalone, InProcessClient, StandaloneError};
