use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

const JSONRPC_VERSION: &str = "2.0";
const CHANNEL_CAPACITY: usize = 100;
const NOTIFICATION_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 256;
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
}

impl StdioProcessConfig {
    pub fn new(
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
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
    stdout_rx: Arc<Mutex<Option<mpsc::Receiver<JsonRpcMessage>>>>,
    notification_rx: Option<mpsc::Receiver<JsonRpcNotification>>,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    is_running_flag: Arc<AtomicBool>,
}

impl StdioProcessRunner {
    pub async fn spawn(config: StdioProcessConfig) -> Result<Self, StdioRunnerError> {
        let mut child = Command::new(&config.command)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(StdioRunnerError::SpawnFailed)?;

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

        // Reader: decode and validate frames, split notifications from other messages.
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
            stdout_rx: Arc::new(Mutex::new(Some(stdout_rx))),
            notification_rx: Some(notification_rx),
            stderr_lines,
            is_running_flag,
        })
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
        self.notification_rx.take()
    }

    /// Takes the non-notification message stream (responses, errors, requests); available once.
    pub fn take_message_rx(&mut self) -> Option<mpsc::Receiver<JsonRpcMessage>> {
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
    pub fn into_channels(self) -> (mpsc::Sender<JsonRpcMessage>, mpsc::Receiver<JsonRpcMessage>) {
        let stdin_tx = lock(&self.stdin_tx)
            .take()
            .unwrap_or_else(|| mpsc::channel(1).0);
        let stdout_rx = lock(&self.stdout_rx)
            .take()
            .unwrap_or_else(|| mpsc::channel(1).1);
        (stdin_tx, stdout_rx)
    }
}

/// Structured events emitted by an agent driver for a single turn.
#[derive(Debug, Clone, PartialEq)]
pub enum DriverEvent {
    StreamChunk(String),
    ToolInvoked {
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        name: String,
        output: serde_json::Value,
    },
    AnchorDiscovered(String),
    Completed,
    Error(String),
}

/// Pluggable agent backend SPI.
#[async_trait]
pub trait AgentDriver: Send + Sync {
    async fn send_turn(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<mpsc::Receiver<DriverEvent>, StdioRunnerError>;

    async fn cancel(&self, thread_id: &str) -> Result<(), StdioRunnerError>;

    async fn shutdown(&self) -> Result<(), StdioRunnerError>;
}

struct TurnSink {
    id: serde_json::Value,
    tx: mpsc::Sender<DriverEvent>,
}

type Sink = Arc<Mutex<Option<TurnSink>>>;

/// AgentDriver over an ACP child process speaking JSON-RPC 2.0 on stdio.
/// Must be constructed inside a tokio runtime.
pub struct AcpStdioDriver {
    runner: StdioProcessRunner,
    sender: Mutex<Option<mpsc::Sender<JsonRpcMessage>>>,
    sink: Sink,
    next_id: AtomicU64,
}

impl AcpStdioDriver {
    pub fn new(mut runner: StdioProcessRunner) -> Self {
        let notifications = runner
            .take_notification_rx()
            .unwrap_or_else(|| mpsc::channel(1).1);
        let messages = runner
            .take_message_rx()
            .unwrap_or_else(|| mpsc::channel(1).1);
        let sender = runner.sender();
        let sink: Sink = Arc::new(Mutex::new(None));
        tokio::spawn(pump(notifications, messages, Arc::clone(&sink)));
        Self {
            runner,
            sender: Mutex::new(sender),
            sink,
            next_id: AtomicU64::new(1),
        }
    }

