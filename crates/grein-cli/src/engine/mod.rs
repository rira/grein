mod client;
mod in_process;
mod rpc;
mod socket_client;
mod types;

#[cfg(test)]
mod tests;

pub use client::EngineClient;
pub use in_process::InProcessEngine;
pub use rpc::dispatch_json_rpc;
pub use socket_client::UnixSocketEngineClient;
pub use types::{
    slugify_anchor, truncate_chars_with_ellipsis, AcpBackendConfig, AgentMetaEntry,
    DaemonEngine, EngineError, EngineEvent, PermissionDecisionPayload, SessionSnapshot,
    ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};
