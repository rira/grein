use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

const JSONRPC_VERSION: &str = "2.0";
const CHANNEL_CAPACITY: usize = 100;
const NOTIFICATION_CAPACITY: usize = 256;
const STDERR_BUFFER_LINES: usize = 256;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioProcessConfig {
    pub command: String,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl StdioProcessConfig {
    pub fn new(
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            current_dir: None,
            env: Vec::new(),
        }
    }

    pub fn with_current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    pub fn with_env(mut self, env: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env.extend(env);
        self
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct StdioProcessRunner {
    child: Arc<Mutex<Option<tokio::process::Child>>>,
    pid: Option<u32>,
    stdin_tx: Arc<Mutex<Option<mpsc::Sender<JsonRpcMessage>>>>,
    raw_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<JsonRpcMessage>>>>,
    splitter_started: Arc<AtomicBool>,
    notification_tx: Option<mpsc::Sender<JsonRpcNotification>>,
    stdout_tx: Option<mpsc::Sender<JsonRpcMessage>>,
    stdout_rx: Arc<Mutex<Option<mpsc::Receiver<JsonRpcMessage>>>>,
    notification_rx: Option<mpsc::Receiver<JsonRpcNotification>>,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    is_running_flag: Arc<AtomicBool>,
}

impl StdioProcessRunner {
    pub async fn spawn(config: StdioProcessConfig) -> Result<Self, StdioRunnerError> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = &config.current_dir {
            cmd.current_dir(dir);
        }
        for (key, val) in &config.env {
            cmd.env(key, val);
        }

        let mut child = cmd.spawn().map_err(StdioRunnerError::SpawnFailed)?;

        let pid = child.id();
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| StdioRunnerError::CommunicationError("stdin not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| StdioRunnerError::CommunicationError("stdout not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| StdioRunnerError::CommunicationError("stderr not piped".into()))?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<JsonRpcMessage>(CHANNEL_CAPACITY);
        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<JsonRpcMessage>();
        let (stdout_tx, stdout_rx) = mpsc::channel::<JsonRpcMessage>(CHANNEL_CAPACITY);
        let (notification_tx, notification_rx) =
            mpsc::channel::<JsonRpcNotification>(NOTIFICATION_CAPACITY);
        let is_running_flag = Arc::new(AtomicBool::new(true));
        let stderr_lines = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_BUFFER_LINES)));

        // Writer: newline-delimited JSON-RPC 2.0 envelopes.
        tokio::spawn(async move {
            while let Some(msg) = stdin_rx.recv().await {
                if !msg.has_valid_version() {
                    tracing::warn!("dropping outgoing frame without jsonrpc 2.0 version");
                    continue;
                }
                let mut bytes = match serde_json::to_vec(&msg) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        tracing::warn!("failed to serialize outgoing frame: {err}");
                        continue;
                    }
                };
                bytes.push(b'\n');
                if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
            drop(stdin);
        });

        // Reader: decode and validate frames in strict arrival order.
        let flag_clone = Arc::clone(&is_running_flag);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let msg = match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                    Ok(msg) => msg,
                    Err(err) => {
                        tracing::debug!("rejecting undecodable frame: {err}");
                        continue;
                    }
                };
                if !msg.has_valid_version() {
                    tracing::debug!("rejecting frame with unsupported jsonrpc version");
                    continue;
                }
                if raw_tx.send(msg).is_err() {
                    tracing::debug!("raw message receiver closed");
                    break;
                }
            }
            flag_clone.store(false, Ordering::SeqCst);
        });

        // Stderr drain: never let the child block on a full OS pipe.
        let buffer = Arc::clone(&stderr_lines);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "grein_acp::stderr", "{line}");
                let mut guard = lock(&buffer);
                if guard.len() >= STDERR_BUFFER_LINES {
                    guard.pop_front();
                }
                guard.push_back(line);
            }
        });

        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            pid,
            stdin_tx: Arc::new(Mutex::new(Some(stdin_tx))),
            raw_rx: Arc::new(Mutex::new(Some(raw_rx))),
            splitter_started: Arc::new(AtomicBool::new(false)),
            notification_tx: Some(notification_tx),
            stdout_tx: Some(stdout_tx),
            stdout_rx: Arc::new(Mutex::new(Some(stdout_rx))),
            notification_rx: Some(notification_rx),
            stderr_lines,
            is_running_flag,
        })
    }

    fn ensure_splitter_started(&mut self) {
        if self.splitter_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(mut raw_rx) = lock(&self.raw_rx).take() else {
            return;
        };
        let Some(notification_tx) = self.notification_tx.take() else {
            return;
        };
        let Some(stdout_tx) = self.stdout_tx.take() else {
            return;
        };
        tokio::spawn(async move {
            while let Some(msg) = raw_rx.recv().await {
                match msg {
                    JsonRpcMessage::Notification(notification) => {
                        if let Err(err) = notification_tx.try_send(notification) {
                            tracing::debug!("notification not delivered: {err}");
                        }
                    }
                    other => {
                        if stdout_tx.send(other).await.is_err() {
                            tracing::debug!("message receiver closed");
                        }
                    }
                }
            }
        });
    }

    fn take_raw_rx(&mut self) -> Option<mpsc::UnboundedReceiver<JsonRpcMessage>> {
        self.splitter_started.store(true, Ordering::SeqCst);
        self.notification_tx.take();
        self.stdout_tx.take();
        lock(&self.raw_rx).take()
    }

    pub fn is_running(&self) -> bool {
        if !self.is_running_flag.load(Ordering::SeqCst) {
            return false;
        }
        let mut guard = lock(&self.child);
        match guard.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                Ok(Some(_)) => {
                    self.is_running_flag.store(false, Ordering::SeqCst);
                    false
                }
                Err(_) => false,
            },
            None => false,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Most recent stderr lines captured from the child (bounded).
    pub fn recent_stderr(&self) -> Vec<String> {
        lock(&self.stderr_lines).iter().cloned().collect()
    }

    /// Takes the notification stream; available once.
    pub fn take_notification_rx(&mut self) -> Option<mpsc::Receiver<JsonRpcNotification>> {
        self.ensure_splitter_started();
        self.notification_rx.take()
    }

    /// Takes the non-notification message stream (responses, errors, requests); available once.
    pub fn take_message_rx(&mut self) -> Option<mpsc::Receiver<JsonRpcMessage>> {
        self.ensure_splitter_started();
        lock(&self.stdout_rx).take()
    }

    /// Clones the outgoing sender while the process input is still open.
    pub fn sender(&self) -> Option<mpsc::Sender<JsonRpcMessage>> {
        lock(&self.stdin_tx).clone()
    }

    pub async fn terminate(
        &self,
        timeout_dur: Duration,
    ) -> Result<std::process::ExitStatus, StdioRunnerError> {
        lock(&self.stdin_tx).take();

        let maybe_child = lock(&self.child).take();

        if let Some(mut child) = maybe_child {
            let status = match tokio::time::timeout(timeout_dur, child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(err)) => return Err(StdioRunnerError::SpawnFailed(err)),
                Err(_) => {
                    let _ = child.start_kill();
                    child.wait().await.map_err(StdioRunnerError::SpawnFailed)?
                }
            };
            self.is_running_flag.store(false, Ordering::SeqCst);
            Ok(status)
        } else {
            Err(StdioRunnerError::ProcessTerminated(
                "Subprocess already terminated or claimed".into(),
            ))
        }
    }

    /// Consumes the runner returning the raw sender and message receiver.
    /// If either was already claimed, a closed channel half is returned instead.
    /// Notifications are not part of this stream; use `take_notification_rx` first.
    pub fn into_channels(mut self) -> (mpsc::Sender<JsonRpcMessage>, mpsc::Receiver<JsonRpcMessage>) {
        self.ensure_splitter_started();
        let stdin_tx = lock(&self.stdin_tx)
            .take()
            .unwrap_or_else(|| mpsc::channel(1).0);
        let stdout_rx = lock(&self.stdout_rx)
            .take()
            .unwrap_or_else(|| mpsc::channel(1).1);
        (stdin_tx, stdout_rx)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSessionInfo {
    pub session_id: String,
    pub current_model_id: Option<String>,
    pub current_mode_id: Option<String>,
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

    async fn shutdown(&self) -> Result<(), StdioRunnerError>;
}

type PendingRpcMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>>;
type SessionSinkMap = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<DriverEvent>>>>;

/// AgentDriver over an ACP v1 child process speaking NDJSON JSON-RPC 2.0 on stdio.
/// Must be constructed inside a tokio runtime.
pub struct AcpStdioDriver {
    runner: StdioProcessRunner,
    sender: Arc<Mutex<Option<mpsc::Sender<JsonRpcMessage>>>>,
    pending_rpc: PendingRpcMap,
    session_sinks: SessionSinkMap,
    next_id: AtomicU64,
}

impl AcpStdioDriver {
    pub fn new(mut runner: StdioProcessRunner) -> Self {
        let messages = runner
            .take_raw_rx()
            .unwrap_or_else(|| mpsc::unbounded_channel().1);
        let sender = Arc::new(Mutex::new(runner.sender()));
        let pending_rpc: PendingRpcMap = Arc::new(Mutex::new(HashMap::new()));
        let session_sinks: SessionSinkMap = Arc::new(Mutex::new(HashMap::new()));

        tokio::spawn(pump(
            messages,
            Arc::clone(&sender),
            Arc::clone(&pending_rpc),
            Arc::clone(&session_sinks),
        ));

        Self {
            runner,
            sender,
            pending_rpc,
            session_sinks,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn is_running(&self) -> bool {
        self.runner.is_running()
    }

    pub fn recent_stderr(&self) -> Vec<String> {
        self.runner.recent_stderr()
    }

    fn current_sender(&self) -> Result<mpsc::Sender<JsonRpcMessage>, StdioRunnerError> {
        lock(&self.sender)
            .clone()
            .ok_or_else(|| StdioRunnerError::ProcessTerminated("driver is shut down".into()))
    }

    async fn rpc_call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, StdioRunnerError> {
        let sender = self.current_sender()?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending_rpc).insert(id, tx);

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(id),
            method: method.to_string(),
            params,
        });

        if sender.send(request).await.is_err() {
            lock(&self.pending_rpc).remove(&id);
            return Err(StdioRunnerError::CommunicationError(
                "agent input channel closed".into(),
            ));
        }

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => {
                if !self.runner.is_running() {
                    Err(StdioRunnerError::ProcessTerminated(message))
                } else {
                    Err(StdioRunnerError::CommunicationError(message))
                }
            }
            Err(_) => Err(StdioRunnerError::ProcessTerminated(
                "agent process closed before replying".into(),
            )),
        }
    }

    pub async fn initialize(&self) -> Result<AcpInitializeResult, StdioRunnerError> {
        let params = serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "grein",
                "version": env!("CARGO_PKG_VERSION"),
            }
        });
        let result = self.rpc_call("initialize", Some(params)).await?;
        parse_initialize_result(&result)
    }

    pub async fn authenticate(&self, method_id: &str) -> Result<(), StdioRunnerError> {
        let params = serde_json::json!({ "methodId": method_id });
        self.rpc_call("authenticate", Some(params)).await?;
        Ok(())
    }

    pub async fn new_session(
        &self,
        cwd: impl AsRef<Path>,
        system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, StdioRunnerError> {
        let cwd_str = cwd.as_ref().to_string_lossy().into_owned();
        let mut params = serde_json::json!({
            "cwd": cwd_str,
            "mcpServers": [],
        });
        if let Some(append) = system_prompt_append.filter(|s| !s.is_empty()) {
            if let Some(obj) = params.as_object_mut() {
                obj.insert(
                    "_meta".to_string(),
                    serde_json::json!({
                        "antigravity": {
                            "systemPrompt": {
                                "append": append,
                            }
                        }
                    }),
                );
            }
        }
        let result = self.rpc_call("session/new", Some(params)).await?;
        parse_session_info(&result)
    }

    pub async fn send_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, StdioRunnerError> {
        let sender = self.current_sender()?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (rpc_tx, rpc_rx) = oneshot::channel();

        let session_id_owned = session_id.to_string();
        lock(&self.session_sinks).insert(session_id_owned.clone(), event_tx.clone());
        lock(&self.pending_rpc).insert(id, rpc_tx);

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(id),
            method: "session/prompt".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": prompt}],
            })),
        });

        if sender.send(request).await.is_err() {
            lock(&self.pending_rpc).remove(&id);
            remove_sink_if_same(&self.session_sinks, &session_id_owned, &event_tx);
            return Err(StdioRunnerError::CommunicationError(
                "agent input channel closed".into(),
            ));
        }

        let sinks_for_waiter = Arc::clone(&self.session_sinks);
        tokio::spawn(async move {
            let terminal_event = match rpc_rx.await {
                Ok(Ok(result)) => build_prompt_completed(&session_id_owned, &result),
                Ok(Err(message)) => DriverEvent::Error {
                    session_id: Some(session_id_owned.clone()),
                    message,
                },
                Err(_) => DriverEvent::Error {
                    session_id: Some(session_id_owned.clone()),
                    message: "agent process closed before completing prompt".to_string(),
                },
            };
            let _ = event_tx.send(terminal_event);
            remove_sink_if_same(&sinks_for_waiter, &session_id_owned, &event_tx);
        });

        Ok(event_rx)
    }

    pub async fn respond_permission(
        &self,
        request_id: serde_json::Value,
        option_id: Option<&str>,
    ) -> Result<(), StdioRunnerError> {
        let sender = self.current_sender()?;
        let result = match option_id {
            Some(opt) => serde_json::json!({
                "outcome": {
                    "outcome": "selected",
                    "optionId": opt,
                }
            }),
            None => serde_json::json!({
                "outcome": {
                    "outcome": "cancelled",
                }
            }),
        };
        let response = JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: request_id,
            result,
        });
        sender
            .send(response)
            .await
            .map_err(|_| StdioRunnerError::CommunicationError("agent input channel closed".into()))
    }

    pub async fn cancel_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        let sender = self.current_sender()?;
        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "session/cancel".to_string(),
            params: Some(serde_json::json!({ "sessionId": session_id })),
        });
        sender
            .send(notification)
            .await
            .map_err(|_| StdioRunnerError::CommunicationError("agent input channel closed".into()))
    }

    pub async fn close_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        let params = serde_json::json!({ "sessionId": session_id });
        self.rpc_call("session/close", Some(params)).await?;
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), StdioRunnerError> {
        lock(&self.sender).take();
        let res = match self.runner.terminate(SHUTDOWN_GRACE).await {
            Ok(_) => Ok(()),
            Err(StdioRunnerError::ProcessTerminated(_)) => Ok(()),
            Err(err) => Err(err),
        };
        fail_pending_rpcs(&self.pending_rpc, "driver is shut down");
        res
    }
}

