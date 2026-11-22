use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use grein_acp::{
    AcpConfigOption, AcpPermissionOption, AcpSessionInfo, AcpStdioDriver, DriverEvent,
    StdioProcessConfig, StdioProcessRunner, StdioRunnerError,
};
use grein_core::error::CoreError;
use grein_core::model::{
    Anchor, AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadId, ThreadKind,
    ThreadStatus,
};
use grein_core::storage::sqlite::SqliteStorage;
use grein_core::sync::{synthesize_thread_merge, ContextCompiler};
use grein_tui::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentOptionsSnapshot, AgentSlashCommand,
    HydratedThread, HydratedTurn, PtyCell as TuiPtyCell, PtyColor as TuiPtyColor,
    PtyScreenSnapshot as TuiPtyScreenSnapshot, SubagentSnapshot, SubagentStatus, ThreadMeta,
    TuiEngineEvent,
};
use grein_workspace::allocator::{ThreadId as WorkspaceThreadId, WorktreeAllocator};
use grein_workspace::git::{GitError, GitRepository};
use grein_workspace::git_status::{inspect_git_status, RepoGitStatus};
use grein_workspace::pty_session::{
    InteractivePtySession, PtyCell as WsPtyCell, PtyColor as WsPtyColor,
    PtyScreenSnapshot as WsPtyScreenSnapshot,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

use crate::discovery::{discover_acp_agents, AcpAgentKind};
use crate::subagent_watch::SubagentWatcher;

const INTERNAL_ROOT_CONTENT: &str = "Session initialized";
const ACP_SYSTEM_PROMPT_MAX_CHARS: usize = 32_000;
const SHELL_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const SHELL_MAX_OUTPUT_BYTES: usize = 65_536;
const UNSOLICITED_QUIET_TIMEOUT: Duration = Duration::from_millis(1500);
const CLOSE_SESSION_TIMEOUT: Duration = Duration::from_millis(250);
const PTY_COALESCE_WINDOW: Duration = Duration::from_millis(16);

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
pub struct SessionSnapshot {
    pub node_count: usize,
    pub thread_count: usize,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMetaEntry {
    pub thread_id: String,
    pub model: Option<String>,
    pub mode: Option<String>,
    #[serde(default)]
    pub config_options: Vec<AgentConfigOption>,
    #[serde(default)]
    pub available_commands: Vec<AgentSlashCommand>,
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
    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError>;
    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError>;
    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError>;
    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError>;
    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError>;
    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError>;
    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError>;
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
    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        Ok(Vec::new())
    }
    async fn delete_thread(&self, _thread_id: &str) -> Result<String, EngineError> {
        Err(EngineError::Protocol(
            "delete_thread not supported".to_string(),
        ))
    }
    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        Ok(Vec::new())
    }
    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        Ok(AgentOptionsSnapshot {
            thread_id: thread_id.to_string(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        })
    }
    async fn set_agent_config(
        &self,
        _thread_id: &str,
        _config_id: &str,
        _value: &str,
        _set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        Err(EngineError::Protocol(
            "set_agent_config not supported".to_string(),
        ))
    }
    async fn create_terminal(
        &self,
        _parent_thread_id: Option<&str>,
        _title: Option<&str>,
        _cwd: Option<&str>,
        _rows: u16,
        _cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        Err(EngineError::Protocol(
            "create_terminal not supported".to_string(),
        ))
    }
    async fn pty_write(&self, _thread_id: &str, _bytes: Vec<u8>) -> Result<(), EngineError> {
        Err(EngineError::Protocol(
            "pty_write not supported".to_string(),
        ))
    }
    async fn pty_resize(&self, _thread_id: &str, _rows: u16, _cols: u16) -> Result<(), EngineError> {
        Ok(())
    }
    async fn pty_snapshot(
        &self,
        _thread_id: &str,
        _rows: u16,
        _cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        Err(EngineError::Protocol(
            "pty_snapshot not supported".to_string(),
        ))
    }
    async fn ensure_thread_worktree(&self, _thread_id: &str) -> Result<String, EngineError> {
        Err(EngineError::Protocol(
            "ensure_thread_worktree not supported".to_string(),
        ))
    }
    async fn pty_scroll(
        &self,
        _thread_id: &str,
        _delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        Err(EngineError::Protocol(
            "pty_scroll not supported".to_string(),
        ))
    }
    async fn pty_restart(
        &self,
        _thread_id: &str,
        _rows: u16,
        _cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        Err(EngineError::Protocol(
            "pty_restart not supported".to_string(),
        ))
    }
}

#[allow(clippy::enum_variant_names)]
enum SessionListenerCmd {
    PrepareSolicited {
        reply_tx: oneshot::Sender<NodeId>,
    },
    StartSolicited {
        user_node_id: NodeId,
        done_tx: oneshot::Sender<Result<NodeId, EngineError>>,
        ack_tx: oneshot::Sender<()>,
    },
    AbortSolicited {
        ack_tx: oneshot::Sender<bool>,
    },
}

struct ActiveAcpSession {
    session_id: String,
    driver_gen: u64,
    model: Option<String>,
    mode: Option<String>,
    config_options: Vec<AgentConfigOption>,
    available_commands: Vec<AgentSlashCommand>,
    has_legacy_model: bool,
    has_legacy_mode: bool,
    pending_landmarks: Vec<String>,
    cmd_tx: mpsc::UnboundedSender<SessionListenerCmd>,
    listener_handle: tokio::task::JoinHandle<()>,
}

impl Drop for ActiveAcpSession {
    fn drop(&mut self) {
        self.listener_handle.abort();
    }
}

#[derive(Clone)]
struct ActiveSessionHandle {
    session_id: String,
    driver_gen: u64,
    cmd_tx: mpsc::UnboundedSender<SessionListenerCmd>,
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
    thread_turn_locks: HashMap<String, Arc<AsyncMutex<()>>>,
    default_config: HashMap<String, String>,
    pending_thread_config: HashMap<String, HashMap<String, String>>,
}

