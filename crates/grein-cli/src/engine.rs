use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use grein_acp::{
    AcpPermissionOption, AcpStdioDriver, DriverEvent, StdioProcessConfig, StdioProcessRunner,
};
use grein_core::blackboard::{Blackboard, BlackboardEntry as CoreBlackboardEntry, BlackboardEntryKind};
use grein_core::model::{
    Anchor, AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadId, ThreadStatus,
};
use grein_core::storage::sqlite::SqliteStorage;
use grein_core::sync::{format_whiteboard_delta_prefix, ContextCompiler};
use grein_tui::engine_port::{HydratedThread, HydratedTurn, TuiEngineEvent};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::discovery::{discover_acp_agents, AcpAgentKind};

const INTERNAL_ROOT_CONTENT: &str = "Session initialized";
const ACP_SYSTEM_PROMPT_MAX_CHARS: usize = 32_000;
const SHELL_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const SHELL_MAX_OUTPUT_BYTES: usize = 65_536;

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

fn storage_err(err: impl std::fmt::Display) -> EngineError {
    EngineError::Storage(err.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpBackendConfig {
    pub command: String,
    pub args: Vec<String>,
    pub auth_method: Option<String>,
    pub cwd: PathBuf,
}

impl AcpBackendConfig {
    pub fn discover_default(cwd: PathBuf) -> Option<Self> {
        let bin_env = std::env::var("GREIN_ACP_BIN").ok();
        let args_env = std::env::var("GREIN_ACP_ARGS").ok();
        let auth_env = std::env::var("GREIN_ACP_AUTH_METHOD").ok();
        Self::discover_default_from(cwd, bin_env, args_env, auth_env, None)
    }

    pub fn discover_default_from(
        cwd: PathBuf,
        bin_env: Option<String>,
        args_env: Option<String>,
        auth_env: Option<String>,
        search_path: Option<&str>,
    ) -> Option<Self> {
        let auth_override = auth_env.map(|raw| {
            let trimmed = raw.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        });

        if let Some(bin) = bin_env {
            let command = bin.trim();
            if command.is_empty() {
                return None;
            }
            let args = args_env
                .map(|raw| raw.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default();
            return Some(Self {
                command: command.to_string(),
                args,
                auth_method: auth_override.unwrap_or(None),
                cwd,
            });
        }

        let agents = discover_acp_agents(search_path);
        if let Some(agy) = agents.iter().find(|a| a.kind == AcpAgentKind::Agy) {
            return Some(Self {
                command: agy.executable_path.to_string_lossy().into_owned(),
                args: vec!["acp".to_string(), "--stdio".to_string()],
                auth_method: auth_override.unwrap_or_else(|| Some("oauth-personal".to_string())),
                cwd,
            });
        }
        if let Some(claude) = agents.iter().find(|a| a.kind == AcpAgentKind::Claude) {
            return Some(Self {
                command: claude.executable_path.to_string_lossy().into_owned(),
                args: vec!["--acp".to_string()],
                auth_method: auth_override.unwrap_or(None),
                cwd,
            });
        }
        None
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
    #[serde(default)]
    pub title: String,
    pub parent_node_id: String,
    #[serde(default)]
    pub active_leaf_id: String,
    pub merged: bool,
    #[serde(default)]
    pub status: ThreadStatus,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandResult {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

#[async_trait]
pub trait DaemonEngine: Send + Sync {
    async fn ping(&self) -> Result<(), EngineError>;
    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError>;
    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError>;
    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError>;
    async fn fork_thread(&self, parent_node_id: Option<&str>)
        -> Result<ThreadSummary, EngineError>;
    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError>;
    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError>;
    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError>;
    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError>;
    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError>;
    async fn blackboard_list(&self) -> Result<Vec<BlackboardEntry>, EngineError>;
    async fn blackboard_upsert(
        &self,
        key: &str,
        value: &str,
    ) -> Result<BlackboardEntry, EngineError>;
    async fn blackboard_delete(&self, key: &str) -> Result<bool, EngineError>;
    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError>;
    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError>;
    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError>;
    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError>;
    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
        Ok(None)
    }
}

#[derive(Clone)]
struct ActiveAcpSession {
    session_id: String,
    last_whiteboard_hash: String,
    driver_gen: u64,
}

struct PendingPermissionRecord {
    driver: Arc<AcpStdioDriver>,
    json_rpc_id: Value,
    options: Vec<AcpPermissionOption>,
}

struct DriverSlot {
    driver: Arc<AcpStdioDriver>,
    generation: u64,
}

#[derive(Default)]
struct EngineState {
    last_node: Option<NodeId>,
    decisions: Vec<PermissionDecisionPayload>,
    active_acp_sessions: HashMap<String, ActiveAcpSession>,
    pending_permissions: HashMap<String, PendingPermissionRecord>,
}

/// Engine backed directly by `SqliteStorage` and live ACP v1 stdio drivers.
pub struct InProcessEngine {
    storage: SqliteStorage,
    cwd: PathBuf,
    acp_config: Option<AcpBackendConfig>,
    acp_driver: AsyncMutex<Option<DriverSlot>>,
    driver_gen_seq: AtomicU64,
    state: Mutex<EngineState>,
    tui_subscribers: Mutex<Vec<mpsc::UnboundedSender<TuiEngineEvent>>>,
    next_perm_seq: AtomicU64,
}

fn parse_typed_id<T: DeserializeOwned>(raw: &str) -> Option<T> {
    let trimmed = raw.trim();
    let normalized = trimmed
        .strip_prefix("ThreadId(")
        .or_else(|| trimmed.strip_prefix("NodeId("))
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(trimmed);
    serde_json::from_value(Value::String(normalized.to_string())).ok()
}

fn slugify_anchor(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut prev_dash = false;
    for c in title.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "branch".to_string()
    } else {
        slug
    }
}

fn is_internal_root_node(node: &ConversationNode) -> bool {
    node.parent_id.is_none()
        && node.role == AuthorRole::System
        && (node.content.is_empty() || node.content == INTERNAL_ROOT_CONTENT)
}

fn thread_to_summary(thread: &Thread, is_first: bool) -> ThreadSummary {
    let title = if !thread.title.trim().is_empty() {
        thread.title.clone()
    } else if is_first {
        "Main".to_string()
    } else {
        "Branch".to_string()
    };
    ThreadSummary {
        thread_id: thread.id.0.to_string(),
        title,
        parent_node_id: thread.parent_node_id.0.to_string(),
        active_leaf_id: thread.active_leaf_id.0.to_string(),
        merged: thread.merged,
        status: thread.status,
    }
}

fn node_to_transcript_entry(node: &ConversationNode) -> TranscriptEntry {
    TranscriptEntry {
        node_id: node.id.0.to_string(),
        role: node.role.to_string(),
        content: node.content.clone(),
    }
}

fn whiteboard_hash(blackboard: &Blackboard) -> String {
    let md = blackboard.to_markdown();
    let mut hash: u128 = 0x6c62272e07bb0142_62b821756295c58d;
    const PRIME: u128 = 0x0000000001000000_000000000000013b;
    for byte in md.as_bytes() {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:032x}")
}

fn select_permission_option(options: &[AcpPermissionOption], approved: bool) -> String {
    if approved {
        options
            .iter()
            .find(|o| o.kind.starts_with("allow") || o.option_id == "allow")
            .map(|o| o.option_id.clone())
            .unwrap_or_else(|| "allow".to_string())
    } else {
        options
            .iter()
            .find(|o| o.kind.starts_with("reject") || o.option_id == "deny")
            .map(|o| o.option_id.clone())
            .unwrap_or_else(|| "deny".to_string())
    }
}

fn format_permission_description(tool_name: &str, title: &str, args: &Value) -> String {
    match (tool_name.trim().is_empty(), title.trim().is_empty()) {
        (false, false) => format!("{tool_name}: {title}"),
        (false, true) if !args.is_null() => format!("{tool_name} {args}"),
        (false, true) => tool_name.to_string(),
        (true, false) => title.to_string(),
        (true, true) => "Permission requested".to_string(),
    }
}

impl InProcessEngine {
    pub fn open(db_path: &Path) -> Result<Self, EngineError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let acp_config = AcpBackendConfig::discover_default(cwd.clone());
        Self::open_with_config(db_path, cwd, acp_config)
    }

    pub fn open_with_config(
        db_path: &Path,
        cwd: PathBuf,
        acp_config: Option<AcpBackendConfig>,
    ) -> Result<Self, EngineError> {
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(storage_err)?;
            }
        }
        let storage = SqliteStorage::open(db_path).map_err(storage_err)?;
        Self::from_storage_with_config(storage, cwd, acp_config)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        let storage = SqliteStorage::in_memory().map_err(storage_err)?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::from_storage_with_config(storage, cwd, None)
    }

    pub fn from_storage(storage: SqliteStorage) -> Result<Self, EngineError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::from_storage_with_config(storage, cwd, None)
    }

    pub fn from_storage_with_config(
        storage: SqliteStorage,
        cwd: PathBuf,
        acp_config: Option<AcpBackendConfig>,
    ) -> Result<Self, EngineError> {
        let nodes = storage.list_nodes().map_err(storage_err)?;
        let threads = storage.list_threads().map_err(storage_err)?;
        let _blackboard = storage.list_blackboard_entries().map_err(storage_err)?;
        let last_node = threads
            .first()
            .map(|t| t.active_leaf_id)
            .or_else(|| nodes.last().map(|n| n.id));

        Ok(Self {
            storage,
            cwd,
            acp_config,
            acp_driver: AsyncMutex::new(None),
            driver_gen_seq: AtomicU64::new(1),
            state: Mutex::new(EngineState {
                last_node,
                ..Default::default()
            }),
            tui_subscribers: Mutex::new(Vec::new()),
            next_perm_seq: AtomicU64::new(1),
        })
    }

    pub fn subscribe_tui_events(&self) -> mpsc::UnboundedReceiver<TuiEngineEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut subs) = self.tui_subscribers.lock() {
            subs.push(tx);
        }
        rx
    }

    fn emit_tui(&self, event: TuiEngineEvent) {
        if let Ok(mut subs) = self.tui_subscribers.lock() {
            subs.retain(|tx| tx.send(event.clone()).is_ok());
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, EngineState>, EngineError> {
        self.state
            .lock()
            .map_err(|_| EngineError::Storage("engine state poisoned".to_string()))
    }

    fn load_thread_turns(&self, thread: &Thread) -> Result<Vec<TranscriptEntry>, EngineError> {
        let path = self
            .storage
            .get_trunk_path(thread.active_leaf_id)
            .map_err(storage_err)?;
        Ok(path
            .into_iter()
            .filter(|n| !is_internal_root_node(n))
            .map(|n| node_to_transcript_entry(&n))
            .collect())
    }

    fn mark_thread_error(&self, tid: ThreadId, thread_id: &str, message: &str) {
        let _ = self.storage.update_thread_status(tid, ThreadStatus::Error);
        self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "error".to_string(),
        });
        self.emit_tui(TuiEngineEvent::Error {
            message: message.to_string(),
        });
    }

    async fn ensure_driver(&self, cfg: &AcpBackendConfig) -> Result<(Arc<AcpStdioDriver>, u64), EngineError> {
        let mut guard = self.acp_driver.lock().await;
        if let Some(slot) = guard.as_ref() {
            if slot.driver.is_running() {
                return Ok((Arc::clone(&slot.driver), slot.generation));
            }
            *guard = None;
        }

        let spawn_cfg = StdioProcessConfig::new(&cfg.command, cfg.args.clone())
            .with_current_dir(&cfg.cwd);
        let runner = StdioProcessRunner::spawn(spawn_cfg).await.map_err(|e| {
            EngineError::Transport(format!("failed to spawn ACP agent `{}`: {e}", cfg.command))
        })?;
        let driver = Arc::new(AcpStdioDriver::new(runner));
        if let Err(e) = driver.initialize().await {
            let _ = driver.shutdown().await;
            *guard = None;
            return Err(EngineError::Transport(format!("ACP initialize failed: {e}")));
        }
        if let Some(method) = cfg.auth_method.as_deref().filter(|m| !m.is_empty()) {
            if let Err(e) = driver.authenticate(method).await {
                let _ = driver.shutdown().await;
                *guard = None;
                return Err(EngineError::Transport(format!(
                    "ACP authenticate ({method}) failed: {e}"
                )));
            }
        }
        let generation = self.driver_gen_seq.fetch_add(1, Ordering::Relaxed);
        *guard = Some(DriverSlot {
            driver: Arc::clone(&driver),
            generation,
        });
        Ok((driver, generation))
    }

    async fn invalidate_driver_if_dead(&self, driver: &Arc<AcpStdioDriver>, generation: u64) {
        if driver.is_running() {
            return;
        }
        let mut guard = self.acp_driver.lock().await;
        if guard.as_ref().is_some_and(|s| s.generation == generation) {
            *guard = None;
        }
        if let Ok(mut state) = self.state.lock() {
            state
                .active_acp_sessions
                .retain(|_, s| s.driver_gen != generation);
        }
    }

    pub async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::ensure_main_thread(self).await
    }

    pub async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        DaemonEngine::send_turn_for_thread(self, thread_id, prompt).await
    }

    pub async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::fork_thread_with_title(self, source_thread_or_node, title).await
    }

    pub async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        DaemonEngine::get_thread_transcript(self, thread_id).await
    }

    pub async fn blackboard_delete(&self, key: &str) -> Result<bool, EngineError> {
        DaemonEngine::blackboard_delete(self, key).await
    }

    pub async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        DaemonEngine::cancel_turn(self, thread_id).await
    }

    pub async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        DaemonEngine::run_shell_command(self, thread_id, command).await
    }
}