#[async_trait]
impl AgentDriver for AcpStdioDriver {
    async fn initialize(&self) -> Result<AcpInitializeResult, StdioRunnerError> {
        AcpStdioDriver::initialize(self).await
    }

    async fn authenticate(&self, method_id: &str) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::authenticate(self, method_id).await
    }

    async fn new_session(
        &self,
        cwd: &Path,
        system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, StdioRunnerError> {
        AcpStdioDriver::new_session(self, cwd, system_prompt_append).await
    }

    async fn send_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, StdioRunnerError> {
        AcpStdioDriver::send_prompt(self, session_id, prompt).await
    }

    async fn respond_permission(
        &self,
        request_id: serde_json::Value,
        option_id: Option<&str>,
    ) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::respond_permission(self, request_id, option_id).await
    }

    async fn cancel_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::cancel_session(self, session_id).await
    }

    async fn close_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::close_session(self, session_id).await
    }

    fn recent_stderr(&self) -> Vec<String> {
        AcpStdioDriver::recent_stderr(self)
    }

    async fn shutdown(&self) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::shutdown(self).await
    }
}

fn remove_sink_if_same(
    sinks: &SessionSinkMap,
    session_id: &str,
    expected: &mpsc::UnboundedSender<DriverEvent>,
) {
    let mut guard = lock(sinks);
    if guard
        .get(session_id)
        .is_some_and(|existing| existing.same_channel(expected))
    {
        guard.remove(session_id);
    }
}