    fn current_sender(&self) -> Result<mpsc::Sender<JsonRpcMessage>, StdioRunnerError> {
        lock(&self.sender)
            .clone()
            .ok_or_else(|| StdioRunnerError::ProcessTerminated("driver is shut down".into()))
    }
}

fn translate_notification(n: &JsonRpcNotification) -> Option<DriverEvent> {
    let params = n.params.as_ref();
    let text = |key: &str| {
        params
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let value = |key: &str| {
        params
            .and_then(|p| p.get(key))
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    match n.method.as_str() {
        "stream/chunk" => Some(DriverEvent::StreamChunk(text("text").unwrap_or_default())),
        "tool/invoked" => Some(DriverEvent::ToolInvoked {
            name: text("name").unwrap_or_default(),
            arguments: value("arguments"),
        }),
        "tool/result" => Some(DriverEvent::ToolResult {
            name: text("name").unwrap_or_default(),
            output: value("output"),
        }),
        "anchor/discovered" => Some(DriverEvent::AnchorDiscovered(
            text("anchor").unwrap_or_default(),
        )),
        "turn/completed" => Some(DriverEvent::Completed),
        "error" => Some(DriverEvent::Error(
            text("message").unwrap_or_else(|| "agent reported an error".into()),
        )),
        other => {
            tracing::debug!("ignoring unmapped notification: {other}");
            None
        }
    }
}

async fn dispatch(sink: &Sink, event: DriverEvent) {
    let terminal = matches!(event, DriverEvent::Completed | DriverEvent::Error(_));
    let tx = {
        let mut guard = lock(sink);
        if terminal {
            guard.take().map(|t| t.tx)
        } else {
            guard.as_ref().map(|t| t.tx.clone())
        }
    };
    if let Some(tx) = tx {
        if tx.send(event).await.is_err() {
            lock(sink).take();
        }
    }
}

fn matches_turn(sink: &Sink, id: &serde_json::Value) -> bool {
    lock(sink).as_ref().is_some_and(|t| &t.id == id)
}

async fn pump(
    mut notifications: mpsc::Receiver<JsonRpcNotification>,
    mut messages: mpsc::Receiver<JsonRpcMessage>,
    sink: Sink,
) {
    let mut notifications_open = true;
    let mut messages_open = true;
    loop {
        tokio::select! {
            n = notifications.recv(), if notifications_open => match n {
                Some(n) => {
                    if let Some(event) = translate_notification(&n) {
                        dispatch(&sink, event).await;
                    }
                }
                None => notifications_open = false,
            },
            m = messages.recv(), if messages_open => match m {
                Some(JsonRpcMessage::Response(r)) => {
                    if matches_turn(&sink, &r.id) {
                        dispatch(&sink, DriverEvent::Completed).await;
                    }
                }
                Some(JsonRpcMessage::Error(e)) => {
                    let relevant = match &e.id {
                        Some(id) => matches_turn(&sink, id),
                        None => true,
                    };
                    if relevant {
                        dispatch(&sink, DriverEvent::Error(e.error.to_string())).await;
                    }
                }
                Some(other) => tracing::debug!("ignoring agent-initiated message: {other:?}"),
                None => messages_open = false,
            },
            else => break,
        }
    }
    dispatch(
        &sink,
        DriverEvent::Error("agent process closed its output".into()),
    )
    .await;
}

#[async_trait]
impl AgentDriver for AcpStdioDriver {
    async fn send_turn(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<mpsc::Receiver<DriverEvent>, StdioRunnerError> {
        let sender = self.current_sender()?;
        let id = serde_json::json!(self.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::channel(EVENT_CAPACITY);
        *lock(&self.sink) = Some(TurnSink { id: id.clone(), tx });

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: "session/prompt".to_string(),
            params: Some(serde_json::json!({"threadId": thread_id, "prompt": prompt})),
        });
        if sender.send(request).await.is_err() {
            lock(&self.sink).take();
            return Err(StdioRunnerError::CommunicationError(
                "agent input channel closed".into(),
            ));
        }
        Ok(rx)
    }

    async fn cancel(&self, thread_id: &str) -> Result<(), StdioRunnerError> {
        let sender = self.current_sender()?;
        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "session/cancel".to_string(),
            params: Some(serde_json::json!({"threadId": thread_id})),
        });
        sender
            .send(notification)
            .await
            .map_err(|_| StdioRunnerError::CommunicationError("agent input channel closed".into()))
    }

    async fn shutdown(&self) -> Result<(), StdioRunnerError> {
        lock(&self.sender).take();
        match self.runner.terminate(SHUTDOWN_GRACE).await {
            Ok(_) => Ok(()),
            Err(StdioRunnerError::ProcessTerminated(_)) => Ok(()),
            Err(err) => Err(err),
        }
    }
}