#[async_trait]
impl DaemonEngine for InProcessEngine {
    async fn ping(&self) -> Result<(), EngineError> {
        self.lock().map(|_| ())
    }

    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        let mut state = self.lock()?;
        let threads = self.storage.list_threads().map_err(storage_err)?;
        if let Some(first) = threads.first() {
            return Ok(thread_to_summary(first, true));
        }

        let nodes = self.storage.list_nodes().map_err(storage_err)?;
        let (root_id, leaf_id) = if let (Some(first), Some(last)) = (nodes.first(), nodes.last()) {
            (first.id, last.id)
        } else {
            let root = ConversationNode::new(
                NodeId::new(),
                None::<NodeId>,
                AuthorRole::System,
                INTERNAL_ROOT_CONTENT.to_string(),
            );
            self.storage.insert_node(&root).map_err(storage_err)?;
            (root.id, root.id)
        };

        let anchor_id = AnchorId::new();
        let anchor = Anchor {
            level: 1,
            title: "Main".to_string(),
            slug: "main".to_string(),
            line_number: 1,
        };
        self.storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;
        let tid = self
            .storage
            .create_thread_with_title(root_id, anchor_id, "Main")
            .map_err(storage_err)?;
        if leaf_id != root_id {
            self.storage
                .update_thread_leaf(tid, leaf_id)
                .map_err(storage_err)?;
        }
        state.last_node = Some(leaf_id);
        let created = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("created Main thread missing".to_string()))?;
        Ok(thread_to_summary(&created, true))
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        if self.acp_config.is_none() {
            let mut state = self.lock()?;
            let threads = self.storage.list_threads().map_err(storage_err)?;
            let parent = threads.first().map(|t| t.active_leaf_id).or(state.last_node);
            let node_id = NodeId::new();
            let node = ConversationNode::new(node_id, parent, AuthorRole::User, prompt.to_string());
            self.storage.insert_node(&node).map_err(storage_err)?;
            if let Some(first) = threads.first() {
                self.storage
                    .update_thread_leaf(first.id, node_id)
                    .map_err(storage_err)?;
            }
            state.last_node = Some(node_id);
            return Ok(TurnDispatchResult {
                node_id: node_id.0.to_string(),
            });
        }

        let main = DaemonEngine::ensure_main_thread(self).await?;
        DaemonEngine::send_turn_for_thread(self, &main.thread_id, prompt).await
    }

    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        if prompt.is_empty() {
            return Err(EngineError::InvalidParams(
                "prompt must not be empty".to_string(),
            ));
        }

        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let user_node_id = NodeId::new();
        let user_node = ConversationNode::new(
            user_node_id,
            Some(thread.active_leaf_id),
            AuthorRole::User,
            prompt.to_string(),
        );
        self.storage.insert_node(&user_node).map_err(storage_err)?;
        self.storage
            .update_thread_leaf(tid, user_node_id)
            .map_err(storage_err)?;
        self.storage
            .update_thread_status(tid, ThreadStatus::Streaming)
            .map_err(storage_err)?;
        {
            let mut state = self.lock()?;
            state.last_node = Some(user_node_id);
        }

        self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "streaming".to_string(),
        });

        let Some(cfg) = self.acp_config.clone() else {
            let msg = "no ACP agent configured; install agy or set GREIN_ACP_BIN".to_string();
            self.mark_thread_error(tid, thread_id, &msg);
            return Err(EngineError::Transport(msg));
        };

        let (driver, driver_gen) = match self.ensure_driver(&cfg).await {
            Ok(pair) => pair,
            Err(err) => {
                self.mark_thread_error(tid, thread_id, &err.to_string());
                return Err(err);
            }
        };

        let entries = self.storage.list_blackboard_entries().map_err(storage_err)?;
        let blackboard = Blackboard::from_entries(entries);
        let current_wb_hash = whiteboard_hash(&blackboard);

        let existing_session = {
            let state = self.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .filter(|s| s.driver_gen == driver_gen)
                .cloned()
        };

        let (session_id, outgoing_prompt) = match existing_session {
            Some(active) => {
                let prompt_text = if active.last_whiteboard_hash != current_wb_hash {
                    let delta = format_whiteboard_delta_prefix(&blackboard);
                    let updated = if delta.is_empty() {
                        format!("[Shared Whiteboard Cleared]\n\n---\n\n{prompt}")
                    } else {
                        format!("{delta}{prompt}")
                    };
                    let mut state = self.lock()?;
                    if let Some(entry) = state.active_acp_sessions.get_mut(thread_id) {
                        entry.last_whiteboard_hash = current_wb_hash;
                    }
                    updated
                } else {
                    prompt.to_string()
                };
                (active.session_id, prompt_text)
            }
            None => {
                let mut payload = ContextCompiler::new("")
                    .compile_thread_context(&self.storage, tid, &blackboard)
                    .map_err(storage_err)?;
                payload.trunk_path.retain(|n| !is_internal_root_node(n));
                payload.branch_turns.retain(|n| n.id != user_node_id);
                let include_branch_turns = !payload.branch_turns.is_empty();
                let append = payload
                    .build_acp_system_prompt_append(include_branch_turns, ACP_SYSTEM_PROMPT_MAX_CHARS);
                let append_opt = (!append.trim().is_empty()).then_some(append.as_str());

                let info = match driver.new_session(&cfg.cwd, append_opt).await {
                    Ok(info) => info,
                    Err(err) => {
                        self.invalidate_driver_if_dead(&driver, driver_gen).await;
                        let msg = format!("ACP new_session failed: {err}");
                        self.mark_thread_error(tid, thread_id, &msg);
                        return Err(EngineError::Transport(msg));
                    }
                };

                self.storage
                    .update_thread_acp_session_id(&tid, Some(&info.session_id))
                    .map_err(storage_err)?;
                {
                    let mut state = self.lock()?;
                    state.active_acp_sessions.insert(
                        thread_id.to_string(),
                        ActiveAcpSession {
                            session_id: info.session_id.clone(),
                            last_whiteboard_hash: current_wb_hash,
                            driver_gen,
                        },
                    );
                }
                (info.session_id, prompt.to_string())
            }
        };

        let mut events = match driver.send_prompt(&session_id, &outgoing_prompt).await {
            Ok(rx) => rx,
            Err(err) => {
                self.invalidate_driver_if_dead(&driver, driver_gen).await;
                let msg = format!("ACP send_prompt failed: {err}");
                self.mark_thread_error(tid, thread_id, &msg);
                return Err(EngineError::Transport(msg));
            }
        };

        let mut assistant_acc = String::new();
        let mut leaf_id = user_node_id;
        let mut completed = false;
        let mut saw_error: Option<String> = None;

        while let Some(ev) = events.recv().await {
            match ev {
                DriverEvent::StreamChunk { text, .. } => {
                    assistant_acc.push_str(&text);
                    self.emit_tui(TuiEngineEvent::StreamChunk {
                        thread_id: thread_id.to_string(),
                        content: text,
                    });
                }
                DriverEvent::ThoughtChunk { text, .. } => {
                    self.emit_tui(TuiEngineEvent::ThoughtChunk {
                        thread_id: thread_id.to_string(),
                        content: text,
                    });
                }
                DriverEvent::ToolCall { name, title, .. } => {
                    let tool_name = if !name.is_empty() { name } else { title };
                    self.emit_tui(TuiEngineEvent::ToolInvoked {
                        thread_id: thread_id.to_string(),
                        tool_name,
                    });
                }
                DriverEvent::ToolCallUpdate {
                    name,
                    status,
                    output,
                    ..
                } => {
                    if status.eq_ignore_ascii_case("completed")
                        || status.eq_ignore_ascii_case("failed")
                        || status.eq_ignore_ascii_case("error")
                    {
                        let tool_name = if !name.is_empty() {
                            name
                        } else {
                            "tool".to_string()
                        };
                        let resolved_output =
                            if output.is_empty() && !status.eq_ignore_ascii_case("completed") {
                                status
                            } else {
                                output
                            };
                        self.emit_tui(TuiEngineEvent::ToolResult {
                            thread_id: thread_id.to_string(),
                            tool_name,
                            output: resolved_output,
                        });
                    }
                }
                DriverEvent::SessionInfoUpdate { title, .. } => {
                    let trimmed = title.trim();
                    if !trimmed.is_empty() {
                        if let Some(current) = self.storage.get_thread(tid).map_err(storage_err)? {
                            if current.title.trim().is_empty() || current.title.starts_with("Branch")
                            {
                                self.storage
                                    .update_thread_title(&tid, trimmed)
                                    .map_err(storage_err)?;
                                self.emit_tui(TuiEngineEvent::ThreadTitleUpdated {
                                    thread_id: thread_id.to_string(),
                                    title: trimmed.to_string(),
                                });
                            }
                        }
                    }
                }
                DriverEvent::UsageUpdate { .. } => {}
                DriverEvent::PermissionRequested {
                    request_id,
                    tool_name,
                    title,
                    args,
                    options,
                    ..
                } => {
                    let seq = self.next_perm_seq.fetch_add(1, Ordering::Relaxed);
                    let request_key = format!("{session_id}:{seq}");
                    let description = format_permission_description(&tool_name, &title, &args);
                    {
                        let mut state = self.lock()?;
                        state.pending_permissions.insert(
                            request_key.clone(),
                            PendingPermissionRecord {
                                driver: Arc::clone(&driver),
                                json_rpc_id: request_id,
                                options,
                            },
                        );
                    }
                    let _ = self
                        .storage
                        .update_thread_status(tid, ThreadStatus::PermissionRequired);
                    self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                        thread_id: thread_id.to_string(),
                        status: "permission_required".to_string(),
                    });
                    self.emit_tui(TuiEngineEvent::PermissionRequested {
                        request_id: request_key,
                        description,
                    });
                }
                DriverEvent::PromptCompleted { .. } => {
                    completed = true;
                    if !assistant_acc.is_empty() {
                        let asst_node_id = NodeId::new();
                        let asst_node = ConversationNode::new(
                            asst_node_id,
                            Some(leaf_id),
                            AuthorRole::Assistant,
                            std::mem::take(&mut assistant_acc),
                        );
                        self.storage.insert_node(&asst_node).map_err(storage_err)?;
                        self.storage
                            .update_thread_leaf(tid, asst_node_id)
                            .map_err(storage_err)?;
                        leaf_id = asst_node_id;
                        if let Ok(mut state) = self.lock() {
                            state.last_node = Some(asst_node_id);
                        }
                    }
                    self.storage
                        .update_thread_status(tid, ThreadStatus::Idle)
                        .map_err(storage_err)?;
                    self.emit_tui(TuiEngineEvent::TurnCompleted {
                        thread_id: thread_id.to_string(),
                    });
                    self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                        thread_id: thread_id.to_string(),
                        status: "idle".to_string(),
                    });
                    break;
                }
                DriverEvent::Error { message, .. } => {
                    self.mark_thread_error(tid, thread_id, &message);
                    saw_error = Some(message);
                    break;
                }
            }
        }

        self.invalidate_driver_if_dead(&driver, driver_gen).await;

        if let Some(msg) = saw_error {
            return Err(EngineError::Transport(msg));
        }
        if !completed {
            let msg = "ACP stream closed before PromptCompleted".to_string();
            self.mark_thread_error(tid, thread_id, &msg);
            return Err(EngineError::Transport(msg));
        }

        Ok(TurnDispatchResult {
            node_id: leaf_id.0.to_string(),
        })
    }

    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::fork_thread_with_title(self, parent_node_id, None).await
    }

    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let existing_threads = self.storage.list_threads().map_err(storage_err)?;
        let parent_node_id = match source_thread_or_node.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                if let Some(tid) = parse_typed_id::<ThreadId>(raw) {
                    if let Some(thread) = self.storage.get_thread(tid).map_err(storage_err)? {
                        thread.active_leaf_id
                    } else if let Some(node) =
                        self.storage.get_node(NodeId(tid.0)).map_err(storage_err)?
                    {
                        node.id
                    } else {
                        return Err(EngineError::NotFound(format!("thread or node {raw}")));
                    }
                } else {
                    return Err(EngineError::NotFound(format!("thread or node {raw}")));
                }
            }
            None => {
                if let Some(first) = existing_threads.first() {
                    first.active_leaf_id
                } else if let Some(last_node) = self.lock()?.last_node {
                    last_node
                } else {
                    return Err(EngineError::NotFound("no turns recorded".to_string()));
                }
            }
        };

        let resolved_title = match title.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => t.to_string(),
            None => {
                if existing_threads.is_empty() {
                    "Branch".to_string()
                } else {
                    format!("Branch {}", existing_threads.len() + 1)
                }
            }
        };

        let anchor_id = AnchorId::new();
        let anchor = Anchor {
            level: 1,
            title: resolved_title.clone(),
            slug: slugify_anchor(&resolved_title),
            line_number: 1,
        };
        self.storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;
        let tid = self
            .storage
            .create_thread_with_title(parent_node_id, anchor_id, &resolved_title)
            .map_err(storage_err)?;
        let created = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("forked thread missing".to_string()))?;

        let turns: Vec<HydratedTurn> = self
            .load_thread_turns(&created)?
            .into_iter()
            .map(|e| HydratedTurn {
                id: e.node_id,
                role: e.role,
                content: e.content,
            })
            .collect();

        let summary = thread_to_summary(&created, false);
        self.emit_tui(TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: summary.thread_id.clone(),
                title: summary.title.clone(),
                turns,
            },
        });
        Ok(summary)
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        let threads = self.storage.list_threads().map_err(storage_err)?;
        Ok(threads
            .iter()
            .enumerate()
            .map(|(i, t)| thread_to_summary(t, i == 0))
            .collect())
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        let threads = self.storage.list_threads().map_err(storage_err)?;
        if let Some(first) = threads.first() {
            return self.load_thread_turns(first);
        }
        let nodes = self.storage.list_nodes().map_err(storage_err)?;
        Ok(nodes
            .into_iter()
            .filter(|n| !is_internal_root_node(n))
            .map(|n| node_to_transcript_entry(&n))
            .collect())
    }

    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        self.load_thread_turns(&thread)
    }

    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if self.storage.get_thread(tid).map_err(storage_err)?.is_none() {
            return Err(EngineError::NotFound(format!("thread {thread_id}")));
        }
        self.storage
            .mark_thread_merged(&tid, true)
            .map_err(storage_err)?;
        let updated = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        Ok(thread_to_summary(&updated, false))
    }

    async fn blackboard_list(&self) -> Result<Vec<BlackboardEntry>, EngineError> {
        let entries = self.storage.list_blackboard_entries().map_err(storage_err)?;
        Ok(entries
            .into_iter()
            .map(|e| BlackboardEntry {
                key: e.key,
                value: e.value,
            })
            .collect())
    }

    async fn blackboard_upsert(
        &self,
        key: &str,
        value: &str,
    ) -> Result<BlackboardEntry, EngineError> {
        if key.trim().is_empty() {
            return Err(EngineError::InvalidParams("key must not be empty".into()));
        }
        let entry = CoreBlackboardEntry::new(key, value, BlackboardEntryKind::Note);
        self.storage
            .insert_blackboard_entry(&entry)
            .map_err(storage_err)?;
        self.emit_tui(TuiEngineEvent::BlackboardUpdated {
            key: key.to_string(),
            value: value.to_string(),
        });
        Ok(BlackboardEntry {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    async fn blackboard_delete(&self, key: &str) -> Result<bool, EngineError> {
        if key.trim().is_empty() {
            return Err(EngineError::InvalidParams("key must not be empty".into()));
        }
        let deleted = self
            .storage
            .delete_blackboard_entry(key)
            .map_err(storage_err)?;
        if deleted {
            self.emit_tui(TuiEngineEvent::BlackboardDeleted {
                key: key.to_string(),
            });
        }
        Ok(deleted)
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        let pending = {
            let mut state = self.lock()?;
            state.decisions.push(decision.clone());
            state.pending_permissions.remove(&decision.request_id)
        };
        if let Some(record) = pending {
            let option_id = select_permission_option(&record.options, decision.allow);
            record
                .driver
                .respond_permission(record.json_rpc_id, Some(&option_id))
                .await
                .map_err(|e| EngineError::Transport(format!("respond_permission failed: {e}")))?;
        }
        Ok(EngineEvent::PermissionResolved {
            request_id: decision.request_id,
            allow: decision.allow,
        })
    }

    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        let session_id = {
            let state = self.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .map(|s| s.session_id.clone())
        };
        let Some(sid) = session_id else {
            return Ok(());
        };
        let driver = {
            let guard = self.acp_driver.lock().await;
            guard.as_ref().map(|s| Arc::clone(&s.driver))
        };
        if let Some(driver) = driver {
            driver
                .cancel_session(&sid)
                .await
                .map_err(|e| EngineError::Transport(format!("cancel_session failed: {e}")))?;
        }
        Ok(())
    }

    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        if command.trim().is_empty() {
            return Err(EngineError::InvalidParams(
                "command must not be empty".to_string(),
            ));
        }
        let cwd = self.cwd.clone();
        let cmd = command.to_string();
        let output = tokio::task::spawn_blocking(move || {
            grein_workspace::shell::run_shell_command_blocking(
                &cwd,
                &cmd,
                SHELL_COMMAND_TIMEOUT,
                SHELL_MAX_OUTPUT_BYTES,
            )
        })
        .await
        .map_err(|e| EngineError::Transport(format!("shell task panicked: {e}")))?
        .map_err(|e| EngineError::Transport(format!("shell execution failed: {e}")))?;

        let result = ShellCommandResult {
            command: output.command,
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            timed_out: output.timed_out,
        };
        self.emit_tui(TuiEngineEvent::ShellOutput {
            thread_id: thread_id.to_string(),
            command: result.command.clone(),
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
            exit_code: result.exit_code,
        });
        Ok(result)
    }

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        let threads = DaemonEngine::list_threads(self).await?;
        let transcript = DaemonEngine::get_transcript(self).await?;
        let blackboard = DaemonEngine::blackboard_list(self).await?;
        Ok(SessionSnapshot {
            node_count: transcript.len(),
            thread_count: threads.len(),
            blackboard_entries: blackboard.len(),
            threads,
        })
    }

    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
        Ok(Some(InProcessEngine::subscribe_tui_events(self)))
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
        "session.ensure_main_thread" => to_json(&engine.ensure_main_thread().await?)?,
        "session.send_turn" => {
            let prompt = required_str(params, "prompt")?;
            if let Some(tid) = params.get("thread_id").and_then(Value::as_str) {
                to_json(&engine.send_turn_for_thread(tid, prompt).await?)?
            } else {
                to_json(&engine.send_turn(prompt).await?)?
            }
        }
        "session.send_thread_turn" => {
            let thread_id = required_str(params, "thread_id")?;
            let prompt = required_str(params, "prompt")?;
            to_json(&engine.send_turn_for_thread(thread_id, prompt).await?)?
        }
        "session.fork_thread" => {
            let source = params
                .get("parent_node_id")
                .or_else(|| params.get("source_thread_id"))
                .and_then(Value::as_str);
            let title = params.get("title").and_then(Value::as_str);
            to_json(&engine.fork_thread_with_title(source, title).await?)?
        }
        "session.list_threads" => to_json(&engine.list_threads().await?)?,
        "session.get_transcript" => {
            if let Some(tid) = params.get("thread_id").and_then(Value::as_str) {
                to_json(&engine.get_thread_transcript(tid).await?)?
            } else {
                to_json(&engine.get_transcript().await?)?
            }
        }
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
        "blackboard.delete" => {
            let key = required_str(params, "key")?;
            let deleted = engine.blackboard_delete(key).await?;
            json!({"deleted": deleted})
        }
        "permissions.resolve" => {
            let decision: PermissionDecisionPayload = serde_json::from_value(params.clone())
                .map_err(|e| EngineError::InvalidParams(e.to_string()))?;
            to_json(&engine.resolve_permission(decision).await?)?
        }
        "session.cancel" => {
            let thread_id = required_str(params, "thread_id")?;
            engine.cancel_turn(thread_id).await?;
            json!({"cancelled": true})
        }
        "session.shell" => {
            let thread_id = required_str(params, "thread_id")?;
            let command = required_str(params, "command")?;
            to_json(&engine.run_shell_command(thread_id, command).await?)?
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
    socket_path: PathBuf,
    io: AsyncMutex<SocketIo>,
    next_id: AtomicU64,
}