fn fail_pending_rpcs(pending_rpc: &PendingRpcMap, reason: &str) {
    let senders: Vec<_> = lock(pending_rpc).drain().map(|(_, tx)| tx).collect();
    for tx in senders {
        let _ = tx.send(Err(reason.to_string()));
    }
}

fn format_rpc_error(err: &serde_json::Value) -> String {
    if let Some(msg) = err.get("message").and_then(|v| v.as_str()) {
        if let Some(code) = err.get("code").and_then(|v| v.as_i64()) {
            return format!("JSON-RPC error {code}: {msg}");
        }
        return msg.to_string();
    }
    err.to_string()
}

fn parse_initialize_result(
    result: &serde_json::Value,
) -> Result<AcpInitializeResult, StdioRunnerError> {
    let protocol_version = result
        .get("protocolVersion")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            StdioRunnerError::CommunicationError(
                "initialize response missing protocolVersion".into(),
            )
        })?;
    let agent_name = result
        .pointer("/agentInfo/name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let agent_version = result
        .pointer("/agentInfo/version")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let supports_system_prompt_append = result
        .pointer("/agentCapabilities/_meta/antigravity/systemPrompt/append")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let auth_method_ids = result
        .get("authMethods")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Ok(AcpInitializeResult {
        protocol_version,
        agent_name,
        agent_version,
        supports_system_prompt_append,
        auth_method_ids,
    })
}

