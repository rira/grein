use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Failed to spawn subprocess: {0}")]
    SpawnFailed(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Process is not running")]
    NotRunning,
    #[error("Channel error: {0}")]
    Channel(String),
}

#[derive(Debug, Clone)]
pub struct SubprocessConfig {
    pub program: String,
    pub args: Vec<String>,
}

impl SubprocessConfig {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

impl JsonRpcRequest {
    pub fn new(
        id: impl Into<String>,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub id: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn new(id: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            method: method.into(),
            result: None,
            error: None,
            params: None,
        }
    }
}

pub struct SubprocessRunner {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>,
    stdout_task: Option<tokio::task::JoinHandle<()>>,
}

impl SubprocessRunner {
    pub async fn spawn(config: SubprocessConfig) -> Result<Self, RunnerError> {
        let mut command = Command::new(&config.program);
        command.args(&config.args);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| RunnerError::SpawnFailed(e.to_string()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RunnerError::Io("Subprocess stdin was not piped".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RunnerError::Io("Subprocess stdout was not piped".to_string()))?;

        let pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_requests_clone = Arc::clone(&pending_requests);

        let stdout_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    let mut pending = pending_requests_clone.lock().await;
                    if let Some(tx) = pending.remove(&response.id) {
                        let _ = tx.send(response);
                    }
                }
            }
        });

        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            stdin: Arc::new(Mutex::new(Some(stdin))),
            pending_requests,
            stdout_task: Some(stdout_task),
        })
    }

    pub async fn is_running(&mut self) -> bool {
        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut() {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }

    pub async fn send_request(
        &mut self,
        request: JsonRpcRequest,
    ) -> Result<oneshot::Receiver<JsonRpcResponse>, RunnerError> {
        let (tx, rx) = oneshot::channel();
        let id = request.id.clone();

        let mut json_line = serde_json::to_string(&request)
            .map_err(|e| RunnerError::Serialization(e.to_string()))?;
        json_line.push('\n');

        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(id.clone(), tx);
        }

        let write_result = {
            let mut stdin_guard = self.stdin.lock().await;
            if let Some(stdin) = stdin_guard.as_mut() {
                match stdin.write_all(json_line.as_bytes()).await {
                    Ok(_) => stdin.flush().await,
                    Err(e) => Err(e),
                }
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "Process stdin is not connected",
                ))
            }
        };

        if let Err(e) = write_result {
            let mut pending = self.pending_requests.lock().await;
            pending.remove(&id);
            return Err(RunnerError::Io(e.to_string()));
        }

        Ok(rx)
    }

    pub async fn shutdown(&mut self) -> Result<(), RunnerError> {
        {
            let mut stdin_guard = self.stdin.lock().await;
            let _ = stdin_guard.take();
        }

        if let Some(task) = self.stdout_task.take() {
            task.abort();
        }

        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            let wait_res = tokio::time::timeout(Duration::from_millis(500), child.wait()).await;
            if wait_res.is_err() || matches!(wait_res, Ok(Err(_))) {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }

        Ok(())
    }
}
