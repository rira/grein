mod client;
mod in_process;
mod ports;
mod rpc;
mod socket_client;
mod types;

#[cfg(test)]
mod tests;

pub use client::EngineClient;
pub use in_process::{EngineDependencies, InProcessEngine};
pub use ports::{
    AgentTurnPort, AutocompleteContextPort, DaemonEngine, EngineEventPort, GreinEngine,
    ShellUseCasePort, TerminalPtyUseCasePort, ThreadUseCasePort, WorktreeUseCasePort,
};
pub use rpc::dispatch_json_rpc;
pub use socket_client::UnixSocketEngineClient;
pub use types::{
    slugify_anchor, truncate_chars_with_ellipsis, AcpBackendConfig, AgentMetaEntry, EngineError,
    EngineEvent, PermissionDecisionPayload, SessionSnapshot, ShellCommandResult, ThreadSummary,
    TranscriptEntry, TurnDispatchResult,
};