struct EngineRuntime {
    storage: SqliteStorage,
    cwd: PathBuf,
    acp_config: Option<AcpBackendConfig>,
    acp_driver: AsyncMutex<Option<DriverSlot>>,
    driver_gen_seq: AtomicU64,
    state: Mutex<EngineState>,
    tui_subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<TuiEngineEvent>>>>,
    next_perm_seq: AtomicU64,
    subagent_watch: Mutex<Arc<SubagentWatcher>>,
    unsolicited_quiet_timeout: Mutex<Duration>,
    active_pty_sessions: Arc<Mutex<HashMap<String, Arc<InteractivePtySession>>>>,
    pending_pty_sizes: Arc<Mutex<HashMap<String, (u16, u16)>>>,
    pty_exited_threads: Mutex<HashSet<String>>,
    pty_notify_tx: mpsc::UnboundedSender<String>,
    pty_notify_rx: Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    pty_notifier_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Engine backed directly by `SqliteStorage` and live ACP v1 stdio drivers.
pub struct InProcessEngine {
    runtime: Arc<EngineRuntime>,
}

impl Drop for InProcessEngine {
    fn drop(&mut self) {
        if let Ok(mut state) = self.runtime.state.lock() {
            state.active_acp_sessions.clear();
        }
        if let Ok(mut handle) = self.runtime.pty_notifier_handle.lock() {
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
        let sessions: Vec<Arc<InteractivePtySession>> = self
            .runtime
            .active_pty_sessions
            .lock()
            .map(|mut map| map.drain().map(|(_, s)| s).collect())
            .unwrap_or_default();
        drop(sessions);
        if let Ok(watch) = self.runtime.subagent_watch.lock() {
            watch.shutdown();
        }
    }
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

pub fn slugify_anchor(title: &str) -> String {
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

pub fn truncate_chars_with_ellipsis(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
    }
    let mut out: String = text.chars().take(max_chars - 3).collect();
    out.push_str("...");
    out
}

fn is_internal_root_node(node: &ConversationNode) -> bool {
    node.parent_id.is_none()
        && node.role == AuthorRole::System
        && (node.content.is_empty() || node.content == INTERNAL_ROOT_CONTENT)
}

fn thread_to_summary(thread: &Thread, is_first: bool) -> ThreadSummary {
    let title = if !thread.title.trim().is_empty() {
        thread.title.clone()
    } else if thread.kind == ThreadKind::Terminal {
        "Terminal".to_string()
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

fn pty_color_to_tui(color: WsPtyColor) -> TuiPtyColor {
    match color {
        WsPtyColor::Default => TuiPtyColor::Default,
        WsPtyColor::Indexed(idx) => TuiPtyColor::Indexed(idx),
        WsPtyColor::Rgb(r, g, b) => TuiPtyColor::Rgb(r, g, b),
    }
}

fn pty_cell_to_tui(cell: WsPtyCell) -> TuiPtyCell {
    TuiPtyCell {
        contents: cell.contents,
        fg: pty_color_to_tui(cell.fg),
        bg: pty_color_to_tui(cell.bg),
        bold: cell.bold,
        italic: cell.italic,
        underline: cell.underline,
        inverse: cell.inverse,
        is_wide_continuation: cell.is_wide_continuation,
    }
}

fn pty_snapshot_to_tui(snap: WsPtyScreenSnapshot) -> TuiPtyScreenSnapshot {
    TuiPtyScreenSnapshot {
        rows: snap.rows,
        cols: snap.cols,
        cells: snap
            .cells
            .into_iter()
            .map(|row| row.into_iter().map(pty_cell_to_tui).collect())
            .collect(),
        cursor_row: snap.cursor_row,
        cursor_col: snap.cursor_col,
        hide_cursor: snap.hide_cursor,
        application_cursor: snap.application_cursor,
        bracketed_paste: snap.bracketed_paste,
        alternate_screen: snap.alternate_screen,
        exited: snap.exited,
        exit_code: snap.exit_code,
        scrollback_offset: snap.scrollback_offset,
        cwd: snap.cwd,
    }
}

fn node_to_transcript_entry(node: &ConversationNode) -> TranscriptEntry {
    TranscriptEntry {
        node_id: node.id.0.to_string(),
        role: node.role.to_string(),
        content: node.content.clone(),
    }
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

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn path_from_node_map(
    leaf_id: NodeId,
    nodes_by_id: &HashMap<NodeId, ConversationNode>,
) -> Vec<&ConversationNode> {
    let mut path = Vec::new();
    let mut cur = Some(leaf_id);
    let mut visited = HashSet::new();
    while let Some(nid) = cur {
        if !visited.insert(nid) {
            break;
        }
        let Some(node) = nodes_by_id.get(&nid) else {
            break;
        };
        path.push(node);
        cur = node.parent_id;
    }
    path.reverse();
    path
}

fn extract_subagent_role(args: &Value, title: &str) -> String {
    args.get("Role")
        .or_else(|| args.get("role"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| (!title.trim().is_empty()).then_some(title.trim()))
        .unwrap_or("Subagent")
        .to_string()
}

fn extract_subagent_type(args: &Value) -> String {
    args.get("TypeName")
        .or_else(|| args.get("typeName"))
        .or_else(|| args.get("type_name"))
        .or_else(|| args.get("agent_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("agent")
        .to_string()
}

fn normalize_single_line(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_tool_call_summary(tool_name: &str, title: &str, args: &Value) -> Option<String> {
    const PRIMARY_ARG_KEYS: &[&str] = &[
        "CommandLine",
        "command",
        "cmd",
        "Path",
        "path",
        "TargetFile",
        "SearchPath",
        "Query",
        "query",
    ];

    let normalized_title = normalize_single_line(title);
    if !normalized_title.is_empty() && normalized_title != tool_name.trim() {
        return Some(truncate_chars_with_ellipsis(&normalized_title, 80));
    }

    for key in PRIMARY_ARG_KEYS {
        if let Some(raw_val) = args.get(*key).and_then(Value::as_str) {
            let normalized_val = normalize_single_line(raw_val);
            if !normalized_val.is_empty() {
                return Some(truncate_chars_with_ellipsis(&normalized_val, 80));
            }
        }
    }

    None
}

fn acp_config_option_to_tui(opt: &AcpConfigOption) -> AgentConfigOption {
    AgentConfigOption {
        id: opt.id.clone(),
        name: opt.name.clone(),
        category: opt.category.clone(),
        current_value: opt.current_value.clone(),
        options: opt
            .options
            .iter()
            .map(|c| AgentConfigChoice {
                value: c.value.clone(),
                name: c.name.clone(),
                description: c.description.clone(),
            })
            .collect(),
    }
}

fn normalize_session_config_options(info: &AcpSessionInfo) -> Vec<AgentConfigOption> {
    let mut out: Vec<AgentConfigOption> = info
        .config_options
        .iter()
        .map(acp_config_option_to_tui)
        .collect();

    let has_model = out
        .iter()
        .any(|o| o.id == "model" || o.category.as_deref() == Some("model"));
    if !has_model && !info.available_models.is_empty() {
        out.push(AgentConfigOption {
            id: "model".to_string(),
            name: "Model".to_string(),
            category: Some("model".to_string()),
            current_value: info
                .current_model_id
                .clone()
                .unwrap_or_else(|| info.available_models[0].model_id.clone()),
            options: info
                .available_models
                .iter()
                .map(|m| AgentConfigChoice {
                    value: m.model_id.clone(),
                    name: m.name.clone(),
                    description: m.description.clone(),
                })
                .collect(),
        });
    }

    let has_mode = out
        .iter()
        .any(|o| o.id == "mode" || o.category.as_deref() == Some("mode"));
    if !has_mode && !info.available_modes.is_empty() {
        out.push(AgentConfigOption {
            id: "mode".to_string(),
            name: "Mode".to_string(),
            category: Some("mode".to_string()),
            current_value: info
                .current_mode_id
                .clone()
                .unwrap_or_else(|| info.available_modes[0].id.clone()),
            options: info
                .available_modes
                .iter()
                .map(|m| AgentConfigChoice {
                    value: m.id.clone(),
                    name: m.name.clone(),
                    description: m.description.clone(),
                })
                .collect(),
        });
    }

    out
}

fn extract_model_and_mode_from_options(
    options: &[AgentConfigOption],
    fallback_model: Option<String>,
    fallback_mode: Option<String>,
) -> (Option<String>, Option<String>) {
    let opt_model = options
        .iter()
        .find(|o| o.category.as_deref() == Some("model") || o.id == "model")
        .map(|o| o.current_value.clone())
        .filter(|v| !v.is_empty());
    let opt_mode = options
        .iter()
        .find(|o| o.category.as_deref() == Some("mode") || o.id == "mode")
        .map(|o| o.current_value.clone())
        .filter(|v| !v.is_empty());
    (opt_model.or(fallback_model), opt_mode.or(fallback_mode))
}

fn merge_updated_config_options(
    existing: &[AgentConfigOption],
    mut incoming: Vec<AgentConfigOption>,
    has_legacy_model: bool,
    has_legacy_mode: bool,
) -> (Vec<AgentConfigOption>, bool, bool) {
    let incoming_has_model = incoming
        .iter()
        .any(|o| o.id == "model" || o.category.as_deref() == Some("model"));
    let incoming_has_mode = incoming
        .iter()
        .any(|o| o.id == "mode" || o.category.as_deref() == Some("mode"));

    if !incoming_has_model && has_legacy_model {
        if let Some(model_opt) = existing
            .iter()
            .find(|o| o.id == "model" || o.category.as_deref() == Some("model"))
        {
            incoming.push(model_opt.clone());
        }
    }
    if !incoming_has_mode && has_legacy_mode {
        if let Some(mode_opt) = existing
            .iter()
            .find(|o| o.id == "mode" || o.category.as_deref() == Some("mode"))
        {
            incoming.push(mode_opt.clone());
        }
    }

    (
        incoming,
        has_legacy_model && !incoming_has_model,
        has_legacy_mode && !incoming_has_mode,
    )
}

fn is_unsupported_method_error(err: &StdioRunnerError) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("-32601")
        || msg.contains("method not found")
        || msg.contains("not supported")
        || msg.contains("unsupported")
        || msg.contains("unimplemented")
        || msg.contains("unknown method")
}

fn stdio_err_to_engine(err: StdioRunnerError) -> EngineError {
    match err {
        StdioRunnerError::ProcessTerminated(msg) => EngineError::Transport(msg),
        other => {
            let msg = other.to_string();
            if msg.contains("-32602") {
                EngineError::InvalidParams(msg)
            } else {
                EngineError::Transport(msg)
            }
        }
    }
}

struct ConfigTarget<'a> {
    config_id: &'a str,
    value: &'a str,
    is_model_hint: bool,
    is_mode_hint: bool,
}

#[derive(Clone)]
struct TrackedLayer1Subagent {
    snapshot: SubagentSnapshot,
    launched_turn_seq: u64,
    launched_unsolicited: bool,
    launched_wakeup_seq: u64,
}

impl EngineRuntime {
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

    async fn acquire_thread_turn_lock(
        &self,
        thread_id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, EngineError> {
        let mutex = {
            let mut state = self.lock()?;
            Arc::clone(
                state
                    .thread_turn_locks
                    .entry(thread_id.to_string())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        Ok(mutex.lock_owned().await)
    }

    async fn current_driver_if_running(&self) -> Option<(Arc<AcpStdioDriver>, u64)> {
        let mut guard = self.acp_driver.lock().await;
        if let Some(slot) = guard.as_ref() {
            if slot.driver.is_running() {
                return Some((Arc::clone(&slot.driver), slot.generation));
            }
            let dead_gen = slot.generation;
            *guard = None;
            if let Ok(mut state) = self.state.lock() {
                state
                    .active_acp_sessions
                    .retain(|_, s| s.driver_gen != dead_gen);
            }
        }
        None
    }

    async fn apply_config_to_driver(
        driver: &AcpStdioDriver,
        session_id: &str,
        target: &ConfigTarget<'_>,
    ) -> Result<Option<Vec<AgentConfigOption>>, EngineError> {
        match driver
            .set_config_option(session_id, target.config_id, target.value)
            .await
        {
            Ok(opts) => {
                if opts.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(opts.iter().map(acp_config_option_to_tui).collect()))
                }
            }
            Err(err) if is_unsupported_method_error(&err) => {
                if target.config_id == "model" || target.is_model_hint {
                    driver
                        .set_session_model(session_id, target.value)
                        .await
                        .map_err(stdio_err_to_engine)?;
                    Ok(None)
                } else if target.config_id == "mode" || target.is_mode_hint {
                    driver
                        .set_session_mode(session_id, target.value)
                        .await
                        .map_err(stdio_err_to_engine)?;
                    Ok(None)
                } else {
                    Err(EngineError::Protocol(format!(
                        "ACP config option `{}` not supported: {err}",
                        target.config_id
                    )))
                }
            }
            Err(err) => Err(stdio_err_to_engine(err)),
        }
    }

    fn apply_config_update_to_state(
        &self,
        thread_id: &str,
        session_id: &str,
        target: &ConfigTarget<'_>,
        updated_opts: Option<Vec<AgentConfigOption>>,
    ) -> Result<(AgentOptionsSnapshot, Option<String>, Option<String>), EngineError> {
        let mut state = self.lock()?;
        let entry = state
            .active_acp_sessions
            .get_mut(thread_id)
            .filter(|e| e.session_id == session_id)
            .ok_or_else(|| EngineError::NotFound(format!("active ACP session for {thread_id}")))?;

        match updated_opts {
            Some(incoming) => {
                let (mut merged, next_legacy_model, next_legacy_mode) = merge_updated_config_options(
                    &entry.config_options,
                    incoming,
                    entry.has_legacy_model,
                    entry.has_legacy_mode,
                );
                entry.has_legacy_model = next_legacy_model;
                entry.has_legacy_mode = next_legacy_mode;
                for opt in &mut merged {
                    if opt.id == target.config_id && opt.current_value != target.value {
                        opt.current_value = target.value.to_string();
                    }
                }
                entry.config_options = merged;
            }
            None => {
                for opt in &mut entry.config_options {
                    if opt.id == target.config_id {
                        opt.current_value = target.value.to_string();
                    }
                }
            }
        }

        if target.config_id == "model" || target.is_model_hint {
            entry.model = Some(target.value.to_string());
        }
        if target.config_id == "mode" || target.is_mode_hint {
            entry.mode = Some(target.value.to_string());
        }
        let (model, mode) = extract_model_and_mode_from_options(
            &entry.config_options,
            entry.model.clone(),
            entry.mode.clone(),
        );
        entry.model = model.clone();
        entry.mode = mode.clone();

        let snapshot = AgentOptionsSnapshot {
            thread_id: thread_id.to_string(),
            config_options: entry.config_options.clone(),
            available_commands: entry.available_commands.clone(),
        };
        Ok((snapshot, model, mode))
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_acp_session_for_thread(
        self: &Arc<Self>,
        cfg: &AcpBackendConfig,
        tid: ThreadId,
        thread_id: &str,
        initial_leaf_id: NodeId,
        exclude_node_id: Option<NodeId>,
        emit_initial_meta: bool,
    ) -> Result<(Arc<AcpStdioDriver>, ActiveSessionHandle), EngineError> {
        let (driver, driver_gen) = self.ensure_driver(cfg).await?;

        {
            let mut state = self.lock()?;
            let stale = state
                .active_acp_sessions
                .get(thread_id)
                .is_some_and(|s| s.driver_gen != driver_gen || s.cmd_tx.is_closed());
            if stale {
                state.active_acp_sessions.remove(thread_id);
            }
            if let Some(existing) = state.active_acp_sessions.get(thread_id) {
                return Ok((
                    driver,
                    ActiveSessionHandle {
                        session_id: existing.session_id.clone(),
                        driver_gen: existing.driver_gen,
                        cmd_tx: existing.cmd_tx.clone(),
                    },
                ));
            }
        }

        let mut payload = ContextCompiler::new("")
            .compile_thread_context(&self.storage, tid)
            .map_err(storage_err)?;
        payload.trunk_path.retain(|n| !is_internal_root_node(n));
        if let Some(ex_id) = exclude_node_id {
            payload.branch_turns.retain(|n| n.id != ex_id);
        }
        let include_branch_turns = !payload.branch_turns.is_empty();
        let append =
            payload.build_acp_system_prompt_append(include_branch_turns, ACP_SYSTEM_PROMPT_MAX_CHARS);
        let append_opt = (!append.trim().is_empty()).then_some(append.as_str());

        let session_cwd = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .and_then(|t| t.cwd)
            .filter(|c| !c.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| cfg.cwd.clone());

        let info = match driver.new_session(&session_cwd, append_opt).await {
            Ok(info) => info,
            Err(err) => {
                self.invalidate_driver_if_dead(&driver, driver_gen).await;
                return Err(EngineError::Transport(format!("ACP new_session failed: {err}")));
            }
        };

        self.storage
            .update_thread_acp_session_id(&tid, Some(&info.session_id))
            .map_err(storage_err)?;

        let config_options = normalize_session_config_options(&info);
        let has_legacy_model = !info
            .config_options
            .iter()
            .any(|o| o.id == "model" || o.category.as_deref() == Some("model"))
            && !info.available_models.is_empty();
        let has_legacy_mode = !info
            .config_options
            .iter()
            .any(|o| o.id == "mode" || o.category.as_deref() == Some("mode"))
            && !info.available_modes.is_empty();
        let (model, mode) = extract_model_and_mode_from_options(
            &config_options,
            info.current_model_id.clone(),
            info.current_mode_id.clone(),
        );

        let events_rx = driver.subscribe_session(&info.session_id);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let listener_handle = spawn_session_listener(
            Arc::clone(self),
            tid,
            thread_id.to_string(),
            info.session_id.clone(),
            Arc::clone(&driver),
            initial_leaf_id,
            events_rx,
            cmd_rx,
        );

        let handle = ActiveSessionHandle {
            session_id: info.session_id.clone(),
            driver_gen,
            cmd_tx: cmd_tx.clone(),
        };

        let mut desired_config: Vec<(String, String)> = {
            let mut state = self.lock()?;
            let mut merged = state.default_config.clone();
            if let Some(pending) = state.pending_thread_config.get(thread_id) {
                for (k, v) in pending {
                    merged.insert(k.clone(), v.clone());
                }
            }
            state.active_acp_sessions.insert(
                thread_id.to_string(),
                ActiveAcpSession {
                    session_id: info.session_id.clone(),
                    driver_gen,
                    model: model.clone(),
                    mode: mode.clone(),
                    config_options: config_options.clone(),
                    available_commands: Vec::new(),
                    has_legacy_model,
                    has_legacy_mode,
                    pending_landmarks: Vec::new(),
                    cmd_tx,
                    listener_handle,
                },
            );
            merged.into_iter().collect()
        };

        desired_config.sort_by(|(a, _), (b, _)| {
            let rank = |k: &str| match k {
                "model" => 0,
                "mode" => 1,
                _ => 2,
            };
            rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
        });

        for (cfg_id, cfg_val) in desired_config {
            let (already_matches, is_model_hint, is_mode_hint) = {
                let state = self.lock()?;
                if let Some(entry) = state.active_acp_sessions.get(thread_id) {
                    let matching_opt = entry.config_options.iter().find(|o| o.id == cfg_id);
                    let current = matching_opt.map(|o| o.current_value.as_str()).or_else(|| {
                        if cfg_id == "model" {
                            entry.model.as_deref()
                        } else if cfg_id == "mode" {
                            entry.mode.as_deref()
                        } else {
                            None
                        }
                    });
                    let is_model = matching_opt
                        .is_some_and(|o| o.category.as_deref() == Some("model"));
                    let is_mode = matching_opt
                        .is_some_and(|o| o.category.as_deref() == Some("mode"));
                    (current == Some(cfg_val.as_str()), is_model, is_mode)
                } else {
                    (false, false, false)
                }
            };
            if already_matches {
                continue;
            }
            let target = ConfigTarget {
                config_id: &cfg_id,
                value: &cfg_val,
                is_model_hint,
                is_mode_hint,
            };
            if let Ok(updated_opts) =
                Self::apply_config_to_driver(&driver, &info.session_id, &target).await
            {
                let _ = self.apply_config_update_to_state(
                    thread_id,
                    &info.session_id,
                    &target,
                    updated_opts,
                );
            }
        }

        if emit_initial_meta {
            let (final_model, final_mode, final_opts) = {
                let state = self.lock()?;
                state
                    .active_acp_sessions
                    .get(thread_id)
                    .map(|e| (e.model.clone(), e.mode.clone(), e.config_options.clone()))
                    .unwrap_or((model, mode, config_options))
            };
            if final_model.is_some() || final_mode.is_some() {
                self.emit_tui(TuiEngineEvent::AgentMetaUpdated {
                    thread_id: thread_id.to_string(),
                    model: final_model,
                    mode: final_mode,
                });
            }
            if !final_opts.is_empty() {
                self.emit_tui(TuiEngineEvent::AgentConfigUpdated {
                    thread_id: thread_id.to_string(),
                    config_options: final_opts,
                });
            }
        }

        Ok((driver, handle))
    }

    fn subagent_watch(&self) -> Arc<SubagentWatcher> {
        Arc::clone(
            &*self
                .subagent_watch
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        )
    }

    fn quiet_timeout(&self) -> Duration {
        *self
            .unsolicited_quiet_timeout
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    fn load_thread_turns(&self, thread: &Thread) -> Result<Vec<TranscriptEntry>, EngineError> {
        let path = self
            .storage
            .get_trunk_path(thread.active_leaf_id)
            .map_err(storage_err)?;
        let threads = self.storage.list_threads().map_err(storage_err)?;
        let is_first_thread = threads.first().is_some_and(|first| first.id == thread.id);

        let skip_count = match path.iter().position(|n| n.id == thread.parent_node_id) {
            Some(0) if is_first_thread && !is_internal_root_node(&path[0]) => 0,
            Some(idx) => idx + 1,
            None => 0,
        };

        Ok(path
            .into_iter()
            .skip(skip_count)
            .filter(|n| !is_internal_root_node(n))
            .map(|n| node_to_transcript_entry(&n))
            .collect())
    }

    fn compute_thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        self.compute_thread_metadata_with_inspector(inspect_git_status)
    }

    fn compute_thread_metadata_with_inspector<F>(
        &self,
        mut inspector: F,
    ) -> Result<Vec<ThreadMeta>, EngineError>
    where
        F: FnMut(&Path) -> Option<RepoGitStatus>,
    {
        let threads = self.storage.list_threads().map_err(storage_err)?;
        if threads.is_empty() {
            return Ok(Vec::new());
        }
        let known_ids: HashSet<ThreadId> = threads.iter().map(|t| t.id).collect();
        let nodes = self.storage.list_nodes().map_err(storage_err)?;
        let nodes_by_id: HashMap<NodeId, ConversationNode> =
            nodes.into_iter().map(|n| (n.id, n)).collect();

        let mut node_owner: HashMap<NodeId, ThreadId> = HashMap::new();

        for (idx, thread) in threads.iter().enumerate() {
            let path = path_from_node_map(thread.active_leaf_id, &nodes_by_id);
            let parent_pos = path.iter().position(|n| n.id == thread.parent_node_id);

            if idx == 0 || path.first().is_some_and(|n| n.id == thread.parent_node_id) {
                for node in &path {
                    node_owner.entry(node.id).or_insert(thread.id);
                }
            } else if let Some(p_idx) = parent_pos {
                for node in path.iter().skip(p_idx + 1) {
                    node_owner.entry(node.id).or_insert(thread.id);
                }
            } else {
                for node in &path {
                    node_owner.entry(node.id).or_insert(thread.id);
                }
            }
        }

        let mut git_cache: HashMap<PathBuf, Option<RepoGitStatus>> = HashMap::new();
        let mut meta = Vec::with_capacity(threads.len());
        for (idx, thread) in threads.iter().enumerate() {
            let trunk = path_from_node_map(thread.parent_node_id, &nodes_by_id);
            let is_offline_root_main = idx == 0
                && trunk.len() == 1
                && trunk[0].id == thread.parent_node_id
                && !is_internal_root_node(trunk[0]);

            let parent_thread_id = if let Some(explicit_pid) = thread
                .parent_thread_id
                .filter(|pid| *pid != thread.id && known_ids.contains(pid))
            {
                Some(explicit_pid.0.to_string())
            } else if idx == 0
                && trunk.len() <= 1
                && trunk.first().is_some_and(|n| n.id == thread.parent_node_id)
            {
                None
            } else {
                let mut found = None;
                let mut cur = Some(thread.parent_node_id);
                let mut visited = HashSet::new();
                while let Some(nid) = cur {
                    if !visited.insert(nid) {
                        break;
                    }
                    if let Some(owner) = node_owner.get(&nid) {
                        if *owner != thread.id {
                            found = Some(owner.0.to_string());
                            break;
                        }
                    }
                    cur = nodes_by_id.get(&nid).and_then(|n| n.parent_id);
                }
                found
            };

            let inherited_turn_count = if is_offline_root_main {
                0
            } else {
                trunk
                    .iter()
                    .filter(|n| !is_internal_root_node(n))
                    .count()
            };

            let target_path = thread
                .cwd
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| self.cwd.clone());
            let git_status = git_cache
                .entry(target_path)
                .or_insert_with_key(|path| inspector(path))
                .as_ref();
            let git_branch = git_status.and_then(|s| s.branch.clone());
            let git_dirty = git_status.map_or(0, |s| s.dirty_count);

            meta.push(ThreadMeta {
                thread_id: thread.id.0.to_string(),
                parent_thread_id,
                inherited_turn_count,
                merged: thread.merged,
                kind: thread.kind,
                cwd: thread.cwd.clone(),
                git_branch,
                git_dirty,
            });
        }

        Ok(meta)
    }

    fn mark_thread_error(&self, tid: ThreadId, thread_id: &str, message: &str) {
        if self.storage.get_thread(tid).ok().flatten().is_none() {
            return;
        }
        let _ = self.storage.update_thread_status(tid, ThreadStatus::Error);
        self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "error".to_string(),
        });
        self.emit_tui(TuiEngineEvent::Error {
            message: message.to_string(),
        });
    }

    fn persist_assistant_node(
        &self,
        tid: ThreadId,
        parent_leaf: NodeId,
        content: String,
    ) -> Result<NodeId, EngineError> {
        let asst_node_id = NodeId::new();
        let asst_node = ConversationNode::new(
            asst_node_id,
            Some(parent_leaf),
            AuthorRole::Assistant,
            content,
        );
        self.storage.insert_node(&asst_node).map_err(storage_err)?;
        self.storage
            .update_thread_leaf(tid, asst_node_id)
            .map_err(storage_err)?;
        if let Ok(mut state) = self.state.lock() {
            state.last_node = Some(asst_node_id);
        }
        Ok(asst_node_id)
    }

    async fn ensure_driver(
        &self,
        cfg: &AcpBackendConfig,
    ) -> Result<(Arc<AcpStdioDriver>, u64), EngineError> {
        let mut guard = self.acp_driver.lock().await;
        if let Some(slot) = guard.as_ref() {
            if slot.driver.is_running() {
                return Ok((Arc::clone(&slot.driver), slot.generation));
            }
            *guard = None;
        }

        let spawn_cfg =
            StdioProcessConfig::new(&cfg.command, cfg.args.clone()).with_current_dir(&cfg.cwd);
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

    fn ensure_pty_notifier_started(self: &Arc<Self>) {
        let maybe_rx = self
            .pty_notify_rx
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        let Some(mut rx) = maybe_rx else {
            return;
        };
        let weak_rt: Weak<EngineRuntime> = Arc::downgrade(self);
        let handle = tokio::spawn(async move {
            while let Some(first_tid) = rx.recv().await {
                let mut dirty = HashSet::new();
                dirty.insert(first_tid);
                let deadline = tokio::time::Instant::now() + PTY_COALESCE_WINDOW;
                loop {
                    let remaining =
                        deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, rx.recv()).await {
                        Ok(Some(tid)) => {
                            dirty.insert(tid);
                        }
                        Ok(None) => return,
                        Err(_) => break,
                    }
                }
                let Some(rt) = weak_rt.upgrade() else {
                    break;
                };
                rt.flush_dirty_pty_threads(dirty);
            }
        });
        if let Ok(mut guard) = self.pty_notifier_handle.lock() {
            *guard = Some(handle);
        }
    }

    fn flush_dirty_pty_threads(&self, dirty: HashSet<String>) {
        let mut cwd_updated = false;
        for thread_id in dirty {
            let session = self
                .active_pty_sessions
                .lock()
                .ok()
                .and_then(|map| map.get(&thread_id).cloned());
            let Some(session) = session else {
                continue;
            };
            let raw_snap = session.snapshot();
            let raw_cwd = raw_snap.cwd.clone();
            let snap = pty_snapshot_to_tui(raw_snap);
            let exited = snap.exited;
            let exit_code = snap.exit_code;
            self.emit_tui(TuiEngineEvent::PtyScreenUpdated {
                thread_id: thread_id.clone(),
                snapshot: snap,
            });
            if let Some(raw_dir) = raw_cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(tid) = parse_typed_id::<ThreadId>(&thread_id) {
                    let raw_path = Path::new(raw_dir);
                    if raw_path.is_dir() {
                        if let Ok(canonical_path) = std::fs::canonicalize(raw_path) {
                            if let Some(canonical_dir) = canonical_path.to_str() {
                                if let Ok(Some(thread)) = self.storage.get_thread(tid) {
                                    if thread.cwd.as_deref() != Some(canonical_dir)
                                        && self
                                            .storage
                                            .update_thread_cwd(&tid, Some(canonical_dir))
                                            .is_ok()
                                    {
                                        cwd_updated = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if exited {
                let newly_exited = self
                    .pty_exited_threads
                    .lock()
                    .map(|mut seen| seen.insert(thread_id.clone()))
                    .unwrap_or(false);
                if newly_exited {
                    if let Some(tid) = parse_typed_id::<ThreadId>(&thread_id) {
                        if self.storage.get_thread(tid).ok().flatten().is_some() {
                            let _ = self.storage.update_thread_status(tid, ThreadStatus::Idle);
                        }
                    }
                    self.emit_tui(TuiEngineEvent::PtyExited {
                        thread_id: thread_id.to_string(),
                        exit_code,
                    });
                    self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                        thread_id,
                        status: "idle".to_string(),
                    });
                }
            }
        }
        if cwd_updated {
            if let Ok(meta) = self.compute_thread_metadata() {
                self.emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
            }
        }
    }

    fn resolve_thread_cwd(&self, thread: &Thread) -> PathBuf {
        thread
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| self.cwd.clone())
    }

    fn expand_leading_tilde(raw: &str, home_dir: Option<&str>) -> Option<PathBuf> {
        let home = home_dir.filter(|h| !h.is_empty())?;
        if raw == "~" {
            return Some(PathBuf::from(home));
        }
        raw.strip_prefix("~/").map(|rest| {
            let trimmed_rest = rest.trim_start_matches('/');
            if trimmed_rest.is_empty() {
                PathBuf::from(home)
            } else {
                PathBuf::from(home).join(trimmed_rest)
            }
        })
    }

    fn resolve_terminal_cwd(
        &self,
        explicit_cwd: Option<&str>,
        parent_thread: Option<&Thread>,
    ) -> Result<String, EngineError> {
        let home_dir = std::env::var("HOME").ok();
        self.resolve_terminal_cwd_with_home(explicit_cwd, parent_thread, home_dir.as_deref())
    }

    fn resolve_terminal_cwd_with_home(
        &self,
        explicit_cwd: Option<&str>,
        parent_thread: Option<&Thread>,
        home_dir: Option<&str>,
    ) -> Result<String, EngineError> {
        let base_dir = parent_thread
            .map(|p| self.resolve_thread_cwd(p))
            .unwrap_or_else(|| self.cwd.clone());
        let candidate = match explicit_cwd.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                if let Some(expanded) = Self::expand_leading_tilde(raw, home_dir) {
                    expanded
                } else {
                    let raw_path = Path::new(raw);
                    if raw_path.is_absolute() {
                        raw_path.to_path_buf()
                    } else {
                        base_dir.join(raw_path)
                    }
                }
            }
            None => base_dir,
        };
        if !candidate.is_dir() {
            return Err(EngineError::InvalidParams(format!(
                "terminal cwd does not exist or is not a directory: {}",
                candidate.display()
            )));
        }
        let canonical = std::fs::canonicalize(&candidate).map_err(|e| {
            EngineError::InvalidParams(format!(
                "failed to resolve terminal cwd {}: {e}",
                candidate.display()
            ))
        })?;
        canonical
            .to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| EngineError::InvalidParams("terminal cwd is not valid UTF-8".to_string()))
    }

    async fn spawn_pty_for_thread(
        self: &Arc<Self>,
        thread_id: &str,
        cwd: PathBuf,
        rows: u16,
        cols: u16,
    ) -> Result<Arc<InteractivePtySession>, EngineError> {
        self.ensure_pty_notifier_started();
        let notify_tx = self.pty_notify_tx.clone();
        let tid_for_cb = thread_id.to_string();
        let on_update: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = notify_tx.send(tid_for_cb.clone());
        });
        let clamped_rows = rows.max(4);
        let clamped_cols = cols.max(10);
        let spawned = tokio::task::spawn_blocking(move || {
            InteractivePtySession::spawn(&cwd, clamped_rows, clamped_cols, None, on_update)
        })
        .await
        .map_err(|e| EngineError::Transport(format!("PTY spawn task panicked: {e}")))?
        .map_err(|e| EngineError::Transport(format!("failed to spawn PTY session: {e}")))?;

        let session = Arc::new(spawned);
        if let Ok(mut exited) = self.pty_exited_threads.lock() {
            exited.remove(thread_id);
        }
        if let Ok(mut map) = self.active_pty_sessions.lock() {
            map.insert(thread_id.to_string(), Arc::clone(&session));
        }
        let _ = self.pty_notify_tx.send(thread_id.to_string());
        Ok(session)
    }

    async fn ensure_pty_session(
        self: &Arc<Self>,
        thread_id: &str,
        thread: &Thread,
        rows: u16,
        cols: u16,
    ) -> Result<(Arc<InteractivePtySession>, bool), EngineError> {
        if let Some(existing) = self
            .active_pty_sessions
            .lock()
            .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?
            .get(thread_id)
            .cloned()
        {
            return Ok((existing, false));
        }

        let _guard = self.acquire_thread_turn_lock(thread_id).await?;
        let pending_size = {
            let sessions = self
                .active_pty_sessions
                .lock()
                .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?;
            if let Some(existing) = sessions.get(thread_id).cloned() {
                return Ok((existing, false));
            }
            self.pending_pty_sizes
                .lock()
                .map_err(|_| EngineError::Storage("pending pty sizes map poisoned".to_string()))?
                .remove(thread_id)
        };

        let (spawn_rows, spawn_cols, used_pending) = match pending_size {
            Some((pending_rows, pending_cols)) => (pending_rows, pending_cols, true),
            None => (
                if rows == 0 { 24 } else { rows },
                if cols == 0 { 80 } else { cols },
                false,
            ),
        };
        let cwd = self.resolve_thread_cwd(thread);
        let session = self
            .spawn_pty_for_thread(thread_id, cwd, spawn_rows, spawn_cols)
            .await?;

        let race_pending = self
            .pending_pty_sizes
            .lock()
            .map_err(|_| EngineError::Storage("pending pty sizes map poisoned".to_string()))?
            .remove(thread_id);
        if let Some((later_rows, later_cols)) = race_pending {
            let s_clone = Arc::clone(&session);
            tokio::task::spawn_blocking(move || s_clone.resize(later_rows, later_cols))
                .await
                .map_err(|e| EngineError::Transport(format!("PTY resize task panicked: {e}")))?
                .map_err(|e| EngineError::Transport(format!("PTY resize failed: {e}")))?;
            return Ok((session, true));
        }

        Ok((session, used_pending))
    }

    fn require_terminal_thread(&self, thread_id: &str) -> Result<(ThreadId, Thread), EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind != ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(format!(
                "thread {thread_id} is not a terminal thread"
            )));
        }
        Ok((tid, thread))
    }
}

fn finalize_eligible_subagents(
    runtime: &EngineRuntime,
    thread_id: &str,
    active_subagents: &mut HashMap<String, TrackedLayer1Subagent>,
    current_turn_seq: u64,
    current_wakeup_seq: u64,
    is_unsolicited: bool,
) {
    let watch = runtime.subagent_watch();
    let eligible_ids: Vec<String> = active_subagents
        .iter()
        .filter(|(_, sa)| {
            if is_unsolicited {
                !(sa.launched_unsolicited && sa.launched_wakeup_seq == current_wakeup_seq)
            } else {
                sa.launched_unsolicited || sa.launched_turn_seq < current_turn_seq
            }
        })
        .map(|(id, _)| id.clone())
        .collect();

    for id in eligible_ids {
        if !watch.is_confirmed_running(thread_id, &id) {
            watch.mark_terminal(
                thread_id,
                &id,
                SubagentStatus::Completed,
                Some("done".to_string()),
            );
            active_subagents.remove(&id);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_session_listener(
    runtime: Arc<EngineRuntime>,
    tid: ThreadId,
    thread_id: String,
    session_id: String,
    driver: Arc<AcpStdioDriver>,
    initial_leaf_id: NodeId,
    mut events: mpsc::UnboundedReceiver<DriverEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionListenerCmd>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut solicited_in_flight = false;
        let mut solicited_error_emitted = false;
        let mut solicited_turn_seq: u64 = 0;
        let mut wakeup_seq: u64 = 0;
        let mut wakeup_active = false;
        let mut solicited_done_tx: Option<oneshot::Sender<Result<NodeId, EngineError>>> = None;
        let mut accumulated_text = String::new();
        let mut current_leaf_id = initial_leaf_id;
        let mut unsolicited_streaming_emitted = false;
        let mut quiet_deadline: Option<tokio::time::Instant> = None;
        let mut active_tool_names: HashMap<String, String> = HashMap::new();
        let mut active_subagents: HashMap<String, TrackedLayer1Subagent> = HashMap::new();

        let finalize_unsolicited = |runtime: &EngineRuntime,
                                    current_leaf_id: &mut NodeId,
                                    accumulated_text: &mut String,
                                    unsolicited_streaming_emitted: &mut bool,
                                    quiet_deadline: &mut Option<tokio::time::Instant>,
                                    wakeup_active: &mut bool,
                                    active_subagents: &mut HashMap<String, TrackedLayer1Subagent>,
                                    solicited_turn_seq: u64,
                                    wakeup_seq: u64| {
            if !accumulated_text.trim().is_empty() {
                let text = std::mem::take(accumulated_text);
                if let Ok(new_leaf) = runtime.persist_assistant_node(tid, *current_leaf_id, text) {
                    *current_leaf_id = new_leaf;
                }
            } else {
                accumulated_text.clear();
            }
            let _ = runtime.storage.update_thread_status(tid, ThreadStatus::Idle);
            runtime.emit_tui(TuiEngineEvent::TurnCompleted {
                thread_id: thread_id.clone(),
            });
            runtime.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                thread_id: thread_id.clone(),
                status: "idle".to_string(),
            });
            *unsolicited_streaming_emitted = false;
            *quiet_deadline = None;
            *wakeup_active = false;
            finalize_eligible_subagents(
                runtime,
                &thread_id,
                active_subagents,
                solicited_turn_seq,
                wakeup_seq,
                true,
            );
        };

        loop {
            let sleep_fut = async {
                match quiet_deadline {
                    Some(dl) => tokio::time::sleep_until(dl).await,
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                cmd_opt = cmd_rx.recv() => {
                    let Some(cmd) = cmd_opt else {
                        break;
                    };
                    match cmd {
                        SessionListenerCmd::PrepareSolicited { reply_tx } => {
                            if !solicited_in_flight && !accumulated_text.trim().is_empty() {
                                finalize_unsolicited(
                                    &runtime,
                                    &mut current_leaf_id,
                                    &mut accumulated_text,
                                    &mut unsolicited_streaming_emitted,
                                    &mut quiet_deadline,
                                    &mut wakeup_active,
                                    &mut active_subagents,
                                    solicited_turn_seq,
                                    wakeup_seq,
                                );
                            } else {
                                quiet_deadline = None;
                                wakeup_active = false;
                            }
                            if let Ok(Some(t)) = runtime.storage.get_thread(tid) {
                                current_leaf_id = t.active_leaf_id;
                            }
                            let _ = reply_tx.send(current_leaf_id);
                        }
                        SessionListenerCmd::StartSolicited { user_node_id, done_tx, ack_tx } => {
                            solicited_in_flight = true;
                            solicited_error_emitted = false;
                            solicited_turn_seq += 1;
                            current_leaf_id = user_node_id;
                            accumulated_text.clear();
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            solicited_done_tx = Some(done_tx);
                            let _ = ack_tx.send(());
                        }
                        SessionListenerCmd::AbortSolicited { ack_tx } => {
                            solicited_in_flight = false;
                            solicited_done_tx = None;
                            accumulated_text.clear();
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            let already_emitted = solicited_error_emitted;
                            solicited_error_emitted = false;
                            let _ = ack_tx.send(already_emitted);
                        }
                    }
                }
                ev_opt = events.recv() => {
                    let Some(ev) = ev_opt else {
                        if solicited_in_flight {
                            let msg = "ACP stream closed before PromptCompleted".to_string();
                            runtime.mark_thread_error(tid, &thread_id, &msg);
                            if let Some(tx) = solicited_done_tx.take() {
                                let _ = tx.send(Err(EngineError::Transport(msg)));
                            }
                        } else if !accumulated_text.trim().is_empty() {
                            finalize_unsolicited(
                                &runtime,
                                &mut current_leaf_id,
                                &mut accumulated_text,
                                &mut unsolicited_streaming_emitted,
                                &mut quiet_deadline,
                                &mut wakeup_active,
                                &mut active_subagents,
                                solicited_turn_seq,
                                wakeup_seq,
                            );
                        }
                        break;
                    };

                    match ev {
                        DriverEvent::StreamChunk { text, .. } => {
                            if !solicited_in_flight {
                                if !wakeup_active {
                                    wakeup_active = true;
                                    wakeup_seq += 1;
                                }
                                if !unsolicited_streaming_emitted {
                                    unsolicited_streaming_emitted = true;
                                    let _ = runtime.storage.update_thread_status(tid, ThreadStatus::Streaming);
                                    runtime.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                                        thread_id: thread_id.clone(),
                                        status: "streaming".to_string(),
                                    });
                                }
                                quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                            }
                            accumulated_text.push_str(&text);
                            runtime.emit_tui(TuiEngineEvent::StreamChunk {
                                thread_id: thread_id.clone(),
                                content: text,
                            });
                        }
                        DriverEvent::ThoughtChunk { text, .. } => {
                            if !solicited_in_flight {
                                if !wakeup_active {
                                    wakeup_active = true;
                                    wakeup_seq += 1;
                                }
                                if quiet_deadline.is_some() {
                                    quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                                }
                            }
                            runtime.emit_tui(TuiEngineEvent::ThoughtChunk {
                                thread_id: thread_id.clone(),
                                content: text,
                            });
                        }
                        DriverEvent::ToolCall { tool_call_id, name, title, args, .. } => {
                            if !solicited_in_flight {
                                if !wakeup_active {
                                    wakeup_active = true;
                                    wakeup_seq += 1;
                                }
                                if quiet_deadline.is_some() {
                                    quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                                }
                            }
                            let tool_name = if !name.is_empty() {
                                name.clone()
                            } else if !title.is_empty() {
                                title.clone()
                            } else {
                                "tool".to_string()
                            };
                            active_tool_names.insert(tool_call_id.clone(), tool_name.clone());
                            let summary = extract_tool_call_summary(&tool_name, &title, &args);
                            runtime.emit_tui(TuiEngineEvent::ToolInvoked {
                                thread_id: thread_id.clone(),
                                tool_name: tool_name.clone(),
                                tool_call_id: (!tool_call_id.is_empty())
                                    .then(|| tool_call_id.clone()),
                                summary,
                            });
                            runtime.emit_tui(TuiEngineEvent::ToolActivityUpdated {
                                thread_id: thread_id.clone(),
                                tool_name: Some(tool_name.clone()),
                            });

                            if name == "invoke_subagent"
                                || name.contains("invoke_subagent")
                                || tool_name.contains("invoke_subagent")
                            {
                                let snap = SubagentSnapshot {
                                    id: tool_call_id.clone(),
                                    role: extract_subagent_role(&args, &title),
                                    agent_type: extract_subagent_type(&args),
                                    model: None,
                                    activity: Some("running".to_string()),
                                    status: SubagentStatus::Running,
                                    started_at_ms: now_epoch_ms(),
                                };
                                active_subagents.insert(
                                    tool_call_id.clone(),
                                    TrackedLayer1Subagent {
                                        snapshot: snap.clone(),
                                        launched_turn_seq: solicited_turn_seq,
                                        launched_unsolicited: !solicited_in_flight,
                                        launched_wakeup_seq: wakeup_seq,
                                    },
                                );
                                runtime.emit_tui(TuiEngineEvent::SubagentUpdated {
                                    thread_id: thread_id.clone(),
                                    subagent: snap.clone(),
                                });
                                runtime.subagent_watch().register_subagent(&thread_id, snap);
                            }
                        }
                        DriverEvent::ToolCallUpdate {
                            tool_call_id,
                            name,
                            status,
                            output,
                            ..
                        } => {
                            if !solicited_in_flight && quiet_deadline.is_some() {
                                quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                            }
                            let is_done = status.eq_ignore_ascii_case("completed")
                                || status.eq_ignore_ascii_case("done");
                            let is_failed = status.eq_ignore_ascii_case("failed")
                                || status.eq_ignore_ascii_case("error");

                            if is_done || is_failed {
                                let resolved_name = if !name.is_empty() {
                                    active_tool_names.remove(&tool_call_id);
                                    name
                                } else {
                                    active_tool_names
                                        .remove(&tool_call_id)
                                        .unwrap_or_else(|| "tool".to_string())
                                };
                                let resolved_output = if output.is_empty() && !is_done {
                                    status.clone()
                                } else {
                                    output.clone()
                                };
                                runtime.emit_tui(TuiEngineEvent::ToolResult {
                                    thread_id: thread_id.clone(),
                                    tool_name: resolved_name,
                                    output: resolved_output.clone(),
                                    tool_call_id: (!tool_call_id.is_empty())
                                        .then(|| tool_call_id.clone()),
                                    is_error: is_failed,
                                });
                                runtime.emit_tui(TuiEngineEvent::ToolActivityUpdated {
                                    thread_id: thread_id.clone(),
                                    tool_name: None,
                                });

                                let watch = runtime.subagent_watch();
                                if let Some(sa) = active_subagents.get_mut(&tool_call_id) {
                                    if is_failed {
                                        let reason = if resolved_output.trim().is_empty() {
                                            "failed".to_string()
                                        } else {
                                            resolved_output.trim().chars().take(60).collect()
                                        };
                                        watch.mark_terminal(
                                            &thread_id,
                                            &tool_call_id,
                                            SubagentStatus::Failed,
                                            Some(reason),
                                        );
                                        active_subagents.remove(&tool_call_id);
                                    } else {
                                        watch.ingest_tool_output(&thread_id, &tool_call_id, &output);
                                        sa.snapshot.activity = Some("background active".to_string());
                                        watch.update_activity_if_unlinked(
                                            &thread_id,
                                            &tool_call_id,
                                            "background active",
                                        );
                                    }
                                }
                            }
                        }
                        DriverEvent::SessionInfoUpdate { title, .. } => {
                            let trimmed = title.trim();
                            if !trimmed.is_empty() {
                                if let Ok(Some(current)) = runtime.storage.get_thread(tid) {
                                    if (current.title.trim().is_empty()
                                        || current.title.starts_with("Branch"))
                                        && runtime.storage.update_thread_title(&tid, trimmed).is_ok()
                                    {
                                        runtime.emit_tui(TuiEngineEvent::ThreadTitleUpdated {
                                            thread_id: thread_id.clone(),
                                            title: trimmed.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                        DriverEvent::UsageUpdate { .. } => {
                            if !solicited_in_flight && !accumulated_text.trim().is_empty() {
                                finalize_unsolicited(
                                    &runtime,
                                    &mut current_leaf_id,
                                    &mut accumulated_text,
                                    &mut unsolicited_streaming_emitted,
                                    &mut quiet_deadline,
                                    &mut wakeup_active,
                                    &mut active_subagents,
                                    solicited_turn_seq,
                                    wakeup_seq,
                                );
                            }
                        }
                        DriverEvent::PermissionRequested {
                            request_id,
                            tool_name,
                            title,
                            args,
                            options,
                            ..
                        } => {
                            if !solicited_in_flight {
                                quiet_deadline = None;
                            }
                            let seq = runtime.next_perm_seq.fetch_add(1, Ordering::Relaxed);
                            let request_key = format!("{session_id}:{seq}");
                            let description = format_permission_description(&tool_name, &title, &args);
                            if let Ok(mut state) = runtime.lock() {
                                state.pending_permissions.insert(
                                    request_key.clone(),
                                    PendingPermissionRecord {
                                        driver: Arc::clone(&driver),
                                        json_rpc_id: request_id,
                                        options,
                                    },
                                );
                            }
                            let _ = runtime
                                .storage
                                .update_thread_status(tid, ThreadStatus::PermissionRequired);
                            runtime.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                                thread_id: thread_id.clone(),
                                status: "permission_required".to_string(),
                            });
                            runtime.emit_tui(TuiEngineEvent::PermissionRequested {
                                request_id: request_key,
                                description,
                            });
                        }
                        DriverEvent::PromptCompleted { .. } => {
                            if !accumulated_text.is_empty() {
                                let text = std::mem::take(&mut accumulated_text);
                                match runtime.persist_assistant_node(tid, current_leaf_id, text) {
                                    Ok(new_leaf) => current_leaf_id = new_leaf,
                                    Err(err) => {
                                        let msg = err.to_string();
                                        runtime.mark_thread_error(tid, &thread_id, &msg);
                                        solicited_in_flight = false;
                                        solicited_error_emitted = true;
                                        if let Some(tx) = solicited_done_tx.take() {
                                            let _ = tx.send(Err(err));
                                        }
                                        continue;
                                    }
                                }
                            }
                            let _ = runtime.storage.update_thread_status(tid, ThreadStatus::Idle);
                            runtime.emit_tui(TuiEngineEvent::TurnCompleted {
                                thread_id: thread_id.clone(),
                            });
                            runtime.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                                thread_id: thread_id.clone(),
                                status: "idle".to_string(),
                            });
                            solicited_in_flight = false;
                            solicited_error_emitted = false;
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            finalize_eligible_subagents(
                                &runtime,
                                &thread_id,
                                &mut active_subagents,
                                solicited_turn_seq,
                                wakeup_seq,
                                false,
                            );
                            if let Some(tx) = solicited_done_tx.take() {
                                let _ = tx.send(Ok(current_leaf_id));
                            }
                        }
                        DriverEvent::Error { message, .. } => {
                            runtime.mark_thread_error(tid, &thread_id, &message);
                            accumulated_text.clear();
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            if solicited_in_flight {
                                solicited_in_flight = false;
                                solicited_error_emitted = true;
                                if let Some(tx) = solicited_done_tx.take() {
                                    let _ = tx.send(Err(EngineError::Transport(message)));
                                }
                            }
                        }
                        DriverEvent::CurrentModeUpdate {
                            current_mode_id, ..
                        } => {
                            let mut updated_meta = None;
                            let mut updated_opts = None;
                            if let Ok(mut state) = runtime.lock() {
                                if let Some(entry) = state.active_acp_sessions.get_mut(&thread_id) {
                                    if entry.session_id == session_id
                                        && !current_mode_id.trim().is_empty()
                                    {
                                        entry.mode = Some(current_mode_id.clone());
                                        let mut changed_opt = false;
                                        for opt in &mut entry.config_options {
                                            if (opt.id == "mode"
                                                || opt.category.as_deref() == Some("mode"))
                                                && opt.current_value != current_mode_id
                                            {
                                                opt.current_value = current_mode_id.clone();
                                                changed_opt = true;
                                            }
                                        }
                                        updated_meta = Some((entry.model.clone(), entry.mode.clone()));
                                        if changed_opt {
                                            updated_opts = Some(entry.config_options.clone());
                                        }
                                    }
                                }
                            }
                            if let Some((model, mode)) = updated_meta {
                                runtime.emit_tui(TuiEngineEvent::AgentMetaUpdated {
                                    thread_id: thread_id.clone(),
                                    model,
                                    mode,
                                });
                            }
                            if let Some(config_options) = updated_opts {
                                runtime.emit_tui(TuiEngineEvent::AgentConfigUpdated {
                                    thread_id: thread_id.to_string(),
                                    config_options,
                                });
                            }
                        }
                        DriverEvent::ConfigOptionUpdate { config_options, .. } => {
                            let incoming: Vec<AgentConfigOption> = config_options
                                .iter()
                                .map(acp_config_option_to_tui)
                                .collect();
                            let mut emit_opts = None;
                            let mut emit_meta = None;
                            if let Ok(mut state) = runtime.lock() {
                                if let Some(entry) = state.active_acp_sessions.get_mut(&thread_id) {
                                    if entry.session_id == session_id {
                                        let (merged, next_legacy_model, next_legacy_mode) =
                                            merge_updated_config_options(
                                                &entry.config_options,
                                                incoming,
                                                entry.has_legacy_model,
                                                entry.has_legacy_mode,
                                            );
                                        entry.has_legacy_model = next_legacy_model;
                                        entry.has_legacy_mode = next_legacy_mode;
                                        let (new_model, new_mode) =
                                            extract_model_and_mode_from_options(
                                                &merged,
                                                entry.model.clone(),
                                                entry.mode.clone(),
                                            );
                                        let meta_changed =
                                            new_model != entry.model || new_mode != entry.mode;
                                        entry.config_options = merged.clone();
                                        entry.model = new_model.clone();
                                        entry.mode = new_mode.clone();
                                        emit_opts = Some(merged);
                                        if meta_changed || new_model.is_some() || new_mode.is_some()
                                        {
                                            emit_meta = Some((new_model, new_mode));
                                        }
                                    }
                                }
                            }
                            if let Some(config_options) = emit_opts {
                                runtime.emit_tui(TuiEngineEvent::AgentConfigUpdated {
                                    thread_id: thread_id.clone(),
                                    config_options,
                                });
                            }
                            if let Some((model, mode)) = emit_meta {
                                runtime.emit_tui(TuiEngineEvent::AgentMetaUpdated {
                                    thread_id: thread_id.clone(),
                                    model,
                                    mode,
                                });
                            }
                        }
                        DriverEvent::AvailableCommandsUpdate { commands, .. } => {
                            let converted: Vec<AgentSlashCommand> = commands
                                .into_iter()
                                .map(|c| AgentSlashCommand {
                                    name: c.name,
                                    description: c.description,
                                })
                                .collect();
                            let mut updated = false;
                            if let Ok(mut state) = runtime.lock() {
                                if let Some(entry) = state.active_acp_sessions.get_mut(&thread_id) {
                                    if entry.session_id == session_id {
                                        entry.available_commands = converted.clone();
                                        updated = true;
                                    }
                                }
                            }
                            if updated {
                                runtime.emit_tui(TuiEngineEvent::AgentCommandsUpdated {
                                    thread_id: thread_id.clone(),
                                    commands: converted,
                                });
                            }
                        }
                    }
                }
                _ = sleep_fut, if quiet_deadline.is_some() => {
                    if !solicited_in_flight && !accumulated_text.trim().is_empty() {
                        finalize_unsolicited(
                            &runtime,
                            &mut current_leaf_id,
                            &mut accumulated_text,
                            &mut unsolicited_streaming_emitted,
                            &mut quiet_deadline,
                            &mut wakeup_active,
                            &mut active_subagents,
                            solicited_turn_seq,
                            wakeup_seq,
                        );
                    } else {
                        quiet_deadline = None;
                        wakeup_active = false;
                    }
                }
            }
        }
    })
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
        let last_node = threads
            .first()
            .map(|t| t.active_leaf_id)
            .or_else(|| nodes.last().map(|n| n.id));

        let tui_subscribers = Arc::new(Mutex::new(Vec::<mpsc::UnboundedSender<TuiEngineEvent>>::new()));
        let subs_for_watch = Arc::clone(&tui_subscribers);
        let brain_root = SubagentWatcher::default_brain_root()
            .unwrap_or_else(|| cwd.join(".grein").join("brain_missing"));
        let watcher = SubagentWatcher::new(
            brain_root,
            Arc::new(move |ev| {
                if let Ok(mut subs) = subs_for_watch.lock() {
                    subs.retain(|tx| tx.send(ev.clone()).is_ok());
                }
            }),
        );

        let (pty_notify_tx, pty_notify_rx) = mpsc::unbounded_channel();

        Ok(Self {
            runtime: Arc::new(EngineRuntime {
                storage,
                cwd,
                acp_config,
                acp_driver: AsyncMutex::new(None),
                driver_gen_seq: AtomicU64::new(1),
                state: Mutex::new(EngineState {
                    last_node,
                    ..Default::default()
                }),
                tui_subscribers,
                next_perm_seq: AtomicU64::new(1),
                subagent_watch: Mutex::new(watcher),
                unsolicited_quiet_timeout: Mutex::new(UNSOLICITED_QUIET_TIMEOUT),
                active_pty_sessions: Arc::new(Mutex::new(HashMap::new())),
                pending_pty_sizes: Arc::new(Mutex::new(HashMap::new())),
                pty_exited_threads: Mutex::new(HashSet::new()),
                pty_notify_tx,
                pty_notify_rx: Mutex::new(Some(pty_notify_rx)),
                pty_notifier_handle: Mutex::new(None),
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn set_test_subagent_brain(&self, brain_root: PathBuf) {
        let subs_for_watch = Arc::clone(&self.runtime.tui_subscribers);
        let watcher = SubagentWatcher::with_timings(
            brain_root,
            Arc::new(move |ev| {
                if let Ok(mut subs) = subs_for_watch.lock() {
                    subs.retain(|tx| tx.send(ev.clone()).is_ok());
                }
            }),
            Duration::from_millis(50),
            Duration::from_millis(50),
        );
        let mut guard = self
            .runtime
            .subagent_watch
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        guard.shutdown();
        *guard = watcher;
    }

    #[cfg(test)]
    pub(crate) fn set_test_quiet_timeout(&self, dur: Duration) {
        *self
            .runtime
            .unsolicited_quiet_timeout
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = dur;
    }

    pub fn subscribe_tui_events(&self) -> mpsc::UnboundedReceiver<TuiEngineEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut subs) = self.runtime.tui_subscribers.lock() {
            subs.push(tx);
        }
        rx
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

    pub async fn create_root_thread(
        &self,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::create_root_thread(self, title).await
    }

    pub async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::rename_thread(self, thread_id, title).await
    }

    pub async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::merge_thread_with_summary(self, source_id, target_id, summary).await
    }

    pub async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        DaemonEngine::get_thread_transcript(self, thread_id).await
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

    pub async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        DaemonEngine::thread_metadata(self).await
    }

    pub async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        DaemonEngine::delete_thread(self, thread_id).await
    }

