use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use grein_core::model::{AnchorId, AuthorRole, ConversationNode, NodeId, ThreadId};
use grein_core::storage::sqlite::SqliteStorage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
}

impl EngineError {
    fn rpc_code(&self) -> i64 {
        match self {
            Self::InvalidParams(_) => -32602,
            Self::NotFound(_) => -32004,
            Self::Protocol(_) => -32600,
            Self::Storage(_) | Self::Transport(_) => -32000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    TurnRecorded { node_id: String },
    ThreadForked { thread_id: String },
    ThreadMerged { thread_id: String },
    BlackboardUpserted { key: String },
    PermissionResolved { request_id: String, allow: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub thread_id: String,
    pub parent_node_id: String,
    pub merged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub node_id: String,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlackboardEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub node_count: usize,
    pub thread_count: usize,
    pub blackboard_entries: usize,
    pub threads: Vec<ThreadSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDispatchResult {
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDecisionPayload {
    pub request_id: String,
    pub allow: bool,
}

#[async_trait]
pub trait DaemonEngine: Send + Sync {
    async fn ping(&self) -> Result<(), EngineError>;
    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError>;
    async fn fork_thread(&self, parent_node_id: Option<&str>)
        -> Result<ThreadSummary, EngineError>;
    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError>;
    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError>;
    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError>;
    async fn blackboard_list(&self) -> Result<Vec<BlackboardEntry>, EngineError>;
    async fn blackboard_upsert(
        &self,
        key: &str,
        value: &str,
    ) -> Result<BlackboardEntry, EngineError>;
    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError>;
    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError>;
}

struct ThreadRecord {
    id: ThreadId,
    summary: ThreadSummary,
}

#[derive(Default)]
struct EngineState {
    node_ids: HashMap<String, NodeId>,
    transcript: Vec<TranscriptEntry>,
    last_node: Option<NodeId>,
    threads: Vec<ThreadRecord>,
    blackboard: BTreeMap<String, String>,
    decisions: Vec<PermissionDecisionPayload>,
}

/// Engine backed directly by `SqliteStorage` in the current process.
pub struct InProcessEngine {
    storage: SqliteStorage,
    state: Mutex<EngineState>,
}

impl InProcessEngine {
    pub fn open(db_path: &Path) -> Result<Self, EngineError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::Storage(e.to_string()))?;
        }
        let storage =
            SqliteStorage::open(db_path).map_err(|e| EngineError::Storage(e.to_string()))?;
        Ok(Self::from_storage(storage))
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        let storage =
            SqliteStorage::in_memory().map_err(|e| EngineError::Storage(e.to_string()))?;
        Ok(Self::from_storage(storage))
    }

    pub fn from_storage(storage: SqliteStorage) -> Self {
        Self {
            storage,
            state: Mutex::new(EngineState::default()),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, EngineState>, EngineError> {
        self.state
            .lock()
            .map_err(|_| EngineError::Storage("engine state poisoned".to_string()))
    }
}

#[async_trait]
impl DaemonEngine for InProcessEngine {
    async fn ping(&self) -> Result<(), EngineError> {
        self.lock().map(|_| ())
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        let mut state = self.lock()?;
        let node_id = NodeId::new();
        let node = ConversationNode::new(
            node_id,
            state.last_node,
            AuthorRole::User,
            prompt.to_string(),
        );
        self.storage
            .insert_node(&node)
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let id_text = node_id.to_string();
        state.node_ids.insert(id_text.clone(), node_id);
        state.last_node = Some(node_id);
        state.transcript.push(TranscriptEntry {
            node_id: id_text.clone(),
            role: "user".to_string(),
            content: prompt.to_string(),
        });
        Ok(TurnDispatchResult { node_id: id_text })
    }

    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut state = self.lock()?;
        let parent = match parent_node_id {
            Some(text) => state
                .node_ids
                .get(text)
                .copied()
                .ok_or_else(|| EngineError::NotFound(format!("node {text}")))?,
            None => state
                .last_node
                .ok_or_else(|| EngineError::NotFound("no turns recorded".to_string()))?,
        };
        let thread_id = self
            .storage
            .create_thread(parent, AnchorId::new())
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let summary = ThreadSummary {
            thread_id: format!("{thread_id:?}"),
            parent_node_id: parent.to_string(),
            merged: false,
        };
        state.threads.push(ThreadRecord {
            id: thread_id,
            summary: summary.clone(),
        });
        Ok(summary)
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        let state = self.lock()?;
        Ok(state.threads.iter().map(|t| t.summary.clone()).collect())
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        Ok(self.lock()?.transcript.clone())
    }

    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        let mut state = self.lock()?;
        let record = state
            .threads
            .iter_mut()
            .find(|t| t.summary.thread_id == thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        record.summary.merged = true;
        let _ = &record.id;
        Ok(record.summary.clone())
    }

    async fn blackboard_list(&self) -> Result<Vec<BlackboardEntry>, EngineError> {
        let state = self.lock()?;
        Ok(state
            .blackboard
            .iter()
            .map(|(k, v)| BlackboardEntry {
                key: k.clone(),
                value: v.clone(),
            })
            .collect())
    }

    async fn blackboard_upsert(
        &self,
        key: &str,
        value: &str,
    ) -> Result<BlackboardEntry, EngineError> {
        if key.is_empty() {
            return Err(EngineError::InvalidParams("key must not be empty".into()));
        }
        let mut state = self.lock()?;
        state.blackboard.insert(key.to_string(), value.to_string());
        Ok(BlackboardEntry {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        let mut state = self.lock()?;
        let event = EngineEvent::PermissionResolved {
            request_id: decision.request_id.clone(),
            allow: decision.allow,
        };
        state.decisions.push(decision);
        Ok(event)
    }

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        let persisted = self
            .storage
            .list_blackboard_entries()
            .map(|v| v.len())
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let state = self.lock()?;
        Ok(SessionSnapshot {
            node_count: state.transcript.len(),
            thread_count: state.threads.len(),
            blackboard_entries: persisted + state.blackboard.len(),
            threads: state.threads.iter().map(|t| t.summary.clone()).collect(),
        })
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<Value, EngineError> {
    serde_json::to_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, EngineError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::InvalidParams(format!("missing string param `{key}`")))
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn route(
    engine: &dyn DaemonEngine,
    method: &str,
    params: &Value,
) -> Result<Option<Value>, EngineError> {
    let value = match method {
        "session.ping" => {
            engine.ping().await?;
            json!({"pong": true})
        }
        "session.send_turn" => to_json(&engine.send_turn(required_str(params, "prompt")?).await?)?,
        "session.fork_thread" => {
            let parent = params.get("parent_node_id").and_then(Value::as_str);
            to_json(&engine.fork_thread(parent).await?)?
        }
        "session.list_threads" => to_json(&engine.list_threads().await?)?,
        "session.get_transcript" => to_json(&engine.get_transcript().await?)?,
        "session.merge_thread" => to_json(
            &engine
                .merge_thread(required_str(params, "thread_id")?)
                .await?,
        )?,
        "blackboard.list" => to_json(&engine.blackboard_list().await?)?,
        "blackboard.upsert" => {
            let key = required_str(params, "key")?;
            let value = required_str(params, "value")?;
            to_json(&engine.blackboard_upsert(key, value).await?)?
        }
        "permissions.resolve" => {
            let decision: PermissionDecisionPayload = serde_json::from_value(params.clone())
                .map_err(|e| EngineError::InvalidParams(e.to_string()))?;
            to_json(&engine.resolve_permission(decision).await?)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

/// Handles one JSON-RPC 2.0 request line and returns the serialized response.
pub async fn dispatch_json_rpc(engine: &dyn DaemonEngine, line: &str) -> String {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return rpc_error(Value::Null, -32700, format!("parse error: {e}")).to_string(),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return rpc_error(id, -32600, "invalid request: missing method".to_string()).to_string();
    };
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let response = match route(engine, method, &params).await {
        Ok(Some(result)) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Ok(None) => rpc_error(id, -32601, format!("method not found: {method}")),
        Err(e) => rpc_error(id, e.rpc_code(), e.to_string()),
    };
    response.to_string()
}

struct SocketIo {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

/// Remote engine speaking newline-delimited JSON-RPC 2.0 over a Unix socket.
pub struct UnixSocketEngineClient {
    io: AsyncMutex<SocketIo>,
    next_id: AtomicU64,
}

impl UnixSocketEngineClient {
    pub async fn connect<P: AsRef<Path>>(socket_path: P) -> Result<Self, EngineError> {
        let stream = UnixStream::connect(socket_path.as_ref())
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        let (read, writer) = stream.into_split();
        Ok(Self {
            io: AsyncMutex::new(SocketIo {
                reader: BufReader::new(read),
                writer,
            }),
            next_id: AtomicU64::new(1),
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, EngineError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut io = self.io.lock().await;
        io.writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        io.writer
            .flush()
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        let mut line = String::new();
        let read = io
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        if read == 0 {
            return Err(EngineError::Transport("daemon closed connection".into()));
        }
        let response: Value =
            serde_json::from_str(line.trim()).map_err(|e| EngineError::Protocol(e.to_string()))?;
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown rpc error");
            return Err(EngineError::Protocol(message.to_string()));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| EngineError::Protocol("response missing result".into()))
    }

    async fn call_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, EngineError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
    }
}

#[async_trait]
impl DaemonEngine for UnixSocketEngineClient {
    async fn ping(&self) -> Result<(), EngineError> {
        self.call("session.ping", json!({})).await.map(|_| ())
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        self.call_typed("session.send_turn", json!({"prompt": prompt}))
            .await
    }

    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let params = match parent_node_id {
            Some(p) => json!({"parent_node_id": p}),
            None => json!({}),
        };
        self.call_typed("session.fork_thread", params).await
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        self.call_typed("session.list_threads", json!({})).await
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.call_typed("session.get_transcript", json!({})).await
    }

    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        self.call_typed("session.merge_thread", json!({"thread_id": thread_id}))
            .await
    }

    async fn blackboard_list(&self) -> Result<Vec<BlackboardEntry>, EngineError> {
        self.call_typed("blackboard.list", json!({})).await
    }

    async fn blackboard_upsert(
        &self,
        key: &str,
        value: &str,
    ) -> Result<BlackboardEntry, EngineError> {
        self.call_typed("blackboard.upsert", json!({"key": key, "value": value}))
            .await
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        self.call_typed("permissions.resolve", to_json(&decision)?)
            .await
    }

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        let threads = self.list_threads().await?;
        let transcript = self.get_transcript().await?;
        let blackboard = self.blackboard_list().await?;
        Ok(SessionSnapshot {
            node_count: transcript.len(),
            thread_count: threads.len(),
            blackboard_entries: blackboard.len(),
            threads,
        })
    }
}

/// Unified engine: standalone in-process or remote daemon.
pub enum EngineClient {
    InProcess(InProcessEngine),
    Socket(UnixSocketEngineClient),
}

#[async_trait]
impl DaemonEngine for EngineClient {
    async fn ping(&self) -> Result<(), EngineError> {
        match self {
            Self::InProcess(e) => e.ping().await,
            Self::Socket(e) => e.ping().await,
        }
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        match self {
            Self::InProcess(e) => e.send_turn(prompt).await,
            Self::Socket(e) => e.send_turn(prompt).await,
        }
    }

    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => e.fork_thread(parent_node_id).await,
            Self::Socket(e) => e.fork_thread(parent_node_id).await,
        }
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        match self {
            Self::InProcess(e) => e.list_threads().await,
            Self::Socket(e) => e.list_threads().await,
        }
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        match self {
            Self::InProcess(e) => e.get_transcript().await,
            Self::Socket(e) => e.get_transcript().await,
        }
    }

    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => e.merge_thread(thread_id).await,
            Self::Socket(e) => e.merge_thread(thread_id).await,
        }
    }

    async fn blackboard_list(&self) -> Result<Vec<BlackboardEntry>, EngineError> {
        match self {
            Self::InProcess(e) => e.blackboard_list().await,
            Self::Socket(e) => e.blackboard_list().await,
        }
    }

    async fn blackboard_upsert(
        &self,
        key: &str,
        value: &str,
    ) -> Result<BlackboardEntry, EngineError> {
        match self {
            Self::InProcess(e) => e.blackboard_upsert(key, value).await,
            Self::Socket(e) => e.blackboard_upsert(key, value).await,
        }
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        match self {
            Self::InProcess(e) => e.resolve_permission(decision).await,
            Self::Socket(e) => e.resolve_permission(decision).await,
        }
    }

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        match self {
            Self::InProcess(e) => e.snapshot().await,
            Self::Socket(e) => e.snapshot().await,
        }
    }
}
