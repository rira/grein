pub mod args;
pub mod client;
pub mod daemon;
pub mod discovery;
pub mod standalone;

pub use args::{AttachArgs, Cli, Command, DaemonArgs};
pub use client::{connect_to_daemon, DaemonClient};
pub use daemon::{default_socket_path, run_daemon_with_socket};
pub use discovery::{
    discover_acp_agents, resolve_agents_configuration, AcpAgent, AcpAgentKind, DiscoveryError,
};
pub use standalone::{create_in_process_client, run_standalone, InProcessClient, StandaloneError};
