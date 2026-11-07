//! grein-acp: Agent Client Protocol (ACP) stdio driver and session multiplexer.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AcpError {
    #[error("I/O error communicating with agent subprocess: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON-RPC protocol error: {0}")]
    Protocol(#[from] serde_json::Error),

    #[error("Agent exited unexpectedly with code: {0:?}")]
    AgentExit(Option<i32>),
}

pub struct AcpDriver;

impl AcpDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AcpDriver {
    fn default() -> Self {
        Self::new()
    }
}