impl UnixSocketEngineClient {
    pub async fn connect<P: AsRef<Path>>(socket_path: P) -> Result<Self, EngineError> {
        let socket_path_buf = socket_path.as_ref().to_path_buf();
        let stream = UnixStream::connect(&socket_path_buf)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        let (read, writer) = stream.into_split();
        Ok(Self {
            socket_path: socket_path_buf,
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

    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        self.call_typed("session.ensure_main_thread", json!({}))
            .await
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        self.call_typed("session.send_turn", json!({"prompt": prompt}))
            .await
    }

    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        self.call_typed(
            "session.send_thread_turn",
            json!({"thread_id": thread_id, "prompt": prompt}),
        )
        .await
    }

    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.fork_thread_with_title(parent_node_id, None).await
    }

    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(source) = source_thread_or_node {
            params.insert("parent_node_id".to_string(), json!(source));
        }
        if let Some(title) = title {
            params.insert("title".to_string(), json!(title));
        }
        self.call_typed("session.fork_thread", Value::Object(params))
            .await
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        self.call_typed("session.list_threads", json!({})).await
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.call_typed("session.get_transcript", json!({})).await
    }

    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.call_typed(
            "session.get_transcript",
            json!({"thread_id": thread_id}),
        )
        .await
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

    async fn blackboard_delete(&self, key: &str) -> Result<bool, EngineError> {
        let val = self.call("blackboard.delete", json!({"key": key})).await?;
        Ok(val
            .get("deleted")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        self.call_typed("permissions.resolve", to_json(&decision)?)
            .await
    }

    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        self.call("session.cancel", json!({"thread_id": thread_id}))
            .await
            .map(|_| ())
    }

    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        self.call_typed(
            "session.shell",
            json!({"thread_id": thread_id, "command": command}),
        )
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

    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        let (read, mut writer) = stream.into_split();
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "session.subscribe_events", "params": {}});
        writer
            .write_all(format!("{req}\n").as_bytes())
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;

        let mut reader = BufReader::new(read);
        let mut ack = String::new();
        let n = reader
            .read_line(&mut ack)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        let ack_val: Value =
            serde_json::from_str(ack.trim()).map_err(|e| EngineError::Protocol(e.to_string()))?;
        if ack_val.get("error").is_some() {
            return Ok(None);
        }

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let _keep_writer = writer;
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Ok(event) = serde_json::from_str::<TuiEngineEvent>(trimmed) {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        Ok(Some(rx))
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

    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => e.ensure_main_thread().await,
            Self::Socket(e) => e.ensure_main_thread().await,
        }
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        match self {
            Self::InProcess(e) => e.send_turn(prompt).await,
            Self::Socket(e) => e.send_turn(prompt).await,
        }
    }

    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        match self {
            Self::InProcess(e) => e.send_turn_for_thread(thread_id, prompt).await,
            Self::Socket(e) => e.send_turn_for_thread(thread_id, prompt).await,
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

    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => e.fork_thread_with_title(source_thread_or_node, title).await,
            Self::Socket(e) => e.fork_thread_with_title(source_thread_or_node, title).await,
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

    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        match self {
            Self::InProcess(e) => e.get_thread_transcript(thread_id).await,
            Self::Socket(e) => e.get_thread_transcript(thread_id).await,
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

    async fn blackboard_delete(&self, key: &str) -> Result<bool, EngineError> {
        match self {
            Self::InProcess(e) => e.blackboard_delete(key).await,
            Self::Socket(e) => e.blackboard_delete(key).await,
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

    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        match self {
            Self::InProcess(e) => e.cancel_turn(thread_id).await,
            Self::Socket(e) => e.cancel_turn(thread_id).await,
        }
    }

    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        match self {
            Self::InProcess(e) => e.run_shell_command(thread_id, command).await,
            Self::Socket(e) => e.run_shell_command(thread_id, command).await,
        }
    }

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        match self {
            Self::InProcess(e) => e.snapshot().await,
            Self::Socket(e) => e.snapshot().await,
        }
    }

    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::subscribe_events(e).await,
            Self::Socket(e) => e.subscribe_events().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::time::timeout;

    #[cfg(unix)]
    fn write_executable_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write script");
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    #[test]
    fn test_acp_backend_config_discover_default_env_overrides_and_fallback() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir bin");
        let agy_path = write_executable_script(&bin_dir, "agy", "#!/bin/sh\nexit 0\n");
        let claude_path = write_executable_script(&bin_dir, "claude", "#!/bin/sh\nexit 0\n");
        let search_both = bin_dir.to_string_lossy().into_owned();

        let from_override = AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            Some("/custom/agent".to_string()),
            Some("--acp --stdio".to_string()),
            Some(String::new()),
            Some(&search_both),
        )
        .expect("override config");
        assert_eq!(from_override.command, "/custom/agent");
        assert_eq!(from_override.args, vec!["--acp", "--stdio"]);
        assert_eq!(from_override.auth_method, None);
        assert_eq!(from_override.cwd, temp.path().to_path_buf());

        let from_override_auth = AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            Some("/custom/agent".to_string()),
            None,
            Some("api-key".to_string()),
            Some(&search_both),
        )
        .expect("override with auth");
        assert!(from_override_auth.args.is_empty());
        assert_eq!(from_override_auth.auth_method.as_deref(), Some("api-key"));

        let disabled = AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            Some(String::new()),
            None,
            None,
            Some(&search_both),
        );
        assert_eq!(disabled, None);

        let preferred_agy = AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            None,
            None,
            None,
            Some(&search_both),
        )
        .expect("prefer agy");
        assert_eq!(preferred_agy.command, agy_path.to_string_lossy());
        assert_eq!(preferred_agy.args, vec!["acp", "--stdio"]);
        assert_eq!(preferred_agy.auth_method.as_deref(), Some("oauth-personal"));

        let only_claude_dir = temp.path().join("only_claude");
        std::fs::create_dir_all(&only_claude_dir).expect("mkdir claude");
        let only_claude =
            write_executable_script(&only_claude_dir, "claude", "#!/bin/sh\nexit 0\n");
        let fallback_claude = AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            None,
            None,
            None,
            Some(&only_claude_dir.to_string_lossy()),
        )
        .expect("fallback claude");
        assert_eq!(fallback_claude.command, only_claude.to_string_lossy());
        assert_eq!(fallback_claude.args, vec!["--acp"]);
        assert_eq!(fallback_claude.auth_method, None);
        let _ = claude_path;

        let empty_dir = temp.path().join("empty_bin");
        std::fs::create_dir_all(&empty_dir).expect("mkdir empty");
        let none_found = AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            None,
            None,
            None,
            Some(&empty_dir.to_string_lossy()),
        );
        assert_eq!(none_found, None);
    }

    #[tokio::test]
    async fn test_sqlite_hydration_across_reopen_preserves_threads_blackboard_and_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open fresh db");
        assert!(engine.list_threads().await.expect("list").is_empty());

        let main = engine.ensure_main_thread().await.expect("ensure main");
        assert_eq!(main.title, "Main");
        assert!(!main.merged);
        assert_eq!(main.status, grein_core::model::ThreadStatus::Idle);
        assert!(engine
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("transcript")
            .is_empty());

        let mut rx = engine.subscribe_tui_events();
        engine
            .blackboard_upsert("port", "8080")
            .await
            .expect("upsert port");
        engine
            .blackboard_upsert("arch", "two-tier")
            .await
            .expect("upsert arch");
        assert_eq!(
            engine.snapshot().await.expect("snapshot").blackboard_entries,
            2
        );

        assert!(engine
            .blackboard_delete("port")
            .await
            .expect("delete port"));
        assert!(!engine
            .blackboard_delete("missing")
            .await
            .expect("delete missing"));

        let forked = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("feat-auth"))
            .await
            .expect("fork branch");
        assert_eq!(forked.title, "feat-auth");
        let merged = engine.merge_thread(&forked.thread_id).await.expect("merge");
        assert!(merged.merged);

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::BlackboardUpdated { key, value } if key == "port" && value == "8080"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::BlackboardDeleted { key } if key == "port"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCreated { thread } if thread.id == forked.thread_id && thread.title == "feat-auth"
        )));

        drop(engine);

        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen db");
        let threads = reopened.list_threads().await.expect("reopened threads");
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].thread_id, main.thread_id);
        assert_eq!(threads[0].title, "Main");
        assert!(!threads[0].merged);
        assert_eq!(threads[1].thread_id, forked.thread_id);
        assert_eq!(threads[1].title, "feat-auth");
        assert!(threads[1].merged);

        let bb = reopened.blackboard_list().await.expect("reopened bb");
        assert_eq!(bb.len(), 1);
        assert_eq!(bb[0].key, "arch");
        assert_eq!(bb[0].value, "two-tier");

        let snap = reopened.snapshot().await.expect("reopened snapshot");
        assert_eq!(snap.blackboard_entries, 1);
        assert_eq!(snap.thread_count, 2);
    }

    #[tokio::test]
    async fn test_send_turn_for_thread_without_acp_persists_user_and_errors_cleanly() {
        let engine = InProcessEngine::in_memory().expect("in_memory");
        let main = engine.ensure_main_thread().await.expect("ensure main");
        let mut rx = engine.subscribe_tui_events();

        let err = engine
            .send_turn_for_thread(&main.thread_id, "hello offline")
            .await
            .expect_err("no acp configured must error");
        assert!(!err.to_string().is_empty());

        let transcript = engine
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].role, "user");
        assert_eq!(transcript[0].content, "hello offline");

        let mut seen = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            seen.push(ev);
        }
        assert!(seen.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadStatusChanged { thread_id, status }
                if thread_id == &main.thread_id && status == "streaming"
        )));
        assert!(seen.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadStatusChanged { thread_id, status }
                if thread_id == &main.thread_id && status == "error"
        )));
        assert!(seen.iter().any(|e| matches!(e, TuiEngineEvent::Error { .. })));
    }

    #[tokio::test]
    async fn test_live_acp_streaming_whiteboard_delta_resume_permissions_and_cancel() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let capture_path = temp.path().join("acp_capture.jsonl");

        let mock_script = r#"