    pub async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        DaemonEngine::agent_options(self, thread_id).await
    }

    pub async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        DaemonEngine::set_agent_config(self, thread_id, config_id, value, set_default).await
    }

    pub async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::create_terminal(self, parent_thread_id, title, cwd, rows, cols).await
    }

    pub async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        DaemonEngine::pty_write(self, thread_id, bytes).await
    }

    pub async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        DaemonEngine::pty_resize(self, thread_id, rows, cols).await
    }

    pub async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_snapshot(self, thread_id, rows, cols).await
    }

    pub async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        DaemonEngine::ensure_thread_worktree(self, thread_id).await
    }

    pub async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_scroll(self, thread_id, delta).await
    }

    pub async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_restart(self, thread_id, rows, cols).await
    }
}

#[async_trait]
impl DaemonEngine for InProcessEngine {
    async fn ping(&self) -> Result<(), EngineError> {
        self.runtime.lock().map(|_| ())
    }

    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        let mut state = self.runtime.lock()?;
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        if let Some(first_agent) = threads.iter().find(|t| t.kind == ThreadKind::Agent) {
            let is_first = threads.first().is_some_and(|t| t.id == first_agent.id);
            return Ok(thread_to_summary(first_agent, is_first));
        }

        let nodes = self.runtime.storage.list_nodes().map_err(storage_err)?;
        let (root_id, leaf_id) = if let (Some(first), Some(last)) = (nodes.first(), nodes.last()) {
            (first.id, last.id)
        } else {
            let root = ConversationNode::new(
                NodeId::new(),
                None::<NodeId>,
                AuthorRole::System,
                INTERNAL_ROOT_CONTENT.to_string(),
            );
            self.runtime
                .storage
                .insert_node(&root)
                .map_err(storage_err)?;
            (root.id, root.id)
        };

