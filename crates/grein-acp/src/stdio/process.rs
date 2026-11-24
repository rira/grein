use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Notify};

use super::lock;
use super::protocol::{JsonRpcMessage, JsonRpcNotification, StdioRunnerError};

const CHANNEL_CAPACITY: usize = 100;
const NOTIFICATION_CAPACITY: usize = 256;
const STDERR_BUFFER_LINES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioProcessConfig {
    pub command: String,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
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
            env_remove: Vec::new(),
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

    pub fn without_env(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }
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
    stderr_done: Arc<Notify>,
    stderr_finished: Arc<AtomicBool>,
    last_exit_status: Arc<Mutex<Option<std::process::ExitStatus>>>,
    is_running_flag: Arc<AtomicBool>,
}

async fn read_lossy_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Option<String> {
    buf.clear();
    match reader.read_until(b'\n', buf).await {
        Ok(0) | Err(_) => {
            buf.clear();
            None
        }
        Ok(_) => {
            let line = String::from_utf8_lossy(buf)
                .trim_end_matches(['\r', '\n'])
                .to_string();
            buf.clear();
            Some(line)
        }
    }
}

fn format_exit_status(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("signal {sig}");
        }
    }
    status.to_string()
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
        for key in &config.env_remove {
            cmd.env_remove(key);
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
        let stderr_done = Arc::new(Notify::new());
        let stderr_finished = Arc::new(AtomicBool::new(false));
        let last_exit_status = Arc::new(Mutex::new(None));

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
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            while let Some(line) = read_lossy_line(&mut reader, &mut buf).await {
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
        let stderr_done_clone = Arc::clone(&stderr_done);
        let stderr_finished_clone = Arc::clone(&stderr_finished);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            while let Some(line) = read_lossy_line(&mut reader, &mut buf).await {
                tracing::debug!(target: "grein_acp::stderr", "{line}");
                let mut guard = lock(&buffer);
                if guard.len() >= STDERR_BUFFER_LINES {
                    guard.pop_front();
                }
                guard.push_back(line);
            }
            stderr_finished_clone.store(true, Ordering::SeqCst);
            stderr_done_clone.notify_waiters();
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
            stderr_done,
            stderr_finished,
            last_exit_status,
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

    pub(super) fn take_raw_rx(&mut self) -> Option<mpsc::UnboundedReceiver<JsonRpcMessage>> {
        self.splitter_started.store(true, Ordering::SeqCst);
        self.notification_tx.take();
        self.stdout_tx.take();
        lock(&self.raw_rx).take()
    }

    fn harvest_exit_status(&self) -> Option<bool> {
        let mut guard = lock(&self.child);
        let child = guard.as_mut()?;
        match child.try_wait() {
            Ok(None) => Some(true),
            Ok(Some(status)) => {
                *lock(&self.last_exit_status) = Some(status);
                self.is_running_flag.store(false, Ordering::SeqCst);
                Some(false)
            }
            Err(_) => Some(false),
        }
    }

    pub fn is_running(&self) -> bool {
        let flag_running = self.is_running_flag.load(Ordering::SeqCst);
        if !flag_running && lock(&self.last_exit_status).is_some() {
            return false;
        }
        let child_running = self.harvest_exit_status().unwrap_or(false);
        flag_running && child_running
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Most recent stderr lines captured from the child (bounded).
    pub fn recent_stderr(&self) -> Vec<String> {
        lock(&self.stderr_lines).iter().cloned().collect()
    }

    pub async fn wait_for_stderr_briefly(&self, timeout: Duration) {
        if self.stderr_finished.load(Ordering::SeqCst) {
            return;
        }
        let notified = self.stderr_done.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.stderr_finished.load(Ordering::SeqCst) {
            return;
        }
        let _ = tokio::time::timeout(timeout, notified).await;
    }

    pub(super) async fn harvest_exit_status_briefly(&self, max_wait: Duration) {
        let deadline = tokio::time::Instant::now() + max_wait;
        while lock(&self.last_exit_status).is_none() && lock(&self.child).is_some() {
            let _ = self.harvest_exit_status();
            if lock(&self.last_exit_status).is_some() || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    pub fn exit_status_summary(&self) -> Option<String> {
        if lock(&self.last_exit_status).is_none() {
            let _ = self.harvest_exit_status();
        }
        lock(&self.last_exit_status).map(format_exit_status)
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
            *lock(&self.last_exit_status) = Some(status);
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
    pub fn into_channels(
        mut self,
    ) -> (mpsc::Sender<JsonRpcMessage>, mpsc::Receiver<JsonRpcMessage>) {
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
