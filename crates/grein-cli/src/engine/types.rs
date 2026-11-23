use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use grein_acp::{
    AcpConfigOption, AcpPermissionOption, AcpSessionInfo, StdioRunnerError,
};
use grein_core::model::{AuthorRole, ConversationNode, NodeId, Thread, ThreadKind, ThreadStatus};
use grein_tui::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentOptionsSnapshot, AgentSlashCommand,
    PtyCell as TuiPtyCell, PtyColor as TuiPtyColor,
    PtyScreenSnapshot as TuiPtyScreenSnapshot, ThreadMeta, TuiEngineEvent, WorktreeEntry,
};
use grein_workspace::pty_session::{
    PtyCell as WsPtyCell, PtyColor as WsPtyColor, PtyScreenSnapshot as WsPtyScreenSnapshot,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::discovery::{discover_acp_agents, AcpAgentKind};

pub(super) const INTERNAL_ROOT_CONTENT: &str = "Session initialized";
pub(super) const ACP_SYSTEM_PROMPT_MAX_CHARS: usize = 32_000;
pub(super) const SHELL_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const SHELL_MAX_OUTPUT_BYTES: usize = 65_536;
pub(super) const UNSOLICITED_QUIET_TIMEOUT: Duration = Duration::from_millis(1500);
pub(super) const CLOSE_SESSION_TIMEOUT: Duration = Duration::from_millis(250);
pub(super) const PTY_COALESCE_WINDOW: Duration = Duration::from_millis(16);

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
    pub(super) fn rpc_code(&self) -> i64 {
        match self {
            Self::InvalidParams(_) => -32602,
            Self::NotFound(_) => -32004,
            Self::Protocol(_) => -32600,
            Self::Storage(_) | Self::Transport(_) => -32000,
        }
    }
}

pub(super) fn storage_err(err: impl std::fmt::Display) -> EngineError {
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
    #[serde(default)]
    pub custom_name: Option<String>,
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
    async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        Err(EngineError::Protocol(
            "list_worktrees not supported".to_string(),
        ))
    }
    async fn switch_thread_worktree(
        &self,
        _thread_id: &str,
        _target_path: &str,
    ) -> Result<String, EngineError> {
        Err(EngineError::Protocol(
            "switch_thread_worktree not supported".to_string(),
        ))
    }
    async fn remove_worktree(&self, _path: &str, _force: bool) -> Result<(), EngineError> {
        Err(EngineError::Protocol(
            "remove_worktree not supported".to_string(),
        ))
    }
    async fn worktree_diff_preview(&self, _path: &str) -> Result<String, EngineError> {
        Err(EngineError::Protocol(
            "worktree_diff_preview not supported".to_string(),
        ))
    }
    async fn prune_worktrees(&self) -> Result<String, EngineError> {
        Err(EngineError::Protocol(
            "prune_worktrees not supported".to_string(),
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

pub(super) fn parse_typed_id<T: DeserializeOwned>(raw: &str) -> Option<T> {
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

pub(super) fn is_internal_root_node(node: &ConversationNode) -> bool {
    node.parent_id.is_none()
        && node.role == AuthorRole::System
        && (node.content.is_empty() || node.content == INTERNAL_ROOT_CONTENT)
}

pub(super) fn normalize_custom_name(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(super) fn thread_to_summary(thread: &Thread, is_first: bool) -> ThreadSummary {
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
        custom_name: normalize_custom_name(thread.custom_name.as_deref()),
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

pub(super) fn pty_snapshot_to_tui(snap: WsPtyScreenSnapshot) -> TuiPtyScreenSnapshot {
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

pub(super) fn node_to_transcript_entry(node: &ConversationNode) -> TranscriptEntry {
    TranscriptEntry {
        node_id: node.id.0.to_string(),
        role: node.role.to_string(),
        content: node.content.clone(),
    }
}

pub(super) fn select_permission_option(options: &[AcpPermissionOption], approved: bool) -> String {
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

pub(super) fn format_permission_description(tool_name: &str, title: &str, args: &Value) -> String {
    match (tool_name.trim().is_empty(), title.trim().is_empty()) {
        (false, false) => format!("{tool_name}: {title}"),
        (false, true) if !args.is_null() => format!("{tool_name} {args}"),
        (false, true) => tool_name.to_string(),
        (true, false) => title.to_string(),
        (true, true) => "Permission requested".to_string(),
    }
}

pub(super) fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn path_from_node_map(
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

pub(super) fn extract_subagent_role(args: &Value, title: &str) -> String {
    args.get("Role")
        .or_else(|| args.get("role"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| (!title.trim().is_empty()).then_some(title.trim()))
        .unwrap_or("Subagent")
        .to_string()
}

pub(super) fn extract_subagent_type(args: &Value) -> String {
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

pub(super) fn extract_tool_call_summary(
    tool_name: &str,
    title: &str,
    args: &Value,
) -> Option<String> {
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

pub(super) fn acp_config_option_to_tui(opt: &AcpConfigOption) -> AgentConfigOption {
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

pub(super) fn normalize_session_config_options(info: &AcpSessionInfo) -> Vec<AgentConfigOption> {
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

pub(super) fn extract_model_and_mode_from_options(
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

pub(super) fn merge_updated_config_options(
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

pub(super) fn is_unsupported_method_error(err: &StdioRunnerError) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("-32601")
        || msg.contains("method not found")
        || msg.contains("not supported")
        || msg.contains("unsupported")
        || msg.contains("unimplemented")
        || msg.contains("unknown method")
}

pub(super) fn stdio_err_to_engine(err: StdioRunnerError) -> EngineError {
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
