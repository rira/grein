use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

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

/// Structural parent/child metadata for a conversation or terminal thread.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThreadMeta {
    pub thread_id: String,
    pub parent_thread_id: Option<String>,
    pub inherited_turn_count: usize,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub kind: grein_core::ThreadKind,
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

/// Events streamed from the engine to the TUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TuiEngineEvent {
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

/// Service provider interface between the TUI and an engine implementation.
#[async_trait::async_trait]
pub trait TuiEnginePort: Send + Sync {
    async fn hydrate_session(&self) -> io::Result<HydratedSession>;
    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()>;
    async fn fork_thread(&self, thread_id: &str, anchor_id: &str) -> io::Result<String>;
    async fn merge_thread(&self, source_id: &str, target_id: &str) -> io::Result<()>;
    async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()>;
    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>>;

    async fn fork_branch(
        &self,
        source_thread_id: &str,
        title: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let id = self
            .fork_thread(source_thread_id, title.unwrap_or(""))
            .await?;
        Ok(HydratedThread {
            id,
            title: title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or("Branch")
                .to_string(),
            custom_name: None,
            turns: Vec::new(),
        })
    }

    async fn cancel_turn(&self, _thread_id: &str) -> io::Result<()> {
        Ok(())
    }

    async fn run_shell_command(&self, _thread_id: &str, _command: &str) -> io::Result<()> {
        Ok(())
    }

    async fn delete_thread(&self, _thread_id: &str) -> Result<String, String> {
        Err("delete_thread not supported".to_string())
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
        Ok(Vec::new())
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: &str,
        _summary: Option<&str>,
    ) -> io::Result<()> {
        self.merge_thread(source_id, target_id).await
    }

    async fn create_root(&self, _title: Option<&str>) -> io::Result<HydratedThread> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "create_root not supported",
        ))
    }

    async fn rename_thread(&self, _thread_id: &str, _title: &str) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "rename_thread not supported",
        ))
    }

    async fn agent_options(&self, thread_id: &str) -> io::Result<AgentOptionsSnapshot> {
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
    ) -> io::Result<AgentOptionsSnapshot> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "set_agent_config not supported",
        ))
    }

    async fn create_terminal(
        &self,
        _parent_thread_id: Option<&str>,
        _title: Option<&str>,
        _cwd: Option<&str>,
        _rows: u16,
        _cols: u16,
    ) -> io::Result<HydratedThread> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "create_terminal not supported",
        ))
    }

    async fn pty_write(&self, _thread_id: &str, _bytes: Vec<u8>) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pty_write not supported",
        ))
    }

    async fn pty_resize(&self, _thread_id: &str, _rows: u16, _cols: u16) -> io::Result<()> {
        Ok(())
    }

    async fn pty_snapshot(
        &self,
        _thread_id: &str,
        _rows: u16,
        _cols: u16,
    ) -> io::Result<PtyScreenSnapshot> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pty_snapshot not supported",
        ))
    }

    async fn ensure_thread_worktree(&self, _thread_id: &str) -> io::Result<String> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "ensure_thread_worktree not supported",
        ))
    }

    async fn list_worktrees(&self) -> io::Result<Vec<WorktreeEntry>> {
        Ok(Vec::new())
    }

    async fn switch_thread_worktree(
        &self,
        _thread_id: &str,
        _target_path: &str,
    ) -> io::Result<String> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "switch_thread_worktree not supported",
        ))
    }

    async fn remove_worktree(&self, _path: &str, _force: bool) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "remove_worktree not supported",
        ))
    }

    async fn pty_scroll(&self, _thread_id: &str, _delta: isize) -> io::Result<PtyScreenSnapshot> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pty_scroll not supported",
        ))
    }

    async fn pty_restart(
        &self,
        _thread_id: &str,
        _rows: u16,
        _cols: u16,
    ) -> io::Result<PtyScreenSnapshot> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pty_restart not supported",
        ))
    }

    async fn worktree_diff_preview(&self, _path: &str) -> io::Result<String> {
        Ok(String::new())
    }

    async fn prune_worktrees(&self) -> io::Result<String> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "prune_worktrees is not supported",
        ))
    }

    async fn load_preferences(&self) -> io::Result<UserPreferencesSnapshot> {
        Ok(UserPreferencesSnapshot::default())
    }

    async fn save_theme_preference(&self, _theme: &str) -> io::Result<()> {
        Ok(())
    }

    async fn save_keybinding_preference(&self, _action: &str, _binding: &str) -> io::Result<()> {
        Ok(())
    }

    async fn remove_keybinding_preference(&self, _action: &str) -> io::Result<()> {
        Ok(())
    }

    async fn save_ui_preference(&self, _key: &str, _value: &str) -> io::Result<()> {
        Ok(())
    }

    async fn list_recent_user_prompts(&self, _limit: usize) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn list_workspace_files(&self, _root: Option<&str>) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn create_root_in_cwd(
        &self,
        title: Option<&str>,
        _cwd: Option<&str>,
    ) -> io::Result<HydratedThread> {
        self.create_root(title).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_screen_snapshot_serde_defaults_new_fields() {
        let legacy_json = r#"{
            "rows": 24,
            "cols": 80,
            "cells": [],
            "cursor_row": 0,
            "cursor_col": 0,
            "hide_cursor": false,
            "application_cursor": false,
            "bracketed_paste": false,
            "alternate_screen": false,
            "exited": false,
            "exit_code": null
        }"#;
        let snap: PtyScreenSnapshot =
            serde_json::from_str(legacy_json).expect("deserialize legacy snapshot");
        assert_eq!(snap.scrollback_offset, 0);
        assert_eq!(snap.cwd, None);
        assert!(snap.row_wrapped.is_empty());
        assert_eq!(snap.mouse_protocol_mode, PtyMouseMode::None);
        assert_eq!(snap.mouse_protocol_encoding, PtyMouseEncoding::Default);
    }
}