        let anchor_id = AnchorId::new();
        let anchor = Anchor {
            level: 1,
            title: "Main".to_string(),
            slug: "main".to_string(),
            line_number: 1,
        };
        self.runtime
            .storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;
        let tid = self
            .runtime
            .storage
            .create_thread_with_title(root_id, anchor_id, "Main")
            .map_err(storage_err)?;
        if leaf_id != root_id {
            self.runtime
                .storage
                .update_thread_leaf(tid, leaf_id)
                .map_err(storage_err)?;
        }
        state.last_node = Some(leaf_id);
        let created = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("created Main thread missing".to_string()))?;
        Ok(thread_to_summary(&created, true))
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        if self.runtime.acp_config.is_none() {
            let mut state = self.runtime.lock()?;
            let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
            let target_agent = threads.iter().find(|t| t.kind == ThreadKind::Agent);
            if target_agent.is_none() && !threads.is_empty() {
                return Err(EngineError::InvalidParams(
                    "cannot send agent prompt to a terminal thread".to_string(),
                ));
            }
            let parent = target_agent.map(|t| t.active_leaf_id).or(state.last_node);
            let node_id = NodeId::new();
            let node = ConversationNode::new(node_id, parent, AuthorRole::User, prompt.to_string());
            self.runtime
                .storage
                .insert_node(&node)
                .map_err(storage_err)?;
            if let Some(agent_thread) = target_agent {
                self.runtime
                    .storage
                    .update_thread_leaf(agent_thread.id, node_id)
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
        let existing_thread = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if existing_thread.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot send agent prompt to a terminal thread".to_string(),
            ));
        }

        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;

        // Flush any buffered unsolicited wakeup on an existing active session before inserting user node.
        let existing_cmd_tx = {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .map(|s| s.cmd_tx.clone())
        };
        if let Some(cmd_tx) = existing_cmd_tx {
            let (reply_tx, reply_rx) = oneshot::channel();
            if cmd_tx
                .send(SessionListenerCmd::PrepareSolicited { reply_tx })
                .is_ok()
            {
                let _ = reply_rx.await;
            }
        }

        let thread = self
            .runtime
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
        self.runtime
            .storage
            .insert_node(&user_node)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .update_thread_leaf(tid, user_node_id)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .update_thread_status(tid, ThreadStatus::Streaming)
            .map_err(storage_err)?;
        {
            let mut state = self.runtime.lock()?;
            state.last_node = Some(user_node_id);
        }

        self.runtime.emit_tui(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "streaming".to_string(),
        });

        let Some(cfg) = self.runtime.acp_config.clone() else {
            let msg = "no ACP agent configured; install agy or set GREIN_ACP_BIN".to_string();
            self.runtime.mark_thread_error(tid, thread_id, &msg);
            return Err(EngineError::Transport(msg));
        };

        let (driver, driver_gen) = match self.runtime.ensure_driver(&cfg).await {
            Ok(pair) => pair,
            Err(err) => {
                self.runtime
                    .mark_thread_error(tid, thread_id, &err.to_string());
                return Err(err);
            }
        };

        let existing_handle = {
            let mut state = self.runtime.lock()?;
            let stale = state
                .active_acp_sessions
                .get(thread_id)
                .is_some_and(|s| s.driver_gen != driver_gen || s.cmd_tx.is_closed());
            if stale {
                state.active_acp_sessions.remove(thread_id);
            }
            state
                .active_acp_sessions
                .get(thread_id)
                .map(|s| ActiveSessionHandle {
                    session_id: s.session_id.clone(),
                    driver_gen: s.driver_gen,
                    cmd_tx: s.cmd_tx.clone(),
                })
        };

        let (session_handle, outgoing_prompt) = match existing_handle {
            Some(handle) => {
                let prompt_text = {
                    let mut state = self.runtime.lock()?;
                    let landmarks = state
                        .active_acp_sessions
                        .get_mut(thread_id)
                        .map(|entry| std::mem::take(&mut entry.pending_landmarks))
                        .unwrap_or_default();
                    if landmarks.is_empty() {
                        prompt.to_string()
                    } else {
                        format!(
                            "[Joined Context]\n{}\n\n---\n\n{prompt}",
                            landmarks.join("\n\n")
                        )
                    }
                };
                (handle, prompt_text)
            }
            None => {
                match self
                    .runtime
                    .ensure_acp_session_for_thread(
                        &cfg,
                        tid,
                        thread_id,
                        user_node_id,
                        Some(user_node_id),
                        true,
                    )
                    .await
                {
                    Ok((_, handle)) => (handle, prompt.to_string()),
                    Err(err) => {
                        self.runtime
                            .mark_thread_error(tid, thread_id, &err.to_string());
                        return Err(err);
                    }
                }
            }
        };

        let (done_tx, mut done_rx) = oneshot::channel();
        let (ack_tx, ack_rx) = oneshot::channel();
        if session_handle
            .cmd_tx
            .send(SessionListenerCmd::StartSolicited {
                user_node_id,
                done_tx,
                ack_tx,
            })
            .is_err()
            || ack_rx.await.is_err()
        {
            let msg = "ACP session listener stopped before prompt dispatch".to_string();
            self.runtime.mark_thread_error(tid, thread_id, &msg);
            return Err(EngineError::Transport(msg));
        }

        let submit_res = tokio::select! {
            res = driver.submit_prompt(&session_handle.session_id, &outgoing_prompt) => Some(res),
            done = &mut done_rx => {
                match done {
                    Ok(Ok(leaf_id)) => {
                        return Ok(TurnDispatchResult {
                            node_id: leaf_id.0.to_string(),
                        });
                    }
                    Ok(Err(err)) => return Err(err),
                    Err(_) => {
                        let msg = "ACP session listener closed before PromptCompleted".to_string();
                        self.runtime.mark_thread_error(tid, thread_id, &msg);
                        return Err(EngineError::Transport(msg));
                    }
                }
            }
        };

        if let Some(Err(err)) = submit_res {
            self.runtime
                .invalidate_driver_if_dead(&driver, session_handle.driver_gen)
                .await;
            let (abort_ack_tx, abort_ack_rx) = oneshot::channel();
            let already_emitted = if session_handle
                .cmd_tx
                .send(SessionListenerCmd::AbortSolicited {
                    ack_tx: abort_ack_tx,
                })
                .is_ok()
            {
                abort_ack_rx.await.unwrap_or(false)
            } else {
                false
            };
            let msg = format!("ACP submit_prompt failed: {err}");
            if !already_emitted {
                self.runtime.mark_thread_error(tid, thread_id, &msg);
            }
            return Err(EngineError::Transport(msg));
        }

        match done_rx.await {
            Ok(Ok(leaf_id)) => Ok(TurnDispatchResult {
                node_id: leaf_id.0.to_string(),
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => {
                self.runtime
                    .invalidate_driver_if_dead(&driver, session_handle.driver_gen)
                    .await;
                let msg = "ACP stream closed before PromptCompleted".to_string();
                self.runtime.mark_thread_error(tid, thread_id, &msg);
                Err(EngineError::Transport(msg))
            }
        }
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
        let existing_threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let (parent_node_id, parent_thread_id, parent_cwd) =
            match source_thread_or_node.map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => {
                    if let Some(tid) = parse_typed_id::<ThreadId>(raw) {
                        if let Some(thread) =
                            self.runtime.storage.get_thread(tid).map_err(storage_err)?
                        {
                            if thread.kind == ThreadKind::Terminal {
                                return Err(EngineError::InvalidParams(
                                    "cannot fork an agent branch from a terminal thread"
                                        .to_string(),
                                ));
                            }
                            (thread.active_leaf_id, Some(thread.id), thread.cwd)
                        } else if let Some(node) = self
                            .runtime
                            .storage
                            .get_node(NodeId(tid.0))
                            .map_err(storage_err)?
                        {
                            (node.id, None, None)
                        } else {
                            return Err(EngineError::NotFound(format!("thread or node {raw}")));
                        }
                    } else {
                        return Err(EngineError::NotFound(format!("thread or node {raw}")));
                    }
                }
                None => {
                    if let Some(first) = existing_threads.first() {
                        if first.kind == ThreadKind::Terminal {
                            return Err(EngineError::InvalidParams(
                                "cannot fork an agent branch from a terminal thread".to_string(),
                            ));
                        }
                        (first.active_leaf_id, Some(first.id), first.cwd.clone())
                    } else if let Some(last_node) = self.runtime.lock()?.last_node {
                        (last_node, None, None)
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
        self.runtime
            .storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;
        let tid = self
            .runtime
            .storage
            .create_thread_with_kind(
                parent_node_id,
                anchor_id,
                Some(&resolved_title),
                parent_thread_id,
                ThreadKind::Agent,
                parent_cwd.as_deref(),
            )
            .map_err(storage_err)?;
        let created = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("forked thread missing".to_string()))?;

        let turns: Vec<HydratedTurn> = self
            .runtime
            .load_thread_turns(&created)?
            .into_iter()
            .map(|e| HydratedTurn {
                id: e.node_id,
                role: e.role,
                content: e.content,
            })
            .collect();

        let summary = thread_to_summary(&created, false);
        self.runtime.emit_tui(TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: summary.thread_id.clone(),
                title: summary.title.clone(),
                turns,
            },
        });
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        Ok(summary)
    }

    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        let existing_threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let resolved_title = match title.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => t.to_string(),
            None => {
                let meta_before = self.runtime.compute_thread_metadata()?;
                let mut n = meta_before
                    .iter()
                    .filter(|m| m.parent_thread_id.is_none())
                    .count()
                    + 1;
                while existing_threads
                    .iter()
                    .any(|t| t.title == format!("Root {n}"))
                {
                    n += 1;
                }
                format!("Root {n}")
            }
        };

        let root_node = ConversationNode::new_root(AuthorRole::System, "");
        self.runtime
            .storage
            .insert_node(&root_node)
            .map_err(storage_err)?;

        let anchor_id = AnchorId::new();
        let anchor = Anchor {
            level: 1,
            title: resolved_title.clone(),
            slug: slugify_anchor(&resolved_title),
            line_number: 1,
        };
        self.runtime
            .storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;

        let tid = self
            .runtime
            .storage
            .create_thread_with_parent(root_node.id, anchor_id, Some(&resolved_title), None)
            .map_err(storage_err)?;
        {
            let mut state = self.runtime.lock()?;
            state.last_node = Some(root_node.id);
        }

        let created = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("created root thread missing".to_string()))?;

        let summary = thread_to_summary(&created, false);
        self.runtime.emit_tui(TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: summary.thread_id.clone(),
                title: summary.title.clone(),
                turns: Vec::new(),
            },
        });
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        Ok(summary)
    }

    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(EngineError::InvalidParams(
                "title must not be empty".to_string(),
            ));
        }

        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        self.runtime
            .storage
            .update_thread_title(&tid, trimmed)
            .map_err(|e| match e {
                CoreError::ThreadNotFound(_) => EngineError::NotFound(format!("thread {thread_id}")),
                other => storage_err(other),
            })?;

        let updated = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let is_first = threads.first().is_some_and(|t| t.id == updated.id);
        let summary = thread_to_summary(&updated, is_first);

        self.runtime.emit_tui(TuiEngineEvent::ThreadTitleUpdated {
            thread_id: summary.thread_id.clone(),
            title: summary.title.clone(),
        });
        Ok(summary)
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        Ok(threads
            .iter()
            .enumerate()
            .map(|(i, t)| thread_to_summary(t, i == 0))
            .collect())
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        if let Some(first) = threads.first() {
            return self.runtime.load_thread_turns(first);
        }
        let nodes = self.runtime.storage.list_nodes().map_err(storage_err)?;
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
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        self.runtime.load_thread_turns(&thread)
    }

    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::merge_thread_with_summary(self, thread_id, None, None).await
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let source_tid: ThreadId = parse_typed_id(source_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {source_id}")))?;
        let source = self
            .runtime
            .storage
            .get_thread(source_tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {source_id}")))?;
        if source.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot merge a terminal thread into an agent context".to_string(),
            ));
        }

        let explicit_target = match target_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => match parse_typed_id::<ThreadId>(raw) {
                Some(tid) if tid != source_tid => {
                    self.runtime.storage.get_thread(tid).map_err(storage_err)?
                }
                _ => None,
            },
            None => None,
        };

        let target_thread = match explicit_target {
            Some(t) => Some(t),
            None => {
                let meta = self.runtime.compute_thread_metadata()?;
                let parent_tid = meta
                    .into_iter()
                    .find(|m| m.thread_id == source.id.0.to_string())
                    .and_then(|m| m.parent_thread_id)
                    .and_then(|pid| parse_typed_id::<ThreadId>(&pid))
                    .filter(|tid| *tid != source_tid);
                match parent_tid {
                    Some(pid) => self.runtime.storage.get_thread(pid).map_err(storage_err)?,
                    None => None,
                }
            }
        };

        let Some(target) = target_thread.filter(|t| t.id != source_tid) else {
            return Err(EngineError::InvalidParams(
                "cannot join a root context without a parent branch".to_string(),
            ));
        };
        if target.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot merge into a terminal thread".to_string(),
            ));
        }

        let source_threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let source_is_first = source_threads.first().is_some_and(|t| t.id == source.id);
        let source_title = thread_to_summary(&source, source_is_first).title;
        let resolved_summary = match summary.map(str::trim).filter(|s| !s.is_empty()) {
            Some(explicit) => truncate_chars_with_ellipsis(explicit, 400),
            None => {
                let turns = self.runtime.load_thread_turns(&source)?;
                let raw_digest = turns
                    .iter()
                    .rev()
                    .find(|t| t.role == "assistant" && !t.content.trim().is_empty())
                    .or_else(|| {
                        turns
                            .iter()
                            .rev()
                            .find(|t| t.role == "user" && !t.content.trim().is_empty())
                    })
                    .or_else(|| turns.iter().rev().find(|t| !t.content.trim().is_empty()))
                    .map(|t| t.content.as_str())
                    .unwrap_or(source_title.as_str());
                let normalized = raw_digest.split_whitespace().collect::<Vec<_>>().join(" ");
                truncate_chars_with_ellipsis(&normalized, 400)
            }
        };

        let target_id_str = target.id.0.to_string();
        let target_cmd_tx = {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(&target_id_str)
                .map(|s| s.cmd_tx.clone())
        };
        if let Some(cmd_tx) = target_cmd_tx {
            let (reply_tx, reply_rx) = oneshot::channel();
            if cmd_tx
                .send(SessionListenerCmd::PrepareSolicited { reply_tx })
                .is_ok()
            {
                let _ = reply_rx.await;
            }
        }
        let fresh_target = self
            .runtime
            .storage
            .get_thread(target.id)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {target_id_str}")))?;
        let landmark_node = synthesize_thread_merge(
            source_tid,
            &source_title,
            &resolved_summary,
            Some(fresh_target.active_leaf_id),
        )
        .map_err(storage_err)?;
        self.runtime
            .storage
            .insert_node(&landmark_node)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .update_thread_leaf(fresh_target.id, landmark_node.id)
            .map_err(storage_err)?;
        {
            let mut state = self.runtime.lock()?;
            state.last_node = Some(landmark_node.id);
            if let Some(entry) = state.active_acp_sessions.get_mut(&target_id_str) {
                entry.pending_landmarks.push(landmark_node.content.clone());
            }
        }

        self.runtime
            .storage
            .mark_thread_merged(&source_tid, true)
            .map_err(storage_err)?;
        let updated = self
            .runtime
            .storage
            .get_thread(source_tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {source_id}")))?;

        self.runtime.emit_tui(TuiEngineEvent::ThreadMerged {
            thread_id: updated.id.0.to_string(),
            target_thread_id: target_id_str,
            summary: resolved_summary,
        });
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });

        Ok(thread_to_summary(&updated, source_is_first))
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        let pending = {
            let mut state = self.runtime.lock()?;
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
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .map(|s| s.session_id.clone())
        };
        let Some(sid) = session_id else {
            return Ok(());
        };
        let driver = {
            let guard = self.runtime.acp_driver.lock().await;
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
        let cwd = parse_typed_id::<ThreadId>(thread_id)
            .and_then(|tid| self.runtime.storage.get_thread(tid).ok().flatten())
            .map(|t| self.runtime.resolve_thread_cwd(&t))
            .unwrap_or_else(|| self.runtime.cwd.clone());
        let cmd = command.to_string();
        let output = tokio::task::spawn_blocking(move || {
            grein_workspace::shell::run_shell_command_with_pty_blocking(
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
        self.runtime.emit_tui(TuiEngineEvent::ShellOutput {
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
        Ok(SessionSnapshot {
            node_count: transcript.len(),
            thread_count: threads.len(),
            threads,
        })
    }

    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
        Ok(Some(InProcessEngine::subscribe_tui_events(self)))
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        self.runtime.compute_thread_metadata()
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let deleted_thread = threads
            .iter()
            .find(|t| t.id == tid)
            .cloned()
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if threads.len() <= 1 {
            return Err(EngineError::InvalidParams(
                "cannot delete the only thread".to_string(),
            ));
        }

        let meta_before = self.runtime.compute_thread_metadata()?;
        let parent_thread_id = meta_before
            .into_iter()
            .find(|m| m.thread_id == thread_id)
            .and_then(|m| m.parent_thread_id);

        let removed_pty = self
            .runtime
            .active_pty_sessions
            .lock()
            .ok()
            .and_then(|mut map| map.remove(thread_id));
        if let Ok(mut pending) = self.runtime.pending_pty_sizes.lock() {
            pending.remove(thread_id);
        }
        if let Ok(mut exited) = self.runtime.pty_exited_threads.lock() {
            exited.remove(thread_id);
        }
        if let Some(pty_session) = removed_pty {
            let _ = tokio::task::spawn_blocking(move || drop(pty_session)).await;
        }

        let removed_session = {
            let mut state = self.runtime.lock()?;
            state.pending_thread_config.remove(thread_id);
            state.thread_turn_locks.remove(thread_id);
            state
                .active_acp_sessions
                .remove(thread_id)
                .map(|s| (s.session_id.clone(), s.driver_gen))
        };
        self.runtime.subagent_watch().clear_thread(thread_id);

        if let Some((session_id, driver_gen)) = removed_session {
            let driver_opt = {
                let guard = self.runtime.acp_driver.lock().await;
                guard
                    .as_ref()
                    .filter(|s| s.generation == driver_gen && s.driver.is_running())
                    .map(|s| Arc::clone(&s.driver))
            };
            if let Some(driver) = driver_opt {
                let _ = tokio::time::timeout(
                    CLOSE_SESSION_TIMEOUT,
                    driver.close_session(&session_id),
                )
                .await;
            }
        }

        let reparent_target = deleted_thread
            .parent_thread_id
            .as_ref()
            .filter(|pid| **pid != tid && threads.iter().any(|t| t.id == **pid));
        self.runtime
            .storage
            .reparent_child_threads(&tid, reparent_target)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .delete_thread(tid)
            .map_err(storage_err)?;
        let remaining = self.runtime.storage.list_threads().map_err(storage_err)?;
        if let Ok(mut state) = self.runtime.lock() {
            state.last_node = remaining.first().map(|t| t.active_leaf_id);
        }

        let next_active_thread_id = parent_thread_id
            .filter(|pid| remaining.iter().any(|t| t.id.0.to_string() == *pid))
            .or_else(|| remaining.first().map(|t| t.id.0.to_string()))
            .unwrap_or_default();

        let meta_after = self.runtime.compute_thread_metadata()?;
        self.runtime.emit_tui(TuiEngineEvent::ThreadDeleted {
            thread_id: thread_id.to_string(),
            next_active_thread_id: next_active_thread_id.clone(),
        });
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta: meta_after });
        Ok(next_active_thread_id)
    }

    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        let state = self.runtime.lock()?;
        Ok(state
            .active_acp_sessions
            .iter()
            .map(|(thread_id, s)| AgentMetaEntry {
                thread_id: thread_id.clone(),
                model: s.model.clone(),
                mode: s.mode.clone(),
                config_options: s.config_options.clone(),
                available_commands: s.available_commands.clone(),
            })
            .collect())
    }

    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind == ThreadKind::Terminal {
            return Ok(AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            });
        }

        let Some(cfg) = self.runtime.acp_config.clone() else {
            let state = self.runtime.lock()?;
            return Ok(match state.active_acp_sessions.get(thread_id) {
                Some(s) => AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: s.config_options.clone(),
                    available_commands: s.available_commands.clone(),
                },
                None => AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: Vec::new(),
                    available_commands: Vec::new(),
                },
            });
        };

        if let Some((_, current_gen)) = self.runtime.current_driver_if_running().await {
            let state = self.runtime.lock()?;
            if let Some(s) = state.active_acp_sessions.get(thread_id) {
                if s.driver_gen == current_gen && !s.cmd_tx.is_closed() {
                    return Ok(AgentOptionsSnapshot {
                        thread_id: thread_id.to_string(),
                        config_options: s.config_options.clone(),
                        available_commands: s.available_commands.clone(),
                    });
                }
            }
        }

        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let fresh_thread = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        self.runtime
            .ensure_acp_session_for_thread(
                &cfg,
                tid,
                thread_id,
                fresh_thread.active_leaf_id,
                None,
                true,
            )
            .await?;

        let state = self.runtime.lock()?;
        Ok(match state.active_acp_sessions.get(thread_id) {
            Some(s) => AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: s.config_options.clone(),
                available_commands: s.available_commands.clone(),
            },
            None => AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            },
        })
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        if config_id.trim().is_empty() || value.trim().is_empty() {
            return Err(EngineError::InvalidParams(
                "config_id and value must not be empty".to_string(),
            ));
        }

        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot set agent config on a terminal thread".to_string(),
            ));
        }

        let Some(cfg) = self.runtime.acp_config.clone() else {
            return Err(EngineError::Transport(
                "no ACP agent configured; install agy or set GREIN_ACP_BIN".to_string(),
            ));
        };

        let existing = if let Some((driver, current_gen)) =
            self.runtime.current_driver_if_running().await
        {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .filter(|s| s.driver_gen == current_gen && !s.cmd_tx.is_closed())
                .map(|s| {
                    let opt = s.config_options.iter().find(|o| o.id == config_id);
                    (
                        driver,
                        s.session_id.clone(),
                        opt.is_some_and(|o| o.category.as_deref() == Some("model")),
                        opt.is_some_and(|o| o.category.as_deref() == Some("mode")),
                    )
                })
        } else {
            None
        };

        let (driver, session_id, is_model_hint, is_mode_hint) = match existing {
            Some(found) => found,
            None => {
                let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
                let fresh_thread = self
                    .runtime
                    .storage
                    .get_thread(tid)
                    .map_err(storage_err)?
                    .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
                let (driver, handle) = self
                    .runtime
                    .ensure_acp_session_for_thread(
                        &cfg,
                        tid,
                        thread_id,
                        fresh_thread.active_leaf_id,
                        None,
                        true,
                    )
                    .await?;
                let state = self.runtime.lock()?;
                let opt = state
                    .active_acp_sessions
                    .get(thread_id)
                    .and_then(|s| s.config_options.iter().find(|o| o.id == config_id));
                (
                    driver,
                    handle.session_id,
                    opt.is_some_and(|o| o.category.as_deref() == Some("model")),
                    opt.is_some_and(|o| o.category.as_deref() == Some("mode")),
                )
            }
        };

        let target = ConfigTarget {
            config_id,
            value,
            is_model_hint,
            is_mode_hint,
        };
        let updated_opts =
            EngineRuntime::apply_config_to_driver(&driver, &session_id, &target).await?;

        let (snapshot, model, mode) = self.runtime.apply_config_update_to_state(
            thread_id,
            &session_id,
            &target,
            updated_opts,
        )?;

        {
            let mut state = self.runtime.lock()?;
            state
                .pending_thread_config
                .entry(thread_id.to_string())
                .or_default()
                .insert(config_id.to_string(), value.to_string());
            if set_default {
                state
                    .default_config
                    .insert(config_id.to_string(), value.to_string());
            }
        }

        self.runtime.emit_tui(TuiEngineEvent::AgentConfigUpdated {
            thread_id: thread_id.to_string(),
            config_options: snapshot.config_options.clone(),
        });
        self.runtime.emit_tui(TuiEngineEvent::AgentMetaUpdated {
            thread_id: thread_id.to_string(),
            model,
            mode,
        });

        Ok(snapshot)
    }

    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        let parent_thread = match parent_thread_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                let parent_tid: ThreadId = parse_typed_id(raw)
                    .ok_or_else(|| EngineError::NotFound(format!("thread {raw}")))?;
                let thread = self
                    .runtime
                    .storage
                    .get_thread(parent_tid)
                    .map_err(storage_err)?
                    .ok_or_else(|| EngineError::NotFound(format!("thread {raw}")))?;
                Some(thread)
            }
            None => None,
        };

        let resolved_cwd = self
            .runtime
            .resolve_terminal_cwd(cwd, parent_thread.as_ref())?;
        let resolved_title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("Terminal")
            .to_string();

        let (parent_node_id, parent_tid_opt) = match parent_thread.as_ref() {
            Some(parent) => (parent.active_leaf_id, Some(parent.id)),
            None => {
                let root_node = ConversationNode::new_root(AuthorRole::System, "");
                self.runtime
                    .storage
                    .insert_node(&root_node)
                    .map_err(storage_err)?;
                (root_node.id, None)
            }
        };

        let anchor_id = AnchorId::new();
        let anchor = Anchor {
            level: 1,
            title: resolved_title.clone(),
            slug: slugify_anchor(&resolved_title),
            line_number: 1,
        };
        self.runtime
            .storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;

        let tid = self
            .runtime
            .storage
            .create_thread_with_kind(
                parent_node_id,
                anchor_id,
                Some(&resolved_title),
                parent_tid_opt,
                ThreadKind::Terminal,
                Some(&resolved_cwd),
            )
            .map_err(storage_err)?;
        let tid_str = tid.0.to_string();

        if let Err(err) = self
            .runtime
            .spawn_pty_for_thread(&tid_str, PathBuf::from(&resolved_cwd), rows, cols)
            .await
        {
            if let Ok(mut pending) = self.runtime.pending_pty_sizes.lock() {
                pending.remove(&tid_str);
            }
            let _ = self.runtime.storage.delete_thread(tid);
            return Err(err);
        }

        let created = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("created terminal thread missing".to_string()))?;
        let summary = thread_to_summary(&created, false);
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        self.runtime.emit_tui(TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: summary.thread_id.clone(),
                title: summary.title.clone(),
                turns: Vec::new(),
            },
        });
        Ok(summary)
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;
        let (session, _) = self
            .runtime
            .ensure_pty_session(thread_id, &thread, 24, 80)
            .await?;
        tokio::task::spawn_blocking(move || session.write_bytes(&bytes))
            .await
            .map_err(|e| EngineError::Transport(format!("PTY write task panicked: {e}")))?
            .map_err(|e| EngineError::Transport(format!("PTY write failed: {e}")))
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        let _ = self.runtime.require_terminal_thread(thread_id)?;
        if rows == 0 || cols == 0 {
            return Ok(());
        }
        let target_rows = rows.max(4);
        let target_cols = cols.max(10);
        let session = {
            let sessions = self
                .runtime
                .active_pty_sessions
                .lock()
                .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?;
            if let Some(existing) = sessions.get(thread_id).cloned() {
                Some(existing)
            } else {
                self.runtime
                    .pending_pty_sizes
                    .lock()
                    .map_err(|_| {
                        EngineError::Storage("pending pty sizes map poisoned".to_string())
                    })?
                    .insert(thread_id.to_string(), (target_rows, target_cols));
                None
            }
        };
        let Some(session) = session else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || session.resize(target_rows, target_cols))
            .await
            .map_err(|e| EngineError::Transport(format!("PTY resize task panicked: {e}")))?
            .map_err(|e| EngineError::Transport(format!("PTY resize failed: {e}")))
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;
        let (session, used_pending) = self
            .runtime
            .ensure_pty_session(thread_id, &thread, rows, cols)
            .await?;
        let snap = session.snapshot();
        if !used_pending && rows > 0 && cols > 0 {
            let target_rows = rows.max(4);
            let target_cols = cols.max(10);
            if snap.rows != target_rows || snap.cols != target_cols {
                let s_clone = Arc::clone(&session);
                tokio::task::spawn_blocking(move || s_clone.resize(target_rows, target_cols))
                    .await
                    .map_err(|e| EngineError::Transport(format!("PTY resize task panicked: {e}")))?
                    .map_err(|e| EngineError::Transport(format!("PTY resize failed: {e}")))?;
                return Ok(pty_snapshot_to_tui(session.snapshot()));
            }
        }
        Ok(pty_snapshot_to_tui(snap))
    }

    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;
        let (session, _) = self
            .runtime
            .ensure_pty_session(thread_id, &thread, 24, 80)
            .await?;
        session.scroll_by(delta);
        let snap = pty_snapshot_to_tui(session.snapshot());
        self.runtime.emit_tui(TuiEngineEvent::PtyScreenUpdated {
            thread_id: thread_id.to_string(),
            snapshot: snap.clone(),
        });
        Ok(snap)
    }

    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        let (tid, _) = self.runtime.require_terminal_thread(thread_id)?;
        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;

        let removed_pty = self
            .runtime
            .active_pty_sessions
            .lock()
            .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?
            .remove(thread_id);
        if let Ok(mut pending) = self.runtime.pending_pty_sizes.lock() {
            pending.remove(thread_id);
        }
        if let Ok(mut exited) = self.runtime.pty_exited_threads.lock() {
            exited.remove(thread_id);
        }
        if let Some(pty_session) = removed_pty {
            let _ = tokio::task::spawn_blocking(move || drop(pty_session)).await;
        }

        let target_rows = if rows == 0 { 24 } else { rows.max(4) };
        let target_cols = if cols == 0 { 80 } else { cols.max(10) };
        let cwd = self.runtime.resolve_thread_cwd(&thread);
        let session = self
            .runtime
            .spawn_pty_for_thread(thread_id, cwd, target_rows, target_cols)
            .await?;

        let _ = self
            .runtime
            .storage
            .update_thread_status(tid, ThreadStatus::Idle);
        let snap = pty_snapshot_to_tui(session.snapshot());
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadStatusChanged {
                thread_id: thread_id.to_string(),
                status: "idle".to_string(),
            });
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        self.runtime.emit_tui(TuiEngineEvent::PtyScreenUpdated {
            thread_id: thread_id.to_string(),
            snapshot: snap.clone(),
        });
        Ok(snap)
    }

    async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let initial = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if initial.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot allocate a git worktree for a terminal thread".to_string(),
            ));
        }

        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let thread = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let repo = GitRepository::discover(&self.runtime.cwd).map_err(|err| match err {
            GitError::NotFound => EngineError::InvalidParams(format!(
                "workspace {} is not inside a git repository",
                self.runtime.cwd.display()
            )),
            GitError::CommandFailed(msg) => EngineError::Transport(msg),
        })?;
        let expected_dir = repo.root().join(".grein").join("worktrees").join(thread_id);

        let worktree_path = if expected_dir.is_dir() && expected_dir.join(".git").exists() {
            expected_dir
        } else {
            let tid_owned = thread_id.to_string();
            tokio::task::spawn_blocking(move || {
                let allocator = WorktreeAllocator::new(repo);
                let mut session = allocator.open_session(WorkspaceThreadId::from(tid_owned))?;
                session.transition_to_isolated_worktree()?;
                Ok::<PathBuf, grein_workspace::allocator::WorkspaceError>(
                    session.working_directory().to_path_buf(),
                )
            })
            .await
            .map_err(|e| EngineError::Transport(format!("worktree task panicked: {e}")))?
            .map_err(|e| EngineError::Transport(format!("failed to create worktree: {e}")))?
        };

        let canonical = std::fs::canonicalize(&worktree_path).map_err(|e| {
            EngineError::Transport(format!(
                "failed to canonicalize worktree {}: {e}",
                worktree_path.display()
            ))
        })?;
        let path_str = canonical
            .to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| EngineError::InvalidParams("worktree path is not valid UTF-8".to_string()))?;

        let cwd_changed = thread.cwd.as_deref() != Some(path_str.as_str());
        if cwd_changed {
            self.runtime
                .storage
                .update_thread_cwd(&tid, Some(&path_str))
                .map_err(storage_err)?;
            self.runtime
                .storage
                .update_thread_acp_session_id(&tid, None)
                .map_err(storage_err)?;
            let old_session = {
                let mut state = self.runtime.lock()?;
                state
                    .active_acp_sessions
                    .remove(thread_id)
                    .map(|s| (s.session_id.clone(), s.driver_gen))
            };
            if let Some((session_id, driver_gen)) = old_session {
                let driver_opt = {
                    let guard = self.runtime.acp_driver.lock().await;
                    guard
                        .as_ref()
                        .filter(|s| s.generation == driver_gen && s.driver.is_running())
                        .map(|s| Arc::clone(&s.driver))
                };
                if let Some(driver) = driver_opt {
                    let _ = tokio::time::timeout(
                        CLOSE_SESSION_TIMEOUT,
                        driver.close_session(&session_id),
                    )
                    .await;
                }
            }
        }

        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        Ok(path_str)
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
        "session.create_root_thread" => {
            let title = params.get("title").and_then(Value::as_str);
            to_json(&engine.create_root_thread(title).await?)?
        }
        "session.rename_thread" => {
            let thread_id = required_str(params, "thread_id")?;
            let title = required_str(params, "title")?;
            to_json(&engine.rename_thread(thread_id, title).await?)?
        }
        "session.list_threads" => to_json(&engine.list_threads().await?)?,
        "session.get_transcript" => {
            if let Some(tid) = params.get("thread_id").and_then(Value::as_str) {
                to_json(&engine.get_thread_transcript(tid).await?)?
            } else {
                to_json(&engine.get_transcript().await?)?
            }
        }
        "session.merge_thread" => {
            let source_id = required_str(params, "thread_id")?;
            let target_id = params
                .get("target_thread_id")
                .or_else(|| params.get("target_id"))
                .and_then(Value::as_str);
            let summary = params.get("summary").and_then(Value::as_str);
            to_json(
                &engine
                    .merge_thread_with_summary(source_id, target_id, summary)
                    .await?,
            )?
        }
        "session.thread_metadata" => to_json(&engine.thread_metadata().await?)?,
        "session.delete_thread" => {
            let thread_id = required_str(params, "thread_id")?;
            let next_active_thread_id = engine.delete_thread(thread_id).await?;
            json!({"next_active_thread_id": next_active_thread_id})
        }
        "session.agent_metadata" => to_json(&engine.agent_metadata().await?)?,
        "session.agent_options" => {
            let thread_id = required_str(params, "thread_id")?;
            to_json(&engine.agent_options(thread_id).await?)?
        }
        "session.set_agent_config" => {
            let thread_id = required_str(params, "thread_id")?;
            let config_id = required_str(params, "config_id")?;
            let value = required_str(params, "value")?;
            let set_default = params
                .get("set_default")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            to_json(
                &engine
                    .set_agent_config(thread_id, config_id, value, set_default)
                    .await?,
            )?
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
        "session.create_terminal" | "create_terminal" => {
            let parent_thread_id = params.get("parent_thread_id").and_then(Value::as_str);
            let title = params.get("title").and_then(Value::as_str);
            let cwd = params.get("cwd").and_then(Value::as_str);
            let rows = parse_opt_u16(params, "rows", 24)?;
            let cols = parse_opt_u16(params, "cols", 80)?;
            to_json(
                &engine
                    .create_terminal(parent_thread_id, title, cwd, rows, cols)
                    .await?,
            )?
        }
        "session.pty_write" | "pty_write" => {
            let thread_id = required_str(params, "thread_id")?;
            let bytes_val = params
                .get("bytes")
                .cloned()
                .ok_or_else(|| EngineError::InvalidParams("missing `bytes` param".to_string()))?;
            let bytes: Vec<u8> = serde_json::from_value(bytes_val)
                .map_err(|e| EngineError::InvalidParams(format!("invalid `bytes`: {e}")))?;
            engine.pty_write(thread_id, bytes).await?;
            json!({"ok": true})
        }
        "session.pty_resize" | "pty_resize" => {
            let thread_id = required_str(params, "thread_id")?;
            let rows = parse_req_u16(params, "rows")?;
            let cols = parse_req_u16(params, "cols")?;
            engine.pty_resize(thread_id, rows, cols).await?;
            json!({"ok": true})
        }
        "session.pty_snapshot" | "pty_snapshot" => {
            let thread_id = required_str(params, "thread_id")?;
            let rows = parse_opt_u16(params, "rows", 0)?;
            let cols = parse_opt_u16(params, "cols", 0)?;
            to_json(&engine.pty_snapshot(thread_id, rows, cols).await?)?
        }
        "session.ensure_thread_worktree" | "ensure_thread_worktree" => {
            let thread_id = required_str(params, "thread_id")?;
            let cwd = engine.ensure_thread_worktree(thread_id).await?;
            json!({"cwd": cwd})
        }
        "session.pty_scroll" | "pty_scroll" => {
            let thread_id = required_str(params, "thread_id")?;
            let delta = parse_req_isize(params, "delta")?;
            to_json(&engine.pty_scroll(thread_id, delta).await?)?
        }
        "session.pty_restart" | "pty_restart" => {
            let thread_id = required_str(params, "thread_id")?;
            let rows = parse_opt_u16(params, "rows", 24)?;
            let cols = parse_opt_u16(params, "cols", 80)?;
            to_json(&engine.pty_restart(thread_id, rows, cols).await?)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn parse_opt_u16(params: &Value, key: &str, default: u16) -> Result<u16, EngineError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| EngineError::InvalidParams(format!("invalid u16 param `{key}`"))),
    }
}

