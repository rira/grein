use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use grein_acp::{AcpConfigOption, AcpPermissionOption, AcpSessionInfo, AgentDriverError};
use grein_core::model::{AuthorRole, ConversationNode, NodeId, Thread, ThreadKind};
pub use grein_core::port::dto::{
    AgentConfigChoice, AgentConfigOption, AgentMetaEntry, EngineControlEvent as EngineEvent,
    PermissionDecisionPayload, SessionSnapshot, ShellCommandResult, ThreadSummary,
    TranscriptEntry, TurnDispatchResult,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::discovery::{claude_acp_args, discover_acp_agents, AcpAgentKind};

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
                args: claude_acp_args(&claude.executable_path),
                auth_method: auth_override.unwrap_or(None),
                cwd,
            });
        }
        None
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
        agent_provider: thread.agent_provider.clone(),
        parent_node_id: thread.parent_node_id.0.to_string(),
        active_leaf_id: thread.active_leaf_id.0.to_string(),
        merged: thread.merged,
        status: thread.status,
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

pub(super) fn acp_config_option_to_core(opt: &AcpConfigOption) -> AgentConfigOption {
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
        .map(acp_config_option_to_core)
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

pub(super) fn is_unsupported_method_error(err: &AgentDriverError) -> bool {
    if matches!(err, AgentDriverError::Unsupported(_)) {
        return true;
    }
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("-32601")
        || msg.contains("method not found")
        || msg.contains("not supported")
        || msg.contains("unsupported")
        || msg.contains("unimplemented")
        || msg.contains("unknown method")
}

pub(super) fn driver_err_to_engine(err: AgentDriverError) -> EngineError {
    match err {
        AgentDriverError::Terminated(msg) => EngineError::Transport(msg),
        AgentDriverError::Protocol(msg) => {
            if msg.contains("-32602") {
                EngineError::InvalidParams(msg)
            } else {
                EngineError::Protocol(msg)
            }
        }
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

#[cfg(test)]
mod tests {
    use grein_core::port::dto::{PtyMouseEncoding, PtyMouseMode, PtyScreenSnapshot};
    use grein_workspace::pty_session::{
        PtyMouseEncoding as WsPtyMouseEncoding, PtyMouseMode as WsPtyMouseMode,
        PtyScreenSnapshot as WsPtyScreenSnapshot,
    };

    #[test]
    fn workspace_pty_snapshot_is_identical_to_core_dto() {
        let ws_snap = WsPtyScreenSnapshot {
            rows: 2,
            cols: 3,
            row_wrapped: vec![true, false],
            mouse_protocol_mode: WsPtyMouseMode::AnyMotion,
            mouse_protocol_encoding: WsPtyMouseEncoding::Sgr,
            ..WsPtyScreenSnapshot::default()
        };
        let core_snap: PtyScreenSnapshot = ws_snap;
        assert_eq!(core_snap.row_wrapped, vec![true, false]);
        assert_eq!(core_snap.mouse_protocol_mode, PtyMouseMode::AnyMotion);
        assert_eq!(core_snap.mouse_protocol_encoding, PtyMouseEncoding::Sgr);
    }

    #[test]
    fn discover_default_from_derives_claude_args_per_executable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let early = temp.path().join("early");
        let late = temp.path().join("late");
        std::fs::create_dir_all(&early).unwrap();
        std::fs::create_dir_all(&late).unwrap();

        let make_exec = |dir: &std::path::Path, name: &str| {
            let p = dir.join(name);
            std::fs::write(&p, "#!/bin/sh\nexit 0\n").expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&p).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&p, perms).unwrap();
            }
            p
        };

        let claude = make_exec(&early, "claude");
        let early_only = super::AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            None,
            None,
            None,
            Some(&early.to_string_lossy()),
        )
        .expect("claude config");
        assert_eq!(early_only.command, claude.to_string_lossy());
        assert_eq!(early_only.args, vec!["--acp".to_string()]);

        let code_acp = make_exec(&late, "claude-code-acp");
        let search = std::env::join_paths([&early, &late])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let with_code_acp = super::AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            None,
            None,
            None,
            Some(&search),
        )
        .expect("claude-code-acp config");
        assert_eq!(with_code_acp.command, code_acp.to_string_lossy());
        assert!(with_code_acp.args.is_empty());

        let agent_acp = make_exec(&late, "claude-agent-acp");
        let with_agent_acp = super::AcpBackendConfig::discover_default_from(
            temp.path().to_path_buf(),
            None,
            None,
            None,
            Some(&search),
        )
        .expect("claude-agent-acp config");
        assert_eq!(with_agent_acp.command, agent_acp.to_string_lossy());
        assert!(with_agent_acp.args.is_empty());
    }
}
