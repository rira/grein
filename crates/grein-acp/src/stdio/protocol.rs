use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub(super) const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub error: serde_json::Value,
}

fn default_jsonrpc() -> String {
    JSONRPC_VERSION.to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
    Error(JsonRpcError),
}

impl JsonRpcMessage {
    /// Returns true when the envelope declares JSON-RPC version "2.0".
    pub fn has_valid_version(&self) -> bool {
        let version = match self {
            Self::Request(m) => &m.jsonrpc,
            Self::Notification(m) => &m.jsonrpc,
            Self::Response(m) => &m.jsonrpc,
            Self::Error(m) => &m.jsonrpc,
        };
        version == JSONRPC_VERSION
    }
}

#[derive(Debug)]
pub enum StdioRunnerError {
    SpawnFailed(std::io::Error),
    ProcessTerminated(String),
    Timeout,
    CommunicationError(String),
}

impl std::fmt::Display for StdioRunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailed(err) => write!(f, "Failed to spawn subprocess: {}", err),
            Self::ProcessTerminated(msg) => {
                write!(f, "Subprocess terminated unexpectedly: {}", msg)
            }
            Self::Timeout => write!(f, "Process termination timed out"),
            Self::CommunicationError(msg) => write!(f, "Communication error: {}", msg),
        }
    }
}

impl std::error::Error for StdioRunnerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SpawnFailed(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for StdioRunnerError {
    fn from(err: std::io::Error) -> Self {
        Self::SpawnFailed(err)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_read_tokens: u64,
    #[serde(default)]
    pub cached_write_tokens: u64,
    #[serde(default)]
    pub thought_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionOption {
    #[serde(default)]
    pub option_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpModelOption {
    pub model_id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpModeOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigChoice {
    pub value: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    pub current_value: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub options: Vec<AcpConfigChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSlashCommand {
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AcpSessionInfo {
    pub session_id: String,
    pub current_model_id: Option<String>,
    pub current_mode_id: Option<String>,
    pub available_models: Vec<AcpModelOption>,
    pub available_modes: Vec<AcpModeOption>,
    pub config_options: Vec<AcpConfigOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpInitializeResult {
    pub protocol_version: u64,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub supports_system_prompt_append: bool,
    pub auth_method_ids: Vec<String>,
}

/// Structured events emitted by an ACP v1 agent driver for a session turn.
#[derive(Debug, Clone, PartialEq)]
pub enum DriverEvent {
    StreamChunk {
        session_id: String,
        text: String,
    },
    ThoughtChunk {
        session_id: String,
        text: String,
    },
    ToolCall {
        session_id: String,
        tool_call_id: String,
        name: String,
        title: String,
        args: serde_json::Value,
        status: String,
    },
    ToolCallUpdate {
        session_id: String,
        tool_call_id: String,
        name: String,
        status: String,
        output: String,
    },
    SessionInfoUpdate {
        session_id: String,
        title: String,
    },
    CurrentModeUpdate {
        session_id: String,
        current_mode_id: String,
    },
    ConfigOptionUpdate {
        session_id: String,
        config_options: Vec<AcpConfigOption>,
    },
    AvailableCommandsUpdate {
        session_id: String,
        commands: Vec<AcpSlashCommand>,
    },
    UsageUpdate {
        session_id: String,
        used: u64,
        size: u64,
    },
    PermissionRequested {
        request_id: serde_json::Value,
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        title: String,
        args: serde_json::Value,
        options: Vec<AcpPermissionOption>,
    },
    PromptCompleted {
        session_id: String,
        stop_reason: String,
        usage: Option<AcpUsage>,
    },
    Error {
        session_id: Option<String>,
        message: String,
    },
}

/// Pluggable agent backend SPI speaking ACP v1.
#[async_trait]
pub trait AgentDriver: Send + Sync {
    async fn initialize(&self) -> Result<AcpInitializeResult, StdioRunnerError>;

    async fn authenticate(&self, method_id: &str) -> Result<(), StdioRunnerError>;

    async fn new_session(
        &self,
        cwd: &Path,
        system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, StdioRunnerError>;

    async fn send_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, StdioRunnerError>;

    async fn respond_permission(
        &self,
        request_id: serde_json::Value,
        option_id: Option<&str>,
    ) -> Result<(), StdioRunnerError>;

    async fn cancel_session(&self, session_id: &str) -> Result<(), StdioRunnerError>;

    async fn close_session(&self, session_id: &str) -> Result<(), StdioRunnerError>;

    fn recent_stderr(&self) -> Vec<String> {
        Vec::new()
    }

    fn subscribe_session(&self, _session_id: &str) -> mpsc::UnboundedReceiver<DriverEvent> {
        let (_tx, rx) = mpsc::unbounded_channel();
        rx
    }

    async fn submit_prompt(
        &self,
        _session_id: &str,
        _prompt: &str,
    ) -> Result<(), StdioRunnerError> {
        Err(StdioRunnerError::CommunicationError(
            "submit_prompt not supported by this driver".into(),
        ))
    }

    async fn set_session_model(
        &self,
        _session_id: &str,
        _model_id: &str,
    ) -> Result<(), StdioRunnerError> {
        Err(StdioRunnerError::CommunicationError("not supported".into()))
    }

    async fn set_session_mode(
        &self,
        _session_id: &str,
        _mode_id: &str,
    ) -> Result<(), StdioRunnerError> {
        Err(StdioRunnerError::CommunicationError("not supported".into()))
    }

    async fn set_config_option(
        &self,
        _session_id: &str,
        _config_id: &str,
        _value: &str,
    ) -> Result<Vec<AcpConfigOption>, StdioRunnerError> {
        Err(StdioRunnerError::CommunicationError("not supported".into()))
    }

    async fn shutdown(&self) -> Result<(), StdioRunnerError>;
}