fn parse_session_info(result: &serde_json::Value) -> Result<AcpSessionInfo, StdioRunnerError> {
    let session_id = result
        .get("sessionId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            StdioRunnerError::CommunicationError("session/new response missing sessionId".into())
        })?
        .to_string();
    let current_model_id = result
        .pointer("/models/currentModelId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let current_mode_id = result
        .pointer("/modes/currentModeId")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(AcpSessionInfo {
        session_id,
        current_model_id,
        current_mode_id,
    })
}

fn build_prompt_completed(session_id: &str, result: &serde_json::Value) -> DriverEvent {
    let stop_reason = result
        .get("stopReason")
        .and_then(|v| v.as_str())
        .unwrap_or("end_turn")
        .to_string();
    let usage = result
        .get("usage")
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value::<AcpUsage>(v.clone()).ok());
    DriverEvent::PromptCompleted {
        session_id: session_id.to_string(),
        stop_reason,
        usage,
    }
}

fn str_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_default()
}

fn extract_tool_call_update_output(update: &serde_json::Value) -> String {
    if let Some(s) = update.pointer("/rawOutput/output").and_then(|v| v.as_str()) {
        if !s.is_empty() {
            return s.to_string();
        }
    }
    if let Some(s) = update
        .pointer("/content/0/content/text")
        .and_then(|v| v.as_str())
    {
        return s.to_string();
    }
    if let Some(s) = update.pointer("/content/0/text").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(v) = update.pointer("/rawOutput/output") {
        if let Some(s) = v.as_str() {
            return s.to_string();
        }
        if !v.is_null() {
            return v.to_string();
        }
    }
    String::new()
}

