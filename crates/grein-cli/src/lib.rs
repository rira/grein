pub mod args;
pub mod client;
pub mod daemon;
pub mod discovery;
pub mod standalone;

pub use args::{AttachArgs, Cli, Command, DaemonArgs};
pub use client::{DaemonClient, connect_to_daemon};
pub use daemon::{default_socket_path, run_daemon_with_socket};
pub use discovery::{
    AcpAgent, AcpAgentKind, DiscoveryError, discover_acp_agents, resolve_agents_configuration,
};
pub use standalone::{InProcessClient, StandaloneError, create_in_process_client, run_standalone};