fn parse_req_u16(params: &Value, key: &str) -> Result<u16, EngineError> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| EngineError::InvalidParams(format!("missing u16 param `{key}`")))
}

fn parse_req_isize(params: &Value, key: &str) -> Result<isize, EngineError> {
    params
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| EngineError::InvalidParams(format!("missing isize param `{key}`")))
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

    fn is_stale_daemon_terminal_rpc(method: &str) -> bool {
        matches!(
            method,
            "session.create_terminal"
                | "session.pty_write"
                | "session.pty_resize"
                | "session.pty_snapshot"
                | "session.pty_scroll"
                | "session.pty_restart"
                | "session.ensure_thread_worktree"
        )
    }

    fn parse_rpc_response(method: &str, line: &str) -> Result<Value, EngineError> {
        let response: Value =
            serde_json::from_str(line.trim()).map_err(|e| EngineError::Protocol(e.to_string()))?;
        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(Value::as_i64);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown rpc error");
            if code == Some(-32601) && Self::is_stale_daemon_terminal_rpc(method) {
                return Err(EngineError::Protocol(format!(
                    "running `grein daemon` appears outdated and needs to be restarted (`{method}`: {message})"
                )));
            }
            return Err(EngineError::Protocol(message.to_string()));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| EngineError::Protocol("response missing result".into()))
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
        Self::parse_rpc_response(method, &line)
    }

    async fn call_dedicated(&self, method: &str, params: Value) -> Result<Value, EngineError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        let (read, mut writer) = stream.into_split();
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        let read_bytes = reader
            .read_line(&mut line)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        if read_bytes == 0 {
            return Err(EngineError::Transport("daemon closed connection".into()));
        }
        Self::parse_rpc_response(method, &line)
    }

    async fn call_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, EngineError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
    }

    async fn call_dedicated_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, EngineError> {
        let value = self.call_dedicated(method, params).await?;
        serde_json::from_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
    }

    pub async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        DaemonEngine::thread_metadata(self).await
    }

    pub async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        DaemonEngine::delete_thread(self, thread_id).await
    }

    pub async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        DaemonEngine::agent_options(self, thread_id).await
    }

    pub async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        DaemonEngine::set_agent_config(self, thread_id, config_id, value, set_default).await
    }

    pub async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::create_terminal(self, parent_thread_id, title, cwd, rows, cols).await
    }

    pub async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        DaemonEngine::pty_write(self, thread_id, bytes).await
    }

    pub async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        DaemonEngine::pty_resize(self, thread_id, rows, cols).await
    }

    pub async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_snapshot(self, thread_id, rows, cols).await
    }

    pub async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        DaemonEngine::ensure_thread_worktree(self, thread_id).await
    }

    pub async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_scroll(self, thread_id, delta).await
    }

    pub async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_restart(self, thread_id, rows, cols).await
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

    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(title) = title {
            params.insert("title".to_string(), json!(title));
        }
        self.call_typed("session.create_root_thread", Value::Object(params))
            .await
    }

    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        self.call_typed(
            "session.rename_thread",
            json!({"thread_id": thread_id, "title": title}),
        )
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
        self.merge_thread_with_summary(thread_id, None, None).await
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        params.insert("thread_id".to_string(), json!(source_id));
        if let Some(target_id) = target_id {
            params.insert("target_thread_id".to_string(), json!(target_id));
        }
        if let Some(summary) = summary {
            params.insert("summary".to_string(), json!(summary));
        }
        self.call_typed("session.merge_thread", Value::Object(params))
            .await
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        self.call_dedicated_typed("permissions.resolve", to_json(&decision)?)
            .await
    }

    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        self.call_dedicated("session.cancel", json!({"thread_id": thread_id}))
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
        Ok(SessionSnapshot {
            node_count: transcript.len(),
            thread_count: threads.len(),
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

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        self.call_dedicated_typed("session.thread_metadata", json!({}))
            .await
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        let val = self
            .call_dedicated("session.delete_thread", json!({"thread_id": thread_id}))
            .await?;
        val.get("next_active_thread_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing next_active_thread_id".into()))
    }

    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        self.call_typed("session.agent_metadata", json!({})).await
    }

    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        self.call_typed("session.agent_options", json!({"thread_id": thread_id}))
            .await
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        self.call_typed(
            "session.set_agent_config",
            json!({
                "thread_id": thread_id,
                "config_id": config_id,
                "value": value,
                "set_default": set_default,
            }),
        )
        .await
    }

    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(pid) = parent_thread_id {
            params.insert("parent_thread_id".to_string(), json!(pid));
        }
        if let Some(t) = title {
            params.insert("title".to_string(), json!(t));
        }
        if let Some(dir) = cwd {
            params.insert("cwd".to_string(), json!(dir));
        }
        params.insert("rows".to_string(), json!(rows));
        params.insert("cols".to_string(), json!(cols));
        self.call_dedicated_typed("session.create_terminal", Value::Object(params))
            .await
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        self.call_dedicated(
            "session.pty_write",
            json!({"thread_id": thread_id, "bytes": bytes}),
        )
        .await
        .map(|_| ())
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        self.call_dedicated(
            "session.pty_resize",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
        .map(|_| ())
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_snapshot",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
    }

    async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        let val = self
            .call_dedicated(
                "session.ensure_thread_worktree",
                json!({"thread_id": thread_id}),
            )
            .await?;
        val.get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing cwd".into()))
    }

    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_scroll",
            json!({"thread_id": thread_id, "delta": delta}),
        )
        .await
    }

    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_restart",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
    }
}