sess_counter=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    case "$line" in
        *'"method":"initialize"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock-agy","version":"1.0"},"authMethods":[{"id":"oauth-personal","name":"OAuth","description":"OAuth"}],"agentCapabilities":{"_meta":{"antigravity":{"systemPrompt":{"append":true}}}}}}\n' "$id"
            ;;
        *'"method":"authenticate"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            sess_counter=$((sess_counter+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s"}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/prompt"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            case "$line" in
                *'"sessionId":"sess-1"'*) sid="sess-1" ;;
                *'"sessionId":"sess-2"'*) sid="sess-2" ;;
                *) sid="sess-3" ;;
            esac
            if printf '%s' "$line" | grep -q "trigger-cancel"; then
                while IFS= read -r next_line; do
                    printf '%s\n' "$next_line" >> "$GREIN_CAPTURE"
                    if printf '%s' "$next_line" | grep -q '"method":"session/cancel"'; then
                        printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"cancelled"}}\n' "$id"
                        break
                    fi
                done
            elif printf '%s' "$line" | grep -q "trigger-perm"; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"tool_call","toolCallId":"tc-1","name":"run_command","title":"Run cargo","args":{"cmd":"cargo test"},"status":"pending"}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{"sessionId":"%s","toolCall":{"toolCallId":"tc-1","name":"run_command","title":"Run cargo","args":{"cmd":"cargo test"}},"options":[{"optionId":"allow_once","name":"Allow","kind":"allow_once"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]}}\n' "$sid"
                IFS= read -r perm_reply
                printf '%s\n' "$perm_reply" >> "$GREIN_CAPTURE"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-1","name":"run_command","status":"completed","rawOutput":{"output":"ok-tool"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello "}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"world!"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            else
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"session_info_update","title":"Auto Named Branch"}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Reply ok"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#;
        let wrapper_script = format!(
            "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
            capture_path.to_string_lossy(),
            mock_script
        );
        let script_path = write_executable_script(temp.path(), "mock_acp.sh", &wrapper_script);

        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: Some("oauth-personal".to_string()),
            cwd: temp.path().to_path_buf(),
        };

        let engine = Arc::new(
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg.clone()))
                .expect("open"),
        );
        let main = engine.ensure_main_thread().await.expect("main");
        engine
            .blackboard_upsert("db", "sqlite")
            .await
            .expect("wb upsert");

        let mut rx = engine.subscribe_tui_events();
        let engine_perm = Arc::clone(&engine);
        let perm_task = tokio::spawn(async move {
            let mut collected = Vec::new();
            while let Some(ev) = rx.recv().await {
                if let TuiEngineEvent::PermissionRequested { ref request_id, .. } = ev {
                    engine_perm
                        .resolve_permission(PermissionDecisionPayload {
                            request_id: request_id.clone(),
                            allow: true,
                        })
                        .await
                        .expect("resolve permission");
                }
                let is_done = matches!(
                    ev,
                    TuiEngineEvent::ThreadStatusChanged { ref status, .. } if status == "idle"
                );
                collected.push(ev);
                if is_done {
                    break;
                }
            }
            collected
        });

        let turn1 = timeout(
            Duration::from_secs(5),
            engine.send_turn_for_thread(&main.thread_id, "trigger-perm first prompt"),
        )
        .await
        .expect("turn1 timeout")
        .expect("turn1 ok");
        assert!(!turn1.node_id.is_empty());

        let evs1 = timeout(Duration::from_secs(3), perm_task)
            .await
            .expect("events timeout")
            .expect("join");
        assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::ThoughtChunk { content, .. } if content == "thinking")));
        assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::ToolInvoked { tool_name, .. } if tool_name == "run_command")));
        assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::ToolResult { output, .. } if output == "ok-tool")));
        assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::StreamChunk { content, .. } if content == "Hello ")));
        assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::StreamChunk { content, .. } if content == "world!")));
        assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::TurnCompleted { .. })));

        // Update whiteboard -> Turn 2 reuses sess-1 and prepends delta prefix.
        engine
            .blackboard_upsert("port", "9090")
            .await
            .expect("wb update");
        timeout(
            Duration::from_secs(5),
            engine.send_turn_for_thread(&main.thread_id, "second prompt"),
        )
        .await
        .expect("turn2 timeout")
        .expect("turn2 ok");

        // Turn 3 with unchanged whiteboard -> no delta prefix.
        timeout(
            Duration::from_secs(5),
            engine.send_turn_for_thread(&main.thread_id, "third prompt"),
        )
        .await
        .expect("turn3 timeout")
        .expect("turn3 ok");

        // Fork generic branch -> gets auto-renamed by session_info_update & sees ancestor trunk.
        let branch_auto = engine
            .fork_thread_with_title(Some(&main.thread_id), None)
            .await
            .expect("fork auto");
        assert!(branch_auto.title.starts_with("Branch"));
        timeout(
            Duration::from_secs(5),
            engine.send_turn_for_thread(&branch_auto.thread_id, "branch auto prompt"),
        )
        .await
        .expect("branch auto timeout")
        .expect("branch auto ok");
        let threads_after_auto = engine.list_threads().await.expect("list");
        let updated_auto = threads_after_auto
            .iter()
            .find(|t| t.thread_id == branch_auto.thread_id)
            .expect("auto branch exists");
        assert_eq!(updated_auto.title, "Auto Named Branch");

        // Fork explicit named branch -> title is NOT overwritten by session_info_update.
        let branch_named = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("explicit-title"))
            .await
            .expect("fork named");
        timeout(
            Duration::from_secs(5),
            engine.send_turn_for_thread(&branch_named.thread_id, "branch named prompt"),
        )
        .await
        .expect("branch named timeout")
        .expect("branch named ok");
        let threads_after_named = engine.list_threads().await.expect("list");
        let preserved = threads_after_named
            .iter()
            .find(|t| t.thread_id == branch_named.thread_id)
            .expect("named branch exists");
        assert_eq!(preserved.title, "explicit-title");

        // Cancel test on Main
        let engine_cancel = Arc::clone(&engine);
        let main_id = main.thread_id.clone();
        let cancel_handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            engine_cancel.cancel_turn(&main_id).await.expect("cancel ok");
        });
        timeout(
            Duration::from_secs(5),
            engine.send_turn_for_thread(&main.thread_id, "trigger-cancel now"),
        )
        .await
        .expect("cancel turn timeout")
        .expect("cancelled turn completes");
        cancel_handle.await.expect("cancel join");

        // Drop engine, reopen DB, verify resumed session passes Branch History in systemPrompt.append.
        drop(engine);
        let capture_resume = temp.path().join("acp_resume_capture.jsonl");
        let wrapper_resume = format!(
            "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
            capture_resume.to_string_lossy(),
            mock_script
        );
        let script_resume =
            write_executable_script(temp.path(), "mock_acp_resume.sh", &wrapper_resume);
        let cfg_resume = AcpBackendConfig {
            command: script_resume.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };
        let reopened = InProcessEngine::open_with_config(
            &db_path,
            temp.path().to_path_buf(),
            Some(cfg_resume),
        )
        .expect("reopen");
        timeout(
            Duration::from_secs(5),
            reopened.send_turn_for_thread(&main.thread_id, "resumed prompt"),
        )
        .await
        .expect("resume turn timeout")
        .expect("resume turn ok");

        // Verify captured ACP wire traffic
        let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
            .expect("read capture")
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid json"))
            .collect();

        assert_eq!(frames[0]["method"], "initialize");
        assert_eq!(frames[1]["method"], "authenticate");
        assert_eq!(frames[1]["params"]["methodId"], "oauth-personal");
        assert_eq!(frames[2]["method"], "session/new");
        let sys_append_1 = frames[2]["params"]["_meta"]["antigravity"]["systemPrompt"]["append"]
            .as_str()
            .expect("systemPrompt append");
        assert!(sys_append_1.contains("sqlite"));
        assert!(!sys_append_1.contains("trigger-perm first prompt"));

        let perm_reply = frames
            .iter()
            .find(|f| f.get("id") == Some(&json!("perm-1")))
            .expect("perm reply");
        assert_eq!(perm_reply["result"]["outcome"]["optionId"], "allow_once");

        let prompts: Vec<&str> = frames
            .iter()
            .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/prompt"))
            .map(|f| {
                f["params"]["prompt"][0]["text"]
                    .as_str()
                    .expect("prompt text")
            })
            .collect();
        assert!(!prompts[0].contains("[Shared Whiteboard Updated]"));
        assert!(prompts[1].contains("[Shared Whiteboard Updated]"));
        assert!(prompts[1].contains("9090"));
        assert!(prompts[1].ends_with("second prompt"));
        assert_eq!(prompts[2], "third prompt");

        assert!(frames
            .iter()
            .any(|f| f.get("method").and_then(Value::as_str) == Some("session/cancel")));

        let resume_frames: Vec<Value> = std::fs::read_to_string(&capture_resume)
            .expect("read resume capture")
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid json"))
            .collect();
        let resume_new = resume_frames
            .iter()
            .find(|f| f.get("method").and_then(Value::as_str) == Some("session/new"))
            .expect("resume session/new");
        let resume_append = resume_new["params"]["_meta"]["antigravity"]["systemPrompt"]["append"]
            .as_str()
            .expect("resume systemPrompt append");
        assert!(resume_append.contains("# Branch History"));
        assert!(resume_append.contains("Hello world!"));
    }

    #[tokio::test]
    async fn test_run_shell_command_and_json_rpc_new_methods() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open");
        let mut rx = engine.subscribe_tui_events();

        let ensured = dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"session.ensure_main_thread","params":{}}"#,
        )
        .await;
        let ensured_val: Value = serde_json::from_str(&ensured).expect("json");
        let main_id = ensured_val["result"]["thread_id"]
            .as_str()
            .expect("thread_id")
            .to_string();

        let shell_resp = dispatch_json_rpc(
            &engine,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session.shell",
                "params": {"thread_id": main_id, "command": "printf 'shell-ok'"}
            })
            .to_string(),
        )
        .await;
        let shell_val: Value = serde_json::from_str(&shell_resp).expect("shell json");
        assert_eq!(shell_val["result"]["stdout"], "shell-ok");
        assert_eq!(shell_val["result"]["exit_code"], 0);

        let ev = rx.recv().await.expect("shell event");
        assert!(matches!(
            ev,
            TuiEngineEvent::ShellOutput { stdout, exit_code, .. } if stdout == "shell-ok" && exit_code == 0
        ));

        let upsert_resp = dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":3,"method":"blackboard.upsert","params":{"key":"k","value":"v"}}"#,
        )
        .await;
        assert!(serde_json::from_str::<Value>(&upsert_resp).unwrap()["result"].is_object());

        let del_resp = dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":4,"method":"blackboard.delete","params":{"key":"k"}}"#,
        )
        .await;
        let del_val: Value = serde_json::from_str(&del_resp).expect("del json");
        assert_eq!(del_val["result"]["deleted"], true);
    }

    #[tokio::test]
    async fn test_acp_deny_permission_whiteboard_clear_and_crash_recovery() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let capture_path = temp.path().join("acp_deny_capture.jsonl");
        let flag_path = temp.path().join("crash_once.flag");
        std::fs::write(&flag_path, "1").expect("flag");

        let mock_script = r#"
