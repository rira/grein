use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::stdio::{
    AcpConfigOption, AcpInitializeResult, AcpSessionInfo, DriverEvent, StdioRunnerError,
};

/// Transport-agnostic error returned by [`AgentDriver`] and [`AgentDriverFactory`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentDriverError {
    #[error("failed to spawn agent driver: {0}")]
    SpawnFailed(String),
    #[error("agent driver terminated: {0}")]
    Terminated(String),
    #[error("agent driver operation timed out")]
    Timeout,
    #[error("agent driver transport error: {0}")]
    Transport(String),
    #[error("agent driver protocol error: {0}")]
    Protocol(String),
    #[error("unsupported agent driver operation: {0}")]
    Unsupported(String),
}

impl From<std::io::Error> for AgentDriverError {
    fn from(err: std::io::Error) -> Self {
        Self::SpawnFailed(err.to_string())
    }
}

impl From<StdioRunnerError> for AgentDriverError {
    fn from(err: StdioRunnerError) -> Self {
        match err {
            StdioRunnerError::SpawnFailed(io_err) => Self::SpawnFailed(io_err.to_string()),
            StdioRunnerError::ProcessTerminated(msg) => Self::Terminated(msg),
            StdioRunnerError::Timeout => Self::Timeout,
            StdioRunnerError::CommunicationError(msg) => Self::Transport(msg),
        }
    }
}

impl From<AgentDriverError> for StdioRunnerError {
    fn from(err: AgentDriverError) -> Self {
        match err {
            AgentDriverError::SpawnFailed(msg) => {
                Self::SpawnFailed(std::io::Error::other(msg))
            }
            AgentDriverError::Terminated(msg) => Self::ProcessTerminated(msg),
            AgentDriverError::Timeout => Self::Timeout,
            AgentDriverError::Transport(msg)
            | AgentDriverError::Protocol(msg)
            | AgentDriverError::Unsupported(msg) => Self::CommunicationError(msg),
        }
    }
}

/// Transport-agnostic launch configuration for spawning an ACP [`AgentDriver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSpawnConfig {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub auth_method: Option<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
}

impl AcpSpawnConfig {
    pub fn new(
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            auth_method: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        }
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_auth_method(mut self, auth_method: impl Into<String>) -> Self {
        self.auth_method = Some(auth_method.into());
        self
    }

    pub fn with_env(mut self, env: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env.extend(env);
        self
    }

    pub fn without_env(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }
}

/// Pluggable agent backend SPI speaking ACP v1.
#[async_trait]
pub trait AgentDriver: Send + Sync {
    fn is_running(&self) -> bool;

    async fn initialize(&self) -> Result<AcpInitializeResult, AgentDriverError>;

    async fn authenticate(&self, method_id: &str) -> Result<(), AgentDriverError>;

    async fn new_session(
        &self,
        cwd: &Path,
        system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, AgentDriverError>;

    async fn send_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, AgentDriverError>;

    async fn respond_permission(
        &self,
        request_id: serde_json::Value,
        option_id: Option<&str>,
    ) -> Result<(), AgentDriverError>;

    async fn cancel_session(&self, session_id: &str) -> Result<(), AgentDriverError>;

    async fn close_session(&self, session_id: &str) -> Result<(), AgentDriverError>;

    fn recent_stderr(&self) -> Vec<String> {
        Vec::new()
    }

    fn exit_status(&self) -> Option<String> {
        None
    }

    fn subscribe_session(&self, _session_id: &str) -> mpsc::UnboundedReceiver<DriverEvent> {
        let (_tx, rx) = mpsc::unbounded_channel();
        rx
    }

    async fn submit_prompt(
        &self,
        _session_id: &str,
        _prompt: &str,
    ) -> Result<(), AgentDriverError> {
        Err(AgentDriverError::Unsupported(
            "submit_prompt not supported by this driver".into(),
        ))
    }

    async fn set_session_model(
        &self,
        _session_id: &str,
        _model_id: &str,
    ) -> Result<(), AgentDriverError> {
        Err(AgentDriverError::Unsupported("not supported".into()))
    }

    async fn set_session_mode(
        &self,
        _session_id: &str,
        _mode_id: &str,
    ) -> Result<(), AgentDriverError> {
        Err(AgentDriverError::Unsupported("not supported".into()))
    }

    async fn set_config_option(
        &self,
        _session_id: &str,
        _config_id: &str,
        _value: &str,
    ) -> Result<Vec<AcpConfigOption>, AgentDriverError> {
        Err(AgentDriverError::Unsupported("not supported".into()))
    }

    async fn shutdown(&self) -> Result<(), AgentDriverError>;
}

/// Outbound factory port for spawning and handshaking live [`AgentDriver`] instances.
#[async_trait]
pub trait AgentDriverFactory: Send + Sync {
    async fn spawn(&self, config: &AcpSpawnConfig) -> Result<Arc<dyn AgentDriver>, AgentDriverError>;

    /// Spawns a driver, runs ACP `initialize`, and optionally runs `authenticate` when configured.
    /// Shuts down the driver before returning on handshake/auth failure.
    async fn spawn_initialized(
        &self,
        config: &AcpSpawnConfig,
    ) -> Result<Arc<dyn AgentDriver>, AgentDriverError> {
        let driver = self.spawn(config).await?;
        if let Err(err) = driver.initialize().await {
            let _ = driver.shutdown().await;
            return Err(err);
        }
        if let Some(method) = config.auth_method.as_deref().filter(|m| !m.is_empty()) {
            if let Err(err) = driver.authenticate(method).await {
                let _ = driver.shutdown().await;
                return Err(err);
            }
        }
        Ok(driver)
    }
}