/// Unified engine: standalone in-process or remote daemon.
pub enum EngineClient {
    InProcess(Box<InProcessEngine>),
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

    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => e.create_root_thread(title).await,
            Self::Socket(e) => e.create_root_thread(title).await,
        }
    }

    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => e.rename_thread(thread_id, title).await,
            Self::Socket(e) => e.rename_thread(thread_id, title).await,
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

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => {
                e.merge_thread_with_summary(source_id, target_id, summary)
                    .await
            }
            Self::Socket(e) => {
                e.merge_thread_with_summary(source_id, target_id, summary)
                    .await
            }
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
            Self::InProcess(e) => DaemonEngine::subscribe_events(e.as_ref()).await,
            Self::Socket(e) => e.subscribe_events().await,
        }
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::thread_metadata(e.as_ref()).await,
            Self::Socket(e) => e.thread_metadata().await,
        }
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::delete_thread(e.as_ref(), thread_id).await,
            Self::Socket(e) => e.delete_thread(thread_id).await,
        }
    }

    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::agent_metadata(e.as_ref()).await,
            Self::Socket(e) => DaemonEngine::agent_metadata(e).await,
        }
    }

    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::agent_options(e.as_ref(), thread_id).await,
            Self::Socket(e) => DaemonEngine::agent_options(e, thread_id).await,
        }
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        match self {
            Self::InProcess(e) => {
                DaemonEngine::set_agent_config(e.as_ref(), thread_id, config_id, value, set_default)
                    .await
            }
            Self::Socket(e) => {
                DaemonEngine::set_agent_config(e, thread_id, config_id, value, set_default).await
            }
        }
    }

    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => {
                DaemonEngine::create_terminal(e.as_ref(), parent_thread_id, title, cwd, rows, cols)
                    .await
            }
            Self::Socket(e) => {
                DaemonEngine::create_terminal(e, parent_thread_id, title, cwd, rows, cols).await
            }
        }
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::pty_write(e.as_ref(), thread_id, bytes).await,
            Self::Socket(e) => DaemonEngine::pty_write(e, thread_id, bytes).await,
        }
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::pty_resize(e.as_ref(), thread_id, rows, cols).await,
            Self::Socket(e) => DaemonEngine::pty_resize(e, thread_id, rows, cols).await,
        }
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::pty_snapshot(e.as_ref(), thread_id, rows, cols).await,
            Self::Socket(e) => DaemonEngine::pty_snapshot(e, thread_id, rows, cols).await,
        }
    }

    async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        match self {
            Self::InProcess(e) => {
                DaemonEngine::ensure_thread_worktree(e.as_ref(), thread_id).await
            }
            Self::Socket(e) => DaemonEngine::ensure_thread_worktree(e, thread_id).await,
        }
    }

    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::pty_scroll(e.as_ref(), thread_id, delta).await,
            Self::Socket(e) => DaemonEngine::pty_scroll(e, thread_id, delta).await,
        }
    }

    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        match self {
            Self::InProcess(e) => {
                DaemonEngine::pty_restart(e.as_ref(), thread_id, rows, cols).await
            }
            Self::Socket(e) => DaemonEngine::pty_restart(e, thread_id, rows, cols).await,
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
    async fn test_sqlite_hydration_across_reopen_preserves_threads_and_ignores_legacy_blackboard() {
        use grein_core::blackboard::{BlackboardEntry as LegacyBbEntry, BlackboardEntryKind};

        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");

        // Pre-populate legacy blackboard_entries rows before opening InProcessEngine.
        {
            let legacy_storage = SqliteStorage::open(&db_path).expect("open legacy sqlite");
            legacy_storage
                .insert_blackboard_entry(&LegacyBbEntry::new(
                    "legacy_goal",
                    "LEGACY_BLACKBOARD_VAL",
                    BlackboardEntryKind::Decision,
                ))
                .expect("insert legacy row");
        }

        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open legacy db");
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

        // Joining a root thread must return EngineError::InvalidParams and leave merged == false.
        let join_root_err = engine
            .merge_thread_with_summary(&main.thread_id, None, Some("should fail"))
            .await
            .expect_err("joining a root thread must fail");
        assert!(
            matches!(join_root_err, EngineError::InvalidParams(_)),
            "expected InvalidParams, got: {join_root_err:?}"
        );
        let main_after_err = engine
            .list_threads()
            .await
            .expect("threads")
            .into_iter()
            .find(|t| t.thread_id == main.thread_id)
            .expect("main exists");
        assert!(!main_after_err.merged);

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
            TuiEngineEvent::ThreadCreated { thread } if thread.id == forked.thread_id && thread.title == "feat-auth"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, .. }
                if thread_id == &forked.thread_id && target_thread_id == &main.thread_id
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

        let main_turns = reopened
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("main turns");
        assert!(main_turns
            .iter()
            .any(|t| t.content.contains("[Decision Landmark") && t.content.contains("feat-auth")));

        let snap = reopened.snapshot().await.expect("reopened snapshot");
        assert_eq!(snap.thread_count, 2);
        assert_eq!(snap.node_count, 1);
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
    async fn test_live_acp_streaming_resume_permissions_and_cancel() {
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

        // Turn 2 reuses sess-1 cleanly without whiteboard delta prefix.
        timeout(
            Duration::from_secs(5),
            engine.send_turn_for_thread(&main.thread_id, "second prompt"),
        )
        .await
        .expect("turn2 timeout")
        .expect("turn2 ok");

        // Turn 3 reuses sess-1 cleanly.
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
            .unwrap_or("");
        assert!(!sys_append_1.contains("# Shared Blackboard Working Memory"));
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
        assert_eq!(prompts[1], "second prompt");
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

        let bb_list_resp = dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":3,"method":"blackboard.list","params":{}}"#,
        )
        .await;
        assert_eq!(
            serde_json::from_str::<Value>(&bb_list_resp).unwrap()["error"]["code"],
            -32601
        );

        let upsert_resp = dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":4,"method":"blackboard.upsert","params":{"key":"k","value":"v"}}"#,
        )
        .await;
        assert_eq!(
            serde_json::from_str::<Value>(&upsert_resp).unwrap()["error"]["code"],
            -32601
        );

        let del_resp = dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":5,"method":"blackboard.delete","params":{"key":"k"}}"#,
        )
        .await;
        assert_eq!(
            serde_json::from_str::<Value>(&del_resp).unwrap()["error"]["code"],
            -32601
        );
    }

    #[tokio::test]
    async fn test_acp_deny_permission_and_crash_recovery() {
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

        engine
            .send_turn_for_thread(&main.thread_id, "follow-up after recovery")
            .await
            .expect("follow-up");

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
        assert!(!text.contains("[Shared Whiteboard Cleared]"));
        assert_eq!(text, "follow-up after recovery");
    }

    #[tokio::test]
    async fn test_thread_metadata_inference_branch_local_turns_and_offline_root() {
        // 1. Offline-root edge case: send_turn before ensure_main_thread.
        let engine = InProcessEngine::in_memory().expect("in_memory");
        engine
            .send_turn("offline root question")
            .await
            .expect("offline send_turn");
        let main = engine.ensure_main_thread().await.expect("ensure main");

        let main_turns = engine
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("main transcript");
        assert_eq!(main_turns.len(), 1);
        assert_eq!(main_turns[0].content, "offline root question");

        let meta_initial = engine.thread_metadata().await.expect("meta");
        assert_eq!(meta_initial.len(), 1);
        assert_eq!(meta_initial[0].parent_thread_id, None);
        assert_eq!(meta_initial[0].inherited_turn_count, 0);

        let offline_fork = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("offline-fork"))
            .await
            .expect("fork from offline main");
        let fork_turns = engine
            .get_thread_transcript(&offline_fork.thread_id)
            .await
            .expect("offline fork turns");
        assert!(
            fork_turns.is_empty(),
            "forked thread must not duplicate offline root node"
        );
        let meta_with_fork = engine.thread_metadata().await.expect("meta");
        let fork_meta = meta_with_fork
            .iter()
            .find(|m| m.thread_id == offline_fork.thread_id)
            .expect("fork meta");
        assert_eq!(
            fork_meta.parent_thread_id.as_deref(),
            Some(main.thread_id.as_str())
        );
        assert_eq!(fork_meta.inherited_turn_count, 1);

        // 2. Multi-level tree + intermediate parent deletion.
        let engine2 = InProcessEngine::in_memory().expect("in_memory 2");
        let root = engine2.ensure_main_thread().await.expect("main");
        let root_tid: ThreadId = parse_typed_id(&root.thread_id).unwrap();
        let root_leaf = engine2
            .runtime
            .storage
            .get_thread(root_tid)
            .unwrap()
            .unwrap()
            .active_leaf_id;
        let u1 = NodeId::new();
        engine2
            .runtime
            .storage
            .insert_node(&ConversationNode::new(
                u1,
                Some(root_leaf),
                AuthorRole::User,
                "u1".into(),
            ))
            .unwrap();
        let a1 = NodeId::new();
        engine2
            .runtime
            .storage
            .insert_node(&ConversationNode::new(
                a1,
                Some(u1),
                AuthorRole::Assistant,
                "a1".into(),
            ))
            .unwrap();
        engine2
            .runtime
            .storage
            .update_thread_leaf(root_tid, a1)
            .unwrap();

        let child = engine2
            .fork_thread_with_title(Some(&root.thread_id), Some("child"))
            .await
            .expect("fork child");
        let child_tid: ThreadId = parse_typed_id(&child.thread_id).unwrap();
        let cu1 = NodeId::new();
        engine2
            .runtime
            .storage
            .insert_node(&ConversationNode::new(
                cu1,
                Some(a1),
                AuthorRole::User,
                "cu1".into(),
            ))
            .unwrap();
        let ca1 = NodeId::new();
        engine2
            .runtime
            .storage
            .insert_node(&ConversationNode::new(
                ca1,
                Some(cu1),
                AuthorRole::Assistant,
                "ca1".into(),
            ))
            .unwrap();
        engine2
            .runtime
            .storage
            .update_thread_leaf(child_tid, ca1)
            .unwrap();

        let child_turns = engine2
            .get_thread_transcript(&child.thread_id)
            .await
            .expect("child turns");
        assert_eq!(child_turns.len(), 2);
        assert_eq!(child_turns[0].content, "cu1");
        assert_eq!(child_turns[1].content, "ca1");

        let grandchild = engine2
            .fork_thread_with_title(Some(&child.thread_id), Some("grandchild"))
            .await
            .expect("fork grandchild");
        let gc_tid: ThreadId = parse_typed_id(&grandchild.thread_id).unwrap();
        let gu1 = NodeId::new();
        engine2
            .runtime
            .storage
            .insert_node(&ConversationNode::new(
                gu1,
                Some(ca1),
                AuthorRole::User,
                "gu1".into(),
            ))
            .unwrap();
        engine2
            .runtime
            .storage
            .update_thread_leaf(gc_tid, gu1)
            .unwrap();

        let gc_turns = engine2
            .get_thread_transcript(&grandchild.thread_id)
            .await
            .expect("gc turns");
        assert_eq!(gc_turns.len(), 1);
        assert_eq!(gc_turns[0].content, "gu1");

        let meta_3 = engine2.thread_metadata().await.expect("meta 3");
        let gc_meta_before = meta_3
            .iter()
            .find(|m| m.thread_id == grandchild.thread_id)
            .unwrap();
        assert_eq!(
            gc_meta_before.parent_thread_id.as_deref(),
            Some(child.thread_id.as_str())
        );
        assert_eq!(gc_meta_before.inherited_turn_count, 4);

        // Delete intermediate parent `child` -> `grandchild` walks up to `root`.
        let next_active = engine2
            .delete_thread(&child.thread_id)
            .await
            .expect("delete child");
        assert_eq!(next_active, root.thread_id);

        let meta_after = engine2.thread_metadata().await.expect("meta after");
        assert_eq!(meta_after.len(), 2);
        let gc_meta_after = meta_after
            .iter()
            .find(|m| m.thread_id == grandchild.thread_id)
            .unwrap();
        assert_eq!(
            gc_meta_after.parent_thread_id.as_deref(),
            Some(root.thread_id.as_str())
        );
        assert_eq!(gc_meta_after.inherited_turn_count, 4);
    }

    #[tokio::test]
    async fn test_delete_thread_sole_guard_and_closes_acp_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let capture_path = temp.path().join("close_capture.jsonl");

        let mock_script = r#"
