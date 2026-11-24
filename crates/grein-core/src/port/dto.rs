use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::model::{ThreadKind, ThreadStatus};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPreferencesSnapshot {
    pub theme: Option<String>,
    pub theme_overrides: BTreeMap<String, String>,
    pub keybindings: BTreeMap<String, String>,
    pub ui: BTreeMap<String, String>,
}

/// A single hydrated conversation turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedTurn {
    pub id: String,
    pub role: String,
    pub content: String,
}

/// A hydrated thread with its trunk transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedThread {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub custom_name: Option<String>,
    pub turns: Vec<HydratedTurn>,
}

/// Full session snapshot used to seed the TUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedSession {
    pub active_thread_id: String,
    pub threads: Vec<HydratedThread>,
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

/// Structural parent/child metadata for a conversation or terminal thread.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThreadMeta {
    pub thread_id: String,
    pub parent_thread_id: Option<String>,
    pub inherited_turn_count: usize,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub kind: ThreadKind,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_dirty: usize,
    #[serde(default)]
    pub custom_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub repo_root: String,
    pub repo_name: String,
    pub path: String,
    pub branch: Option<String>,
    pub head_short: Option<String>,
    pub is_primary: bool,
    pub is_detached: bool,
    pub is_grein_managed: bool,
    pub dirty_count: usize,
    pub attached_thread_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PtyMouseMode {
    #[default]
    None,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PtyMouseEncoding {
    #[default]
    Default,
    Utf8,
    Sgr,
}

/// Terminal foreground/background color captured from VT100 emulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PtyColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// A single rendered cell inside a live embedded PTY screen snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PtyCell {
    pub contents: String,
    pub fg: PtyColor,
    pub bg: PtyColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub is_wide_continuation: bool,
}

/// Live virtual terminal screen snapshot streamed from an interactive PTY thread.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PtyScreenSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<Vec<PtyCell>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub hide_cursor: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub exited: bool,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub scrollback_offset: usize,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub row_wrapped: Vec<bool>,
    #[serde(default)]
    pub mouse_protocol_mode: PtyMouseMode,
    #[serde(default)]
    pub mouse_protocol_encoding: PtyMouseEncoding,
}

impl PtyScreenSnapshot {
    /// Extracts clean UTF-8 lines from the visible screen (skipping wide continuations).
    pub fn plain_text(&self) -> String {
        let mut lines: Vec<String> = self
            .cells
            .iter()
            .map(|row| {
                let mut line = String::new();
                for cell in row {
                    if cell.is_wide_continuation {
                        continue;
                    }
                    if cell.contents.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&cell.contents);
                    }
                }
                line.trim_end().to_string()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }
}

/// Execution lifecycle status of a live subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
}

/// Point-in-time snapshot of a subagent running under a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentSnapshot {
    pub id: String,
    pub role: String,
    pub agent_type: String,
    pub model: Option<String>,
    pub activity: Option<String>,
    pub status: SubagentStatus,
    pub started_at_ms: u64,
}

/// Selectable option value inside an ACP session config option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfigChoice {
    pub value: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Configurable ACP session property (such as model, thought_level, or mode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfigOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    pub current_value: String,
    #[serde(default)]
    pub options: Vec<AgentConfigChoice>,
}

/// Agent-advertised slash command or skill exposed by ACP `available_commands_update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Available ACP config options and commands for a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentOptionsSnapshot {
    pub thread_id: String,
    #[serde(default)]
    pub config_options: Vec<AgentConfigOption>,
    #[serde(default)]
    pub available_commands: Vec<AgentSlashCommand>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineControlEvent {
    TurnRecorded { node_id: String },
    ThreadForked { thread_id: String },
    ThreadMerged { thread_id: String },
    PermissionResolved { request_id: String, allow: bool },
}

/// Events streamed from the engine to the TUI using externally-tagged JSON wire encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineStreamEvent {
    StreamChunk {
        thread_id: String,
        content: String,
    },
    TurnCompleted {
        thread_id: String,
    },
    ToolInvoked {
        thread_id: String,
        tool_name: String,
        #[serde(default)]
        tool_call_id: Option<String>,
        #[serde(default)]
        summary: Option<String>,
    },
    ToolResult {
        thread_id: String,
        tool_name: String,
        output: String,
        #[serde(default)]
        tool_call_id: Option<String>,
        #[serde(default)]
        is_error: bool,
    },
    PermissionRequested {
        request_id: String,
        description: String,
    },
    ThreadStatusChanged {
        thread_id: String,
        status: String,
    },
    ThoughtChunk {
        thread_id: String,
        content: String,
    },
    ThreadCreated {
        thread: HydratedThread,
    },
    ThreadTitleUpdated {
        thread_id: String,
        title: String,
    },
    ThreadCustomNameUpdated {
        thread_id: String,
        #[serde(default)]
        custom_name: Option<String>,
    },
    ThreadMerged {
        thread_id: String,
        target_thread_id: String,
        summary: String,
    },
    ThreadMetadataUpdated {
        meta: Vec<ThreadMeta>,
    },
    ThreadDeleted {
        thread_id: String,
        next_active_thread_id: String,
    },
    AgentMetaUpdated {
        thread_id: String,
        model: Option<String>,
        mode: Option<String>,
    },
    AgentConfigUpdated {
        thread_id: String,
        config_options: Vec<AgentConfigOption>,
    },
    AgentCommandsUpdated {
        thread_id: String,
        commands: Vec<AgentSlashCommand>,
    },
    SubagentUpdated {
        thread_id: String,
        subagent: SubagentSnapshot,
    },
    SubagentRemoved {
        thread_id: String,
        subagent_id: String,
    },
    ToolActivityUpdated {
        thread_id: String,
        tool_name: Option<String>,
    },
    ShellOutput {
        thread_id: String,
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    PtyScreenUpdated {
        thread_id: String,
        snapshot: PtyScreenSnapshot,
    },
    PtyExited {
        thread_id: String,
        exit_code: Option<i32>,
    },
    Error {
        message: String,
    },
}

pub type TuiEngineEvent = EngineStreamEvent;
