use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
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
    "2.0".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Error(JsonRpcError),
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

pub struct StdioProcessRunner {
    child: Arc<Mutex<Option<tokio::process::Child>>>,
    pid: Option<u32>,
    stdin_tx: Arc<Mutex<Option<mpsc::Sender<JsonRpcMessage>>>>,
    stdout_rx: Arc<Mutex<Option<mpsc::Receiver<JsonRpcMessage>>>>,
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
        let mut stdin = child.stdin.take().expect("stdin must be piped");
        let stdout = child.stdout.take().expect("stdout must be piped");

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<JsonRpcMessage>(100);
        let (stdout_tx, stdout_rx) = mpsc::channel::<JsonRpcMessage>(100);
        let is_running_flag = Arc::new(AtomicBool::new(true));

        tokio::spawn(async move {
            while let Some(msg) = stdin_rx.recv().await {
                if let Ok(json_str) = serde_json::to_string(&msg) {
                    if stdin.write_all(json_str.as_bytes()).await.is_err() {
                        break;
                    }
                    if stdin.write_all(b"\n").await.is_err() {
                        break;
                    }
                    if stdin.flush().await.is_err() {
                        break;
                    }
                }
            }
            drop(stdin);
        });

        let flag_clone = Arc::clone(&is_running_flag);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(trimmed) {
                    if stdout_tx.send(msg).await.is_err() {
                        break;
                    }
                }
            }
            flag_clone.store(false, Ordering::SeqCst);
        });

        let child = Arc::new(Mutex::new(Some(child)));

        Ok(Self {
            child,
            pid,
            stdin_tx: Arc::new(Mutex::new(Some(stdin_tx))),
            stdout_rx: Arc::new(Mutex::new(Some(stdout_rx))),
            is_running_flag,
        })
    }

    pub fn is_running(&self) -> bool {
        if !self.is_running_flag.load(Ordering::SeqCst) {
            return false;
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(ref mut child) = *guard {
                match child.try_wait() {
                    Ok(None) => true,
                    Ok(Some(_)) => {
                        self.is_running_flag.store(false, Ordering::SeqCst);
                        false
                    }
                    Err(_) => false,
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub async fn terminate(
        &self,
        timeout_dur: Duration,
    ) -> Result<std::process::ExitStatus, StdioRunnerError> {
        if let Ok(mut tx_guard) = self.stdin_tx.lock() {
            tx_guard.take();
        }

        let maybe_child = {
            if let Ok(mut guard) = self.child.lock() {
                guard.take()
            } else {
                None
            }
        };

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

    pub fn into_channels(self) -> (mpsc::Sender<JsonRpcMessage>, mpsc::Receiver<JsonRpcMessage>) {
        let stdin_tx = self
            .stdin_tx
            .lock()
            .unwrap()
            .take()
            .expect("stdin_tx already taken");
        let stdout_rx = self
            .stdout_rx
            .lock()
            .unwrap()
            .take()
            .expect("stdout_rx already taken");
        (stdin_tx, stdout_rx)
    }
}