if [ -f "$GREIN_CRASH_FLAG" ]; then
    rm -f "$GREIN_CRASH_FLAG"
    exit 1
fi
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    case "$line" in
        *'"method":"initialize"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-rec"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            if printf '%s' "$line" | grep -q "deny-me"; then
                printf '{"jsonrpc":"2.0","id":"perm-deny","method":"session/request_permission","params":{"sessionId":"sess-rec","toolCall":{"toolCallId":"tc-d","name":"write_file","title":"Write"},"options":[{"optionId":"allow_once","name":"Allow","kind":"allow_once"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]}}\n'
                IFS= read -r perm_reply
                printf '%s\n' "$perm_reply" >> "$GREIN_CAPTURE"
            fi
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-rec","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
        let wrapper = format!(
            "#!/bin/sh\nexport GREIN_CAPTURE={:?}\nexport GREIN_CRASH_FLAG={:?}\n{}\n",
            capture_path.to_string_lossy(),
            flag_path.to_string_lossy(),
            mock_script
        );
        let script_path = write_executable_script(temp.path(), "mock_recover.sh", &wrapper);
        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };

        let engine = Arc::new(
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
                .expect("open"),
        );
        let main = engine.ensure_main_thread().await.expect("main");
        engine
            .blackboard_upsert("temp", "val")
            .await
            .expect("upsert");

        // First call crashes on initialize -> error returned and driver reset cleanly.
        let crash_err = engine
            .send_turn_for_thread(&main.thread_id, "first fails")
            .await
            .expect_err("crash must fail");
        assert!(crash_err.to_string().contains("initialize"));

        // Second call recovers automatically, denies permission, and finishes cleanly.
        let mut rx = engine.subscribe_tui_events();
        let engine_deny = Arc::clone(&engine);
        let deny_task = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if let TuiEngineEvent::PermissionRequested { request_id, .. } = ev {
                    engine_deny
                        .resolve_permission(PermissionDecisionPayload {
                            request_id,
                            allow: false,
                        })
                        .await
                        .expect("deny");
                    break;
                }
            }
        });
        engine
            .send_turn_for_thread(&main.thread_id, "deny-me")
            .await
            .expect("recovers cleanly");
        deny_task.await.expect("deny task");

        // Clear whiteboard -> next turn sends [Shared Whiteboard Cleared].
        assert!(engine.blackboard_delete("temp").await.expect("delete"));
        engine
            .send_turn_for_thread(&main.thread_id, "after clear")
            .await
            .expect("after clear");

        let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
            .expect("capture")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        let deny_reply = frames
            .iter()
            .find(|f| f.get("id") == Some(&json!("perm-deny")))
            .expect("deny reply");
        assert_eq!(deny_reply["result"]["outcome"]["optionId"], "reject_once");
        let last_prompt = frames
            .iter()
            .rfind(|f| f.get("method").and_then(Value::as_str) == Some("session/prompt"))
            .expect("last prompt");
        let text = last_prompt["params"]["prompt"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("[Shared Whiteboard Cleared]"));
        assert!(text.ends_with("after clear"));
    }
}