fn translate_session_update(n: &JsonRpcNotification) -> Option<(String, DriverEvent)> {
    if n.method != "session/update" {
        tracing::debug!("ignoring unmapped notification: {}", n.method);
        return None;
    }
    let params = n.params.as_ref()?;
    let session_id = params.get("sessionId").and_then(|v| v.as_str())?.to_string();
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate").and_then(|v| v.as_str())?;

    let event = match kind {
        "agent_message_chunk" => DriverEvent::StreamChunk {
            session_id: session_id.clone(),
            text: update
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "agent_thought_chunk" => DriverEvent::ThoughtChunk {
            session_id: session_id.clone(),
            text: update
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "tool_call" => DriverEvent::ToolCall {
            session_id: session_id.clone(),
            tool_call_id: str_field(update, "toolCallId"),
            name: str_field(update, "name"),
            title: str_field(update, "title"),
            args: update
                .get("args")
                .or_else(|| update.get("rawInput"))
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            status: str_field(update, "status"),
        },
        "tool_call_update" => DriverEvent::ToolCallUpdate {
            session_id: session_id.clone(),
            tool_call_id: str_field(update, "toolCallId"),
            name: str_field(update, "name"),
            status: str_field(update, "status"),
            output: extract_tool_call_update_output(update),
        },
        "session_info_update" => DriverEvent::SessionInfoUpdate {
            session_id: session_id.clone(),
            title: str_field(update, "title"),
        },
        "usage_update" => DriverEvent::UsageUpdate {
            session_id: session_id.clone(),
            used: update.get("used").and_then(|v| v.as_u64()).unwrap_or(0),
            size: update.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
        },
        other => {
            tracing::debug!("ignoring unknown session/update variant: {other}");
            return None;
        }
    };

    Some((session_id, event))
}

fn parse_permission_request(req: &JsonRpcRequest) -> Option<(String, DriverEvent)> {
    if req.method != "session/request_permission" {
        return None;
    }
    let params = req.params.as_ref()?;
    let session_id = params.get("sessionId").and_then(|v| v.as_str())?.to_string();
    let tool_call = params.get("toolCall");
    let tool_call_id = tool_call
        .map(|tc| str_field(tc, "toolCallId"))
        .unwrap_or_default();
    let tool_name = tool_call
        .map(|tc| str_field(tc, "name"))
        .unwrap_or_default();
    let title = tool_call
        .map(|tc| str_field(tc, "title"))
        .unwrap_or_default();
    let args = tool_call
        .and_then(|tc| tc.get("args").or_else(|| tc.get("rawInput")))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let options = params
        .get("options")
        .and_then(|v| serde_json::from_value::<Vec<AcpPermissionOption>>(v.clone()).ok())
        .unwrap_or_default();

    Some((
        session_id.clone(),
        DriverEvent::PermissionRequested {
            request_id: req.id.clone(),
            session_id,
            tool_call_id,
            tool_name,
            title,
            args,
            options,
        },
    ))
}

fn dispatch_to_session(sinks: &SessionSinkMap, session_id: &str, event: DriverEvent) {
    let tx = lock(sinks).get(session_id).cloned();
    if let Some(tx) = tx {
        if tx.send(event).is_err() {
            remove_sink_if_same(sinks, session_id, &tx);
        }
    }
}

async fn pump(
    mut messages: mpsc::UnboundedReceiver<JsonRpcMessage>,
    sender: Arc<Mutex<Option<mpsc::Sender<JsonRpcMessage>>>>,
    pending_rpc: PendingRpcMap,
    session_sinks: SessionSinkMap,
) {
    while let Some(msg) = messages.recv().await {
        match msg {
            JsonRpcMessage::Notification(n) => {
                if let Some((session_id, event)) = translate_session_update(&n) {
                    dispatch_to_session(&session_sinks, &session_id, event);
                }
            }
            JsonRpcMessage::Request(req) => {
                if let Some((session_id, event)) = parse_permission_request(&req) {
                    dispatch_to_session(&session_sinks, &session_id, event);
                } else {
                    tracing::debug!("ignoring server-initiated request: {}", req.method);
                }
            }
            JsonRpcMessage::Response(resp) => {
                if let Some(id) = resp.id.as_u64() {
                    if let Some(tx) = lock(&pending_rpc).remove(&id) {
                        let _ = tx.send(Ok(resp.result));
                    }
                }
            }
            JsonRpcMessage::Error(err) => {
                let message = format_rpc_error(&err.error);
                if let Some(id) = err.id.as_ref().and_then(|v| v.as_u64()) {
                    if let Some(tx) = lock(&pending_rpc).remove(&id) {
                        let _ = tx.send(Err(message));
                        continue;
                    }
                }
                let sinks: Vec<_> = lock(&session_sinks).values().cloned().collect();
                for tx in sinks {
                    let _ = tx.send(DriverEvent::Error {
                        session_id: None,
                        message: message.clone(),
                    });
                }
            }
        }
    }

    lock(&sender).take();
    fail_pending_rpcs(&pending_rpc, "agent process closed its output");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_initialize_result_with_capabilities_and_auth_methods() {
        let val = json!({
            "protocolVersion": 1,
            "agentInfo": { "name": "agy", "version": "1.5.14" },
            "authMethods": [
                { "id": "oauth-personal", "name": "OAuth", "description": "Google" },
                { "id": "vertex", "name": "Vertex", "description": "GCP" }
            ],
            "agentCapabilities": {
                "_meta": {
                    "antigravity": {
                        "systemPrompt": { "append": true }
                    }
                }
            }
        });
        let parsed = parse_initialize_result(&val).expect("should parse");
        assert_eq!(
            parsed,
            AcpInitializeResult {
                protocol_version: 1,
                agent_name: Some("agy".into()),
                agent_version: Some("1.5.14".into()),
                supports_system_prompt_append: true,
                auth_method_ids: vec!["oauth-personal".into(), "vertex".into()],
            }
        );

        let missing_ver = json!({ "agentInfo": {} });
        assert!(parse_initialize_result(&missing_ver).is_err());
    }

    #[test]
    fn parses_session_info_with_and_without_models_modes() {
        let full = json!({
            "sessionId": "sid-123",
            "models": { "currentModelId": "gemini-2.5-pro" },
            "modes": { "currentModeId": "architect" }
        });
        assert_eq!(
            parse_session_info(&full).expect("full session info"),
            AcpSessionInfo {
                session_id: "sid-123".into(),
                current_model_id: Some("gemini-2.5-pro".into()),
                current_mode_id: Some("architect".into()),
            }
        );

        let minimal = json!({ "sessionId": "sid-456" });
        assert_eq!(
            parse_session_info(&minimal).expect("minimal session info"),
            AcpSessionInfo {
                session_id: "sid-456".into(),
                current_model_id: None,
                current_mode_id: None,
            }
        );

        assert!(parse_session_info(&json!({ "sessionId": "" })).is_err());
    }

    #[test]
    fn translates_all_session_update_variants_and_ignores_unknown() {
        let make_notif = |update: serde_json::Value| JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "session/update".into(),
            params: Some(json!({ "sessionId": "s1", "update": update })),
        };

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "hi" }
            }))),
            Some((
                "s1".into(),
                DriverEvent::StreamChunk {
                    session_id: "s1".into(),
                    text: "hi".into(),
                }
            ))
        );

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "agent_thought_chunk",
                "content": { "type": "text", "text": "hmm" }
            }))),
            Some((
                "s1".into(),
                DriverEvent::ThoughtChunk {
                    session_id: "s1".into(),
                    text: "hmm".into(),
                }
            ))
        );

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "tc1",
                "name": "read_file",
                "title": "Reading",
                "args": { "Path": "src/lib.rs" },
                "status": "pending"
            }))),
            Some((
                "s1".into(),
                DriverEvent::ToolCall {
                    session_id: "s1".into(),
                    tool_call_id: "tc1".into(),
                    name: "read_file".into(),
                    title: "Reading".into(),
                    args: json!({ "Path": "src/lib.rs" }),
                    status: "pending".into(),
                }
            ))
        );

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tc1",
                "name": "read_file",
                "status": "completed",
                "rawOutput": { "output": "file body" }
            }))),
            Some((
                "s1".into(),
                DriverEvent::ToolCallUpdate {
                    session_id: "s1".into(),
                    tool_call_id: "tc1".into(),
                    name: "read_file".into(),
                    status: "completed".into(),
                    output: "file body".into(),
                }
            ))
        );

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tc2",
                "name": "run_command",
                "status": "completed",
                "content": [{ "type": "content", "content": { "type": "text", "text": "stdout text" } }]
            }))),
            Some((
                "s1".into(),
                DriverEvent::ToolCallUpdate {
                    session_id: "s1".into(),
                    tool_call_id: "tc2".into(),
                    name: "run_command".into(),
                    status: "completed".into(),
                    output: "stdout text".into(),
                }
            ))
        );

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "session_info_update",
                "title": "New Title"
            }))),
            Some((
                "s1".into(),
                DriverEvent::SessionInfoUpdate {
                    session_id: "s1".into(),
                    title: "New Title".into(),
                }
            ))
        );

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "usage_update",
                "used": 400,
                "size": 1000000
            }))),
            Some((
                "s1".into(),
                DriverEvent::UsageUpdate {
                    session_id: "s1".into(),
                    used: 400,
                    size: 1000000,
                }
            ))
        );

        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": []
            }))),
            None
        );
        assert_eq!(
            translate_session_update(&make_notif(json!({
                "sessionUpdate": "config_option_update",
                "options": []
            }))),
            None
        );
    }

    #[test]
    fn parses_permission_request_preserving_exact_id_value() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: json!("1"),
            method: "session/request_permission".into(),
            params: Some(json!({
                "sessionId": "sid-9",
                "toolCall": {
                    "toolCallId": "tc-9",
                    "name": "write_file",
                    "title": "Write",
                    "args": { "Path": "foo.rs" }
                },
                "options": [
                    { "optionId": "allow_once", "name": "Allow", "kind": "allow_once" }
                ]
            })),
        };
        let parsed = parse_permission_request(&req).expect("should parse permission");
        assert_eq!(
            parsed,
            (
                "sid-9".into(),
                DriverEvent::PermissionRequested {
                    request_id: json!("1"),
                    session_id: "sid-9".into(),
                    tool_call_id: "tc-9".into(),
                    tool_name: "write_file".into(),
                    title: "Write".into(),
                    args: json!({ "Path": "foo.rs" }),
                    options: vec![AcpPermissionOption {
                        option_id: "allow_once".into(),
                        name: "Allow".into(),
                        kind: "allow_once".into(),
                    }],
                }
            )
        );
    }

    #[test]
    fn decodes_null_result_and_string_id_request_messages() {
        let null_res = serde_json::from_str::<JsonRpcMessage>(
            r#"{"jsonrpc":"2.0","id":2,"result":null}"#,
        )
        .expect("null result response");
        assert!(matches!(
            null_res,
            JsonRpcMessage::Response(JsonRpcResponse {
                result: serde_json::Value::Null,
                ..
            })
        ));

        let str_req = serde_json::from_str::<JsonRpcMessage>(
            r#"{"jsonrpc":"2.0","id":"1","method":"session/request_permission","params":{}}"#,
        )
        .expect("string id request");
        assert!(matches!(
            str_req,
            JsonRpcMessage::Request(JsonRpcRequest {
                id: serde_json::Value::String(_),
                ..
            })
        ));
    }

    #[test]
    fn prompt_completed_extracts_camel_case_usage() {
        let ev = build_prompt_completed(
            "sid-1",
            &json!({
                "stopReason": "end_turn",
                "usage": {
                    "inputTokens": 11,
                    "outputTokens": 22,
                    "cachedReadTokens": 33,
                    "cachedWriteTokens": 44,
                    "thoughtTokens": 55,
                    "totalTokens": 165
                }
            }),
        );
        assert_eq!(
            ev,
            DriverEvent::PromptCompleted {
                session_id: "sid-1".into(),
                stop_reason: "end_turn".into(),
                usage: Some(AcpUsage {
                    input_tokens: 11,
                    output_tokens: 22,
                    cached_read_tokens: 33,
                    cached_write_tokens: 44,
                    thought_tokens: 55,
                    total_tokens: 165,
                }),
            }
        );
    }
}