sess_counter=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            sess_counter=$((sess_counter+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s"}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/prompt"'*)
            case "$line" in
                *'"sessionId":"sess-1"'*) sid="sess-1" ;;
                *) sid="sess-2" ;;
            esac
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ok"}}}}\n' "$sid"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
        *'"method":"session/close"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
            ;;
    esac
done
"#;
        let wrapper = format!(
            "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
            capture_path.to_string_lossy(),
            mock_script
        );
        let script_path = write_executable_script(temp.path(), "mock_close.sh", &wrapper);
        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };

        let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open");
        engine.set_test_subagent_brain(temp.path().join("missing-brain"));
        let main = engine.ensure_main_thread().await.expect("main");

        // Sole-thread guard
        let sole_err = engine
            .delete_thread(&main.thread_id)
            .await
            .expect_err("must reject deleting only thread");
        assert!(sole_err.to_string().contains("only thread"));

        engine
            .send_turn_for_thread(&main.thread_id, "hi")
            .await
            .expect("main turn");
        let fork = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("temp-branch"))
            .await
            .expect("fork");
        engine
            .send_turn_for_thread(&fork.thread_id, "fork turn")
            .await
            .expect("fork turn");

        let mut rx = engine.subscribe_tui_events();
        let next = engine
            .delete_thread(&fork.thread_id)
            .await
            .expect("delete fork");
        assert_eq!(next, main.thread_id);

        let mut evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            evs.push(ev);
        }
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadDeleted { thread_id, next_active_thread_id }
                if thread_id == &fork.thread_id && next_active_thread_id == &main.thread_id
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMetadataUpdated { meta } if meta.len() == 1
        )));

        let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
            .expect("capture")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        assert!(frames.iter().any(|f| {
            f.get("method").and_then(Value::as_str) == Some("session/close")
                && f["params"]["sessionId"] == "sess-2"
        }));

        // Sole-thread guard remains enforced.
        assert!(engine.delete_thread(&main.thread_id).await.is_err());
    }

    #[tokio::test]
    async fn test_persistent_acp_unsolicited_wakeups_subagents_and_agent_meta() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");

        let mock_script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-bg","models":{"currentModelId":"gemini-2.5-pro"},"modes":{"currentModeId":"code"}}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            if printf '%s' "$line" | grep -q "launch-sub"; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call","toolCallId":"sa-1","name":"invoke_subagent","title":"Researcher","args":{"Role":"Codebase Researcher","TypeName":"research"},"status":"pending"}}}\n'
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call_update","toolCallId":"sa-1","name":"invoke_subagent","status":"completed","rawOutput":{"output":"spawned"}}}}\n'
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Launched"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
                (
                    sleep 0.08
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Wakeup 1 result"}}}}\n'
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"usage_update","used":50,"size":1000}}}\n'
                ) &
            elif printf '%s' "$line" | grep -q "quiet-wake"; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Ack"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
                (
                    sleep 0.06
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Wakeup quiet"}}}}\n'
                ) &
            else
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call","toolCallId":"sa-fail","name":"invoke_subagent","title":"Failing","args":{"Role":"Fail Worker","TypeName":"research"},"status":"pending"}}}\n'
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call_update","toolCallId":"sa-fail","name":"invoke_subagent","status":"failed","rawOutput":{"output":"crashed"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#;
        let script_path = write_executable_script(temp.path(), "mock_bg_acp.sh", mock_script);
        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };

        let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open");
        engine.set_test_subagent_brain(temp.path().join("missing-brain"));
        engine.set_test_quiet_timeout(Duration::from_millis(90));
        let main = engine.ensure_main_thread().await.expect("main");
        let mut rx = engine.subscribe_tui_events();

        // Turn 1: launches subagent, completes turn, then unsolicited UsageUpdate wakeup arrives.
        engine
            .send_turn_for_thread(&main.thread_id, "launch-sub")
            .await
            .expect("turn 1");

        // Wait until unsolicited wakeup completes & subagent auto-removes (50ms test removal delay).
        let mut evs = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Some(ev)) => {
                    let removed = matches!(ev, TuiEngineEvent::SubagentRemoved { .. });
                    evs.push(ev);
                    if removed {
                        break;
                    }
                }
                _ => break,
            }
        }

        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::AgentMetaUpdated { thread_id, model, mode }
                if thread_id == &main.thread_id
                    && model.as_deref() == Some("gemini-2.5-pro")
                    && mode.as_deref() == Some("code")
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ToolActivityUpdated { tool_name: Some(name), .. } if name == "invoke_subagent"
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ToolActivityUpdated { tool_name: None, .. }
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentUpdated { subagent, .. }
                if subagent.id == "sa-1"
                    && subagent.role == "Codebase Researcher"
                    && subagent.agent_type == "research"
                    && subagent.status == SubagentStatus::Running
                    && subagent.activity.as_deref() == Some("background active")
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentUpdated { subagent, .. }
                if subagent.id == "sa-1" && subagent.status == SubagentStatus::Completed
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentRemoved { subagent_id, .. } if subagent_id == "sa-1"
        )));

        let transcript1 = engine
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("transcript1");
        assert_eq!(
            transcript1.last().map(|t| t.content.as_str()),
            Some("Wakeup 1 result")
        );

        // Turn 2: unsolicited wakeup without usage_update -> finalized by quiet timer.
        engine
            .send_turn_for_thread(&main.thread_id, "quiet-wake")
            .await
            .expect("turn 2");

        let mut saw_quiet_completed = false;
        let deadline2 = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline2 {
            match timeout(Duration::from_millis(250), rx.recv()).await {
                Ok(Some(TuiEngineEvent::TurnCompleted { .. })) => {
                    let t = engine
                        .get_thread_transcript(&main.thread_id)
                        .await
                        .expect("t2");
                    if t.last().is_some_and(|m| m.content == "Wakeup quiet") {
                        saw_quiet_completed = true;
                        break;
                    }
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert!(saw_quiet_completed, "quiet timer must finalize unsolicited wakeup");

        // Turn 3: failed invoke_subagent tool_call_update -> Failed & removed.
        engine
            .send_turn_for_thread(&main.thread_id, "fail-sub")
            .await
            .expect("turn 3");
        let mut fail_evs = Vec::new();
        while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
            let removed = matches!(
                ev,
                TuiEngineEvent::SubagentRemoved { ref subagent_id, .. } if subagent_id == "sa-fail"
            );
            fail_evs.push(ev);
            if removed {
                break;
            }
        }
        assert!(fail_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentUpdated { subagent, .. }
                if subagent.id == "sa-fail" && subagent.status == SubagentStatus::Failed
        )));
    }

    #[tokio::test]
    async fn test_empty_branch_fork_lineage_multi_root_rename_and_delete_across_reopen() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("lineage.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open");
        let main = engine.ensure_main_thread().await.expect("main");

        // Rename Main and verify empty title is rejected.
        assert!(engine.rename_thread(&main.thread_id, "   ").await.is_err());
        let renamed_main = engine
            .rename_thread(&main.thread_id, "Primary Root")
            .await
            .expect("rename main");
        assert_eq!(renamed_main.title, "Primary Root");

        // 3-level empty-branch fork: Primary Root -> Branch 5 (0 turns) -> Branch 6 (0 turns).
        let b5 = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("Branch 5"))
            .await
            .expect("fork b5");
        let b6 = engine
            .fork_thread_with_title(Some(&b5.thread_id), Some("Branch 6"))
            .await
            .expect("fork b6");

        // Create second root and rename it.
        let root2 = engine.create_root_thread(None).await.expect("create root2");
        assert_eq!(root2.title, "Root 2");
        let renamed_root2 = engine
            .rename_thread(&root2.thread_id, "Secondary Root")
            .await
            .expect("rename root2");
        assert_eq!(renamed_root2.title, "Secondary Root");
        let root2_child = engine
            .fork_thread_with_title(Some(&root2.thread_id), Some("Root2 Child"))
            .await
            .expect("fork root2 child");

        drop(engine);
        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        let ensured = reopened.ensure_main_thread().await.expect("ensure_main_thread");
        assert_eq!(ensured.title, "Primary Root");

        let threads = reopened.list_threads().await.expect("list");
        assert_eq!(
            threads
                .iter()
                .find(|t| t.thread_id == root2.thread_id)
                .unwrap()
                .title,
            "Secondary Root"
        );

        let meta = reopened.thread_metadata().await.expect("meta");
        let m_main = meta.iter().find(|m| m.thread_id == main.thread_id).unwrap();
        let m_b5 = meta.iter().find(|m| m.thread_id == b5.thread_id).unwrap();
        let m_b6 = meta.iter().find(|m| m.thread_id == b6.thread_id).unwrap();
        let m_root2 = meta.iter().find(|m| m.thread_id == root2.thread_id).unwrap();
        let m_r2c = meta
            .iter()
            .find(|m| m.thread_id == root2_child.thread_id)
            .unwrap();

        assert_eq!(m_main.parent_thread_id, None);
        assert_eq!(m_b5.parent_thread_id.as_deref(), Some(main.thread_id.as_str()));
        assert_eq!(m_b6.parent_thread_id.as_deref(), Some(b5.thread_id.as_str()));
        assert_eq!(m_root2.parent_thread_id, None);
        assert_eq!(m_root2.inherited_turn_count, 0);
        assert_eq!(
            m_r2c.parent_thread_id.as_deref(),
            Some(root2.thread_id.as_str())
        );

        // Deleting intermediate parent b5 reparents b6 to main.
        reopened.delete_thread(&b5.thread_id).await.expect("delete b5");
        let meta_after_mid = reopened.thread_metadata().await.expect("meta mid");
        let m_b6_after = meta_after_mid
            .iter()
            .find(|m| m.thread_id == b6.thread_id)
            .unwrap();
        assert_eq!(
            m_b6_after.parent_thread_id.as_deref(),
            Some(main.thread_id.as_str())
        );

        // Deleting root2 promotes root2_child to root cleanly.
        reopened
            .delete_thread(&root2.thread_id)
            .await
            .expect("delete root2");
        let meta_after_root2 = reopened.thread_metadata().await.expect("meta root2");
        let m_r2c_after = meta_after_root2
            .iter()
            .find(|m| m.thread_id == root2_child.thread_id)
            .unwrap();
        assert_eq!(m_r2c_after.parent_thread_id, None);

        // Deleting Primary Root does NOT recreate Main and promotes b6 to root.
        reopened
            .delete_thread(&main.thread_id)
            .await
            .expect("delete main");
        let remaining = reopened.list_threads().await.expect("remaining");
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.iter().any(|t| t.title == "Main" || t.title == "Primary Root"));
        let meta_final = reopened.thread_metadata().await.expect("meta final");
        assert!(meta_final.iter().all(|m| m.parent_thread_id.is_none()));
    }

    #[tokio::test]
    async fn test_legacy_null_parent_thread_id_infers_nesting_under_main_and_other_roots() {
        let storage = SqliteStorage::in_memory().expect("sqlite");
        let r1 = ConversationNode::new_root(AuthorRole::System, "");
        storage.insert_node(&r1).unwrap();
        let main_tid = storage
            .create_thread_with_title(r1.id, AnchorId::new(), "Main")
            .unwrap();
        let u1 = ConversationNode::new(NodeId::new(), Some(r1.id), AuthorRole::User, "hi".into());
        storage.insert_node(&u1).unwrap();
        storage.update_thread_leaf(main_tid, u1.id).unwrap();

        let main_child_tid = storage
            .create_thread_with_title(u1.id, AnchorId::new(), "Legacy Main Child")
            .unwrap();

        // Second root at idx > 0 with 0 turns and a legacy child with parent_thread_id = None.
        let r2 = ConversationNode::new_root(AuthorRole::System, "");
        storage.insert_node(&r2).unwrap();
        let root2_tid = storage
            .create_thread_with_title(r2.id, AnchorId::new(), "Root 2")
            .unwrap();
        let r2_child_tid = storage
            .create_thread_with_title(r2.id, AnchorId::new(), "Legacy Root2 Child")
            .unwrap();

        let engine = InProcessEngine::from_storage(storage).expect("engine");
        let meta = engine.thread_metadata().await.expect("meta");
        assert_eq!(meta.len(), 4);

        let m_main = meta
            .iter()
            .find(|m| m.thread_id == main_tid.0.to_string())
            .unwrap();
        let m_mc = meta
            .iter()
            .find(|m| m.thread_id == main_child_tid.0.to_string())
            .unwrap();
        let m_r2 = meta
            .iter()
            .find(|m| m.thread_id == root2_tid.0.to_string())
            .unwrap();
        let m_r2c = meta
            .iter()
            .find(|m| m.thread_id == r2_child_tid.0.to_string())
            .unwrap();

        assert_eq!(m_main.parent_thread_id, None);
        assert_eq!(
            m_mc.parent_thread_id.as_deref(),
            Some(main_tid.0.to_string().as_str())
        );
        assert_eq!(m_r2.parent_thread_id, None);
        assert_eq!(
            m_r2c.parent_thread_id.as_deref(),
            Some(root2_tid.0.to_string().as_str())
        );
    }

    #[tokio::test]
    async fn test_manual_join_delivers_landmark_to_live_parent_isolates_siblings_and_rejects_root() {
        use grein_core::blackboard::{BlackboardEntry as LegacyBbEntry, BlackboardEntryKind};

        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("join.db");
        let capture_path = temp.path().join("join_capture.jsonl");

        // Seed legacy rows in SQLite blackboard_entries to prove they open cleanly and are ignored.
        {
            let legacy_storage = SqliteStorage::open(&db_path).expect("open legacy sqlite");
            legacy_storage
                .insert_blackboard_entry(&LegacyBbEntry::new(
                    "legacy_secret",
                    "SHOULD_BE_IGNORED",
                    BlackboardEntryKind::Decision,
                ))
                .expect("seed legacy row");
        }

        let mock_script = r#"
sess_counter=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            sess_counter=$((sess_counter+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s"}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/prompt"'*)
            case "$line" in
                *'"sessionId":"sess-1"'*) sid="sess-1" ;;
                *'"sessionId":"sess-2"'*) sid="sess-2" ;;
                *'"sessionId":"sess-3"'*) sid="sess-3" ;;
                *) sid="sess-4" ;;
            esac
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Branch decision: use SQLite WAL mode."}}}}\n' "$sid"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
        let wrapper = format!(
            "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
            capture_path.to_string_lossy(),
            mock_script
        );
        let script_path = write_executable_script(temp.path(), "mock_join.sh", &wrapper);
        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };

        let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open");
        let main = engine.ensure_main_thread().await.expect("main");
        engine
            .send_turn_for_thread(&main.thread_id, "kickoff")
            .await
            .expect("main turn");

        let main_tid: ThreadId = parse_typed_id(&main.thread_id).unwrap();
        let main_session_id_before = engine
            .runtime
            .storage
            .get_thread(main_tid)
            .unwrap()
            .unwrap()
            .acp_session_id
            .expect("main acp_session_id");
        assert_eq!(main_session_id_before, "sess-1");

        // Joining a root thread returns EngineError::InvalidParams and leaves merged == false.
        let join_root_err = engine
            .merge_thread_with_summary(&main.thread_id, None, Some("bad root join"))
            .await
            .expect_err("joining root must fail");
        assert!(matches!(join_root_err, EngineError::InvalidParams(_)));
        assert!(!engine
            .runtime
            .storage
            .get_thread(main_tid)
            .unwrap()
            .unwrap()
            .merged);

        let branch_a = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("Storage Choice"))
            .await
            .expect("fork a");
        let branch_b = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("Sibling"))
            .await
            .expect("fork b");

        engine
            .send_turn_for_thread(&branch_a.thread_id, "choose storage")
            .await
            .expect("branch_a turn");
        engine
            .send_turn_for_thread(&branch_b.thread_id, "sibling initial")
            .await
            .expect("branch_b turn 1");

        let mut rx = engine.subscribe_tui_events();
        let merged = engine
            .merge_thread_with_summary(&branch_a.thread_id, None, None)
            .await
            .expect("merge_thread_with_summary");
        assert!(merged.merged);

        let mut evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            evs.push(ev);
        }
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, summary }
                if thread_id == &branch_a.thread_id
                    && target_thread_id == &main.thread_id
                    && summary.contains("SQLite WAL mode")
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMetadataUpdated { meta }
                if meta.iter().any(|m| m.thread_id == branch_a.thread_id && m.merged)
        )));

        let main_transcript = engine
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("main transcript");
        assert!(main_transcript.iter().any(|t| {
            t.content.contains("[Decision Landmark") && t.content.contains("SQLite WAL mode")
        }));

        // Live sibling branch receives ZERO [Joined Context] and ZERO [Shared Whiteboard Updated].
        engine
            .send_turn_for_thread(&branch_b.thread_id, "sibling after join")
            .await
            .expect("branch_b turn 2");

        // Live parent (Main) receives [Joined Context] on its next turn while keeping SAME ACP session_id.
        engine
            .send_turn_for_thread(&main.thread_id, "main after join")
            .await
            .expect("main turn 2");
        let main_session_id_after = engine
            .runtime
            .storage
            .get_thread(main_tid)
            .unwrap()
            .unwrap()
            .acp_session_id
            .expect("main acp_session_id after");
        assert_eq!(main_session_id_after, main_session_id_before);

        // Turn 3 on Main does NOT repeat [Joined Context].
        engine
            .send_turn_for_thread(&main.thread_id, "main follow-up 3")
            .await
            .expect("main turn 3");

        // Newly created blank root receives ZERO [Joined Context] and ZERO legacy blackboard.
        let blank_root = engine
            .create_root_thread(Some("Blank Root"))
            .await
            .expect("create blank root");
        let blank_join_err = engine
            .merge_thread_with_summary(&blank_root.thread_id, None, None)
            .await
            .expect_err("joining blank root must fail");
        assert!(matches!(blank_join_err, EngineError::InvalidParams(_)));
        let blank_tid: ThreadId = parse_typed_id(&blank_root.thread_id).unwrap();
        assert!(!engine
            .runtime
            .storage
            .get_thread(blank_tid)
            .unwrap()
            .unwrap()
            .merged);

        engine
            .send_turn_for_thread(&blank_root.thread_id, "blank root prompt")
            .await
            .expect("blank root turn");

        let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
            .expect("capture")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();

        // Exactly 4 session/new calls: main, branch_a, branch_b, blank_root (no reset of main!).
        let session_news: Vec<&Value> = frames
            .iter()
            .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/new"))
            .collect();
        assert_eq!(session_news.len(), 4);
        for sn in &session_news {
            let append = sn["params"]["_meta"]["antigravity"]["systemPrompt"]["append"]
                .as_str()
                .unwrap_or("");
            assert!(!append.contains("# Shared Blackboard Working Memory"));
            assert!(!append.contains("SHOULD_BE_IGNORED"));
            assert!(!append.contains("[Joined Context]"));
        }

        let prompts: Vec<(&str, &str)> = frames
            .iter()
            .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/prompt"))
            .map(|f| {
                (
                    f["params"]["sessionId"].as_str().unwrap(),
                    f["params"]["prompt"][0]["text"].as_str().unwrap(),
                )
            })
            .collect();

        // Sibling sess-3 got verbatim prompt, zero [Joined Context] / [Shared Whiteboard Updated].
        let sibling_prompts: Vec<&str> = prompts
            .iter()
            .filter(|(sid, _)| *sid == "sess-3")
            .map(|(_, text)| *text)
            .collect();
        assert_eq!(sibling_prompts, vec!["sibling initial", "sibling after join"]);

        // Main sess-1 got [Joined Context] with DecisionLandmark on turn 2, and clean prompt on turn 3.
        let main_prompts: Vec<&str> = prompts
            .iter()
            .filter(|(sid, _)| *sid == "sess-1")
            .map(|(_, text)| *text)
            .collect();
        assert_eq!(main_prompts.len(), 3);
        assert_eq!(main_prompts[0], "kickoff");
        assert!(main_prompts[1].starts_with("[Joined Context]\n[Decision Landmark"));
        assert!(main_prompts[1].contains("SQLite WAL mode"));
        assert!(main_prompts[1].ends_with("---\n\nmain after join"));
        assert_eq!(main_prompts[2], "main follow-up 3");

        // Blank root sess-4 got clean prompt.
        let blank_prompts: Vec<&str> = prompts
            .iter()
            .filter(|(sid, _)| *sid == "sess-4")
            .map(|(_, text)| *text)
            .collect();
        assert_eq!(blank_prompts, vec!["blank root prompt"]);
    }

    #[test]
    fn test_extract_tool_call_summary_variants() {
        assert_eq!(
            extract_tool_call_summary("run_command", "Run   tests\nnow", &json!({"cmd": "cargo test"})),
            Some("Run tests now".to_string())
        );
        assert_eq!(
            extract_tool_call_summary(
                "run_command",
                "run_command",
                &json!({"CommandLine": "cargo test --locked"})
            ),
            Some("cargo test --locked".to_string())
        );
        assert_eq!(
            extract_tool_call_summary("read_file", "", &json!({"TargetFile": "src/main.rs"})),
            Some("src/main.rs".to_string())
        );
        assert_eq!(
            extract_tool_call_summary("search", "", &json!({"Query": "fn route"})),
            Some("fn route".to_string())
        );
        assert_eq!(extract_tool_call_summary("noop", "", &json!({})), None);

        let long_cmd = "x".repeat(120);
        let truncated = extract_tool_call_summary("run_command", "", &json!({"command": long_cmd}))
            .expect("truncated");
        assert_eq!(truncated.chars().count(), 80);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_normalize_session_config_options_precedence_and_fallback() {
        use grein_acp::{AcpConfigChoice, AcpModeOption, AcpModelOption};

        // 1. Prefer config_options when non-empty with model/mode
        let full = AcpSessionInfo {
            session_id: "s1".into(),
            current_model_id: Some("legacy-model".into()),
            current_mode_id: Some("legacy-mode".into()),
            available_models: vec![AcpModelOption {
                model_id: "legacy-model".into(),
                name: "Legacy".into(),
                description: None,
            }],
            available_modes: vec![AcpModeOption {
                id: "legacy-mode".into(),
                name: "Legacy Mode".into(),
                description: None,
            }],
            config_options: vec![
                AcpConfigOption {
                    id: "model".into(),
                    name: "Model".into(),
                    category: Some("model".into()),
                    current_value: "modern-model".into(),
                    options: vec![AcpConfigChoice {
                        value: "modern-model".into(),
                        name: "Modern".into(),
                        description: None,
                    }],
                },
                AcpConfigOption {
                    id: "mode".into(),
                    name: "Mode".into(),
                    category: Some("mode".into()),
                    current_value: "modern-mode".into(),
                    options: vec![AcpConfigChoice {
                        value: "modern-mode".into(),
                        name: "Modern Mode".into(),
                        description: None,
                    }],
                },
            ],
        };
        let norm_full = normalize_session_config_options(&full);
        assert_eq!(norm_full.len(), 2);
        assert_eq!(norm_full[0].current_value, "modern-model");
        assert_eq!(norm_full[1].current_value, "modern-mode");

        // 2. Fallback synthesis from available_models / available_modes when config_options empty
        let legacy = AcpSessionInfo {
            session_id: "s2".into(),
            current_model_id: Some("m2".into()),
            current_mode_id: Some("code".into()),
            available_models: vec![
                AcpModelOption {
                    model_id: "m1".into(),
                    name: "M1".into(),
                    description: None,
                },
                AcpModelOption {
                    model_id: "m2".into(),
                    name: "M2".into(),
                    description: Some("Fast".into()),
                },
            ],
            available_modes: vec![AcpModeOption {
                id: "code".into(),
                name: "Code".into(),
                description: None,
            }],
            config_options: Vec::new(),
        };
        let norm_legacy = normalize_session_config_options(&legacy);
        assert_eq!(norm_legacy.len(), 2);
        assert_eq!(norm_legacy[0].id, "model");
        assert_eq!(norm_legacy[0].current_value, "m2");
        assert_eq!(norm_legacy[0].options.len(), 2);
        assert_eq!(norm_legacy[1].id, "mode");
        assert_eq!(norm_legacy[1].current_value, "code");

        // 3. Hybrid: non-empty config_options without model/mode still synthesizes missing model/mode
        let hybrid = AcpSessionInfo {
            session_id: "s3".into(),
            current_model_id: Some("m1".into()),
            current_mode_id: None,
            available_models: vec![AcpModelOption {
                model_id: "m1".into(),
                name: "M1".into(),
                description: None,
            }],
            available_modes: Vec::new(),
            config_options: vec![AcpConfigOption {
                id: "thought_level".into(),
                name: "Thinking".into(),
                category: Some("thought_level".into()),
                current_value: "high".into(),
                options: Vec::new(),
            }],
        };
        let norm_hybrid = normalize_session_config_options(&hybrid);
        assert_eq!(norm_hybrid.len(), 2);
        assert!(norm_hybrid.iter().any(|o| o.id == "thought_level"));
        assert!(norm_hybrid.iter().any(|o| o.id == "model" && o.current_value == "m1"));
    }

    #[tokio::test]
    async fn test_concurrent_send_turn_for_thread_serializes_without_listener_close() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("concurrent_turns.db");
        let capture_path = temp.path().join("concurrent_capture.jsonl");

        let mock_script = r#"
sess_counter=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            sess_counter=$((sess_counter+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s"}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/prompt"'*)
            if printf '%s' "$line" | grep -q "turn-one"; then
                sleep 0.12
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"reply-one"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            else
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"reply-two"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#;
        let wrapper = format!(
            "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
            capture_path.to_string_lossy(),
            mock_script
        );
        let script_path = write_executable_script(temp.path(), "mock_serial.sh", &wrapper);
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

        let e1 = Arc::clone(&engine);
        let t1 = main.thread_id.clone();
        let e2 = Arc::clone(&engine);
        let t2 = main.thread_id.clone();

        let (res1, res2) = tokio::join!(
            async move { e1.send_turn_for_thread(&t1, "turn-one").await },
            async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                e2.send_turn_for_thread(&t2, "turn-two").await
            }
        );

        res1.expect("first turn must succeed");
        res2.expect("second overlapping turn must serialize and succeed");

        let transcript = engine
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("transcript");
        let entries: Vec<(&str, &str)> = transcript
            .iter()
            .map(|t| (t.role.as_str(), t.content.as_str()))
            .collect();
        assert_eq!(
            entries,
            vec![
                ("user", "turn-one"),
                ("assistant", "reply-one"),
                ("user", "turn-two"),
                ("assistant", "reply-two"),
            ]
        );

        let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
            .expect("capture")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        let session_new_count = frames
            .iter()
            .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/new"))
            .count();
        assert_eq!(session_new_count, 1, "must reuse single ACP session");
    }

    #[tokio::test]
    async fn test_live_acp_config_updates_and_commands_via_session_listener() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("live_updates.db");

        let mock_script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-u","models":{"currentModelId":"m1","availableModels":[{"modelId":"m1","name":"M1"},{"modelId":"m2","name":"M2"}]},"modes":{"currentModeId":"default","availableModes":[{"id":"default","name":"Default"},{"id":"plan","name":"Plan"}]}}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compact context"}]}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"current_mode_update","currentModeId":"plan"}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"thought_level","name":"Thinking","category":"thought_level","currentValue":"high","options":[{"value":"high","name":"High"}]}]}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"updated"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
        let script_path = write_executable_script(temp.path(), "mock_updates.sh", mock_script);
        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };

        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
                .expect("open");
        let main = engine.ensure_main_thread().await.expect("main");
        let mut rx = engine.subscribe_tui_events();

        engine
            .send_turn_for_thread(&main.thread_id, "trigger updates")
            .await
            .expect("turn");

        let mut evs = Vec::new();
        while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
            let done = matches!(ev, TuiEngineEvent::ThreadStatusChanged { ref status, .. } if status == "idle");
            evs.push(ev);
            if done {
                break;
            }
        }

        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::AgentCommandsUpdated { thread_id, commands }
                if thread_id == &main.thread_id
                    && commands.len() == 1
                    && commands[0].name == "compact"
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::AgentMetaUpdated { thread_id, mode: Some(mode), .. }
                if thread_id == &main.thread_id && mode == "plan"
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::AgentConfigUpdated { thread_id, config_options }
                if thread_id == &main.thread_id
                    && config_options.iter().any(|o| o.id == "thought_level" && o.current_value == "high")
                    && config_options.iter().any(|o| o.id == "model" && o.current_value == "m1")
                    && config_options.iter().any(|o| o.id == "mode" && o.current_value == "plan")
        )));

        let metas = engine.agent_metadata().await.expect("agent_metadata");
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].mode.as_deref(), Some("plan"));
        assert_eq!(metas[0].available_commands.len(), 1);
        assert!(metas[0].config_options.iter().any(|o| o.id == "thought_level"));
    }

    #[tokio::test]
    async fn test_create_terminal_emits_metadata_updated_before_thread_created() {
        let temp = tempfile::tempdir().expect("tempdir");
        let work_dir = temp.path().join("term_dir");
        std::fs::create_dir_all(&work_dir).expect("mkdir");
        let canonical_dir = std::fs::canonicalize(&work_dir).expect("canonical");
        let db_path = temp.path().join("term_order.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None)
                .expect("open");
        let main = engine.ensure_main_thread().await.expect("main");
        let mut rx = engine.subscribe_tui_events();

        let term = engine
            .create_terminal(Some(&main.thread_id), Some("Ordered Term"), None, 24, 80)
            .await
            .expect("create_terminal");

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }

        let meta_idx = events
            .iter()
            .position(|ev| {
                matches!(
                    ev,
                    TuiEngineEvent::ThreadMetadataUpdated { meta }
                        if meta.iter().any(|m| {
                            m.thread_id == term.thread_id
                                && m.kind == ThreadKind::Terminal
                                && m.cwd.as_deref() == Some(canonical_dir.to_string_lossy().as_ref())
                        })
                )
            })
            .expect("expected ThreadMetadataUpdated containing new terminal meta");
        let created_idx = events
            .iter()
            .position(|ev| {
                matches!(
                    ev,
                    TuiEngineEvent::ThreadCreated { thread } if thread.id == term.thread_id
                )
            })
            .expect("expected ThreadCreated for terminal");

        assert!(
            meta_idx < created_idx,
            "ThreadMetadataUpdated (idx {meta_idx}) must precede ThreadCreated (idx {created_idx}): {events:?}"
        );
    }

    #[tokio::test]
    async fn test_unix_socket_client_stale_daemon_32601_hint() {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("stale.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");

        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut lines = BufReader::new(read).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let req: Value = serde_json::from_str(&line).expect("json");
                        let id = req.get("id").cloned().unwrap_or(Value::Null);
                        let method = req
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let resp = rpc_error(id, -32601, format!("method not found: {method}"));
                        if write.write_all(format!("{resp}\n").as_bytes()).await.is_err()
                            || write.flush().await.is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });

        let client = UnixSocketEngineClient::connect(&socket_path)
            .await
            .expect("connect");

        let err_create = client
            .create_terminal(None, None, None, 24, 80)
            .await
            .expect_err("create_terminal should fail");
        let err_write = client
            .pty_write("t-1", b"echo hi\n".to_vec())
            .await
            .expect_err("pty_write should fail");
        let err_resize = client
            .pty_resize("t-1", 30, 100)
            .await
            .expect_err("pty_resize should fail");
        let err_snap = client
            .pty_snapshot("t-1", 30, 100)
            .await
            .expect_err("pty_snapshot should fail");
        let err_wt = client
            .ensure_thread_worktree("t-1")
            .await
            .expect_err("ensure_thread_worktree should fail");
        let err_scroll = client
            .pty_scroll("t-1", 5)
            .await
            .expect_err("pty_scroll should fail");
        let err_restart = client
            .pty_restart("t-1", 24, 80)
            .await
            .expect_err("pty_restart should fail");

        for (name, err) in [
            ("create_terminal", err_create),
            ("pty_write", err_write),
            ("pty_resize", err_resize),
            ("pty_snapshot", err_snap),
            ("ensure_thread_worktree", err_wt),
            ("pty_scroll", err_scroll),
            ("pty_restart", err_restart),
        ] {
            let msg = err.to_string().to_ascii_lowercase();
            assert!(
                msg.contains("grein daemon")
                    && msg.contains("outdated")
                    && msg.contains("restart"),
                "{name} -32601 error must explain stale grein daemon needs restart, got: {err}"
            );
        }

        let err_ping = client.ping().await.expect_err("ping -32601");
        let ping_msg = err_ping.to_string().to_ascii_lowercase();
        assert!(
            !ping_msg.contains("outdated"),
            "unrelated -32601 method should keep standard protocol error, got: {err_ping}"
        );

        server.abort();
    }

    #[test]
    fn test_resolve_terminal_cwd_tilde_edge_cases() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("ws");
        let home = temp.path().join("fake_home");
        let home_sub = home.join("sub");
        std::fs::create_dir_all(&workspace).expect("mkdir ws");
        std::fs::create_dir_all(&home_sub).expect("mkdir home_sub");
        let canonical_home = std::fs::canonicalize(&home).expect("canonical home");
        let canonical_sub = std::fs::canonicalize(&home_sub).expect("canonical sub");

        let db_path = temp.path().join("tilde_unit.db");
        let engine = InProcessEngine::open_with_config(&db_path, workspace, None).expect("open");
        let home_str = canonical_home.to_string_lossy().into_owned();

        assert_eq!(
            engine
                .runtime
                .resolve_terminal_cwd_with_home(Some("~"), None, Some(&home_str))
                .expect("~ resolves"),
            home_str
        );
        assert_eq!(
            engine
                .runtime
                .resolve_terminal_cwd_with_home(Some("~/"), None, Some(&home_str))
                .expect("~/ resolves"),
            home_str
        );
        assert_eq!(
            engine
                .runtime
                .resolve_terminal_cwd_with_home(Some("~/sub"), None, Some(&home_str))
                .expect("~/sub resolves"),
            canonical_sub.to_string_lossy().as_ref()
        );
        assert_eq!(
            engine
                .runtime
                .resolve_terminal_cwd_with_home(Some("~//sub"), None, Some(&home_str))
                .expect("~//sub resolves without dropping home prefix"),
            canonical_sub.to_string_lossy().as_ref()
        );

        // Unset or empty HOME does not expand tilde.
        assert!(matches!(
            engine
                .runtime
                .resolve_terminal_cwd_with_home(Some("~"), None, None),
            Err(EngineError::InvalidParams(_))
        ));
        assert!(matches!(
            engine
                .runtime
                .resolve_terminal_cwd_with_home(Some("~/sub"), None, Some("")),
            Err(EngineError::InvalidParams(_))
        ));
        // ~otheruser is not expanded to $HOME.
        assert!(matches!(
            engine
                .runtime
                .resolve_terminal_cwd_with_home(Some("~otheruser/sub"), None, Some(&home_str)),
            Err(EngineError::InvalidParams(_))
        ));
    }

    #[tokio::test]
    async fn test_pending_pty_sizes_clamping_and_delete_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("pending_clamp.db");

        let term_id = {
            let engine =
                InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                    .expect("open");
            let _main = engine.ensure_main_thread().await.expect("main");
            let term = engine
                .create_terminal(None, Some("Clamp Term"), None, 24, 80)
                .await
                .expect("create");
            term.thread_id
        };

        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        reopened
            .pty_resize(&term_id, 2, 5)
            .await
            .expect("clamped pending resize");
        assert_eq!(
            reopened
                .runtime
                .pending_pty_sizes
                .lock()
                .unwrap()
                .get(&term_id)
                .copied(),
            Some((4, 10))
        );

        // Zero resize leaves pending size untouched.
        reopened.pty_resize(&term_id, 0, 80).await.expect("zero resize");
        assert_eq!(
            reopened
                .runtime
                .pending_pty_sizes
                .lock()
                .unwrap()
                .get(&term_id)
                .copied(),
            Some((4, 10))
        );

        reopened.delete_thread(&term_id).await.expect("delete");
        assert!(!reopened
            .runtime
            .pending_pty_sizes
            .lock()
            .unwrap()
            .contains_key(&term_id));
    }

    #[tokio::test]
    async fn test_compute_thread_metadata_deduplicates_git_status_per_unique_path() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp = tempfile::tempdir().expect("tempdir");
        let root_dir = temp.path().join("root_ws");
        let sub_dir = temp.path().join("sub_ws");
        std::fs::create_dir_all(&root_dir).expect("mkdir root");
        std::fs::create_dir_all(&sub_dir).expect("mkdir sub");
        let canonical_root = std::fs::canonicalize(&root_dir).expect("canonical root");
        let canonical_sub = std::fs::canonicalize(&sub_dir).expect("canonical sub");
        let db_path = temp.path().join("dedup_git.db");

        let engine = InProcessEngine::open_with_config(&db_path, canonical_root.clone(), None)
            .expect("open");
        let main = engine.ensure_main_thread().await.expect("main");
        let _branch1 = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("b1"))
            .await
            .expect("b1");
        let _branch2 = engine
            .fork_thread_with_title(Some(&main.thread_id), Some("b2"))
            .await
            .expect("b2");
        let term1 = engine
            .create_terminal(
                Some(&main.thread_id),
                Some("t1"),
                Some(&canonical_sub.to_string_lossy()),
                24,
                80,
            )
            .await
            .expect("t1");
        let term2 = engine
            .create_terminal(
                Some(&main.thread_id),
                Some("t2"),
                Some(&canonical_sub.to_string_lossy()),
                24,
                80,
            )
            .await
            .expect("t2");

        let total_calls = AtomicUsize::new(0);
        let calls_per_path: std::sync::Mutex<HashMap<PathBuf, usize>> =
            std::sync::Mutex::new(HashMap::new());

        let meta = engine
            .runtime
            .compute_thread_metadata_with_inspector(|path| {
                total_calls.fetch_add(1, Ordering::SeqCst);
                *calls_per_path
                    .lock()
                    .unwrap()
                    .entry(path.to_path_buf())
                    .or_insert(0) += 1;
                if path == canonical_root.as_path() {
                    Some(RepoGitStatus {
                        branch: Some("main".to_string()),
                        dirty_count: 2,
                        is_worktree: false,
                    })
                } else if path == canonical_sub.as_path() {
                    Some(RepoGitStatus {
                        branch: Some("feat/sub".to_string()),
                        dirty_count: 5,
                        is_worktree: true,
                    })
                } else {
                    None
                }
            })
            .expect("compute_thread_metadata_with_inspector");

        assert_eq!(meta.len(), 5);
        assert_eq!(
            total_calls.load(Ordering::SeqCst),
            2,
            "inspect_git_status must be called once per unique path across 5 threads"
        );
        let counts = calls_per_path.lock().unwrap();
        assert_eq!(counts.get(&canonical_root).copied(), Some(1));
        assert_eq!(counts.get(&canonical_sub).copied(), Some(1));

        let main_meta = meta.iter().find(|m| m.thread_id == main.thread_id).unwrap();
        assert_eq!(main_meta.git_branch.as_deref(), Some("main"));
        assert_eq!(main_meta.git_dirty, 2);

        let t1_meta = meta.iter().find(|m| m.thread_id == term1.thread_id).unwrap();
        let t2_meta = meta.iter().find(|m| m.thread_id == term2.thread_id).unwrap();
        assert_eq!(t1_meta.git_branch.as_deref(), Some("feat/sub"));
        assert_eq!(t1_meta.git_dirty, 5);
        assert_eq!(t2_meta.git_branch.as_deref(), Some("feat/sub"));
        assert_eq!(t2_meta.git_dirty, 5);
    }
}
