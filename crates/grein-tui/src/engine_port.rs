use std::io;

use tokio::sync::mpsc;

pub use grein_core::port::dto::{
    AgentConfigChoice, AgentConfigOption, AgentOptionsSnapshot, AgentProviderInfo,
    AgentSlashCommand, HydratedSession, HydratedThread, HydratedTurn, PtyCell, PtyColor,
    PtyMouseEncoding, PtyMouseMode, PtyScreenSnapshot, SubagentSnapshot, SubagentStatus,
    ThreadMeta, ThreadStatus, TuiEngineEvent, UserPreferencesSnapshot, WorktreeEntry,
};

/// Service provider interface for loading and persisting user preferences.
#[async_trait::async_trait]
pub trait PreferencesPort: Send + Sync {
    async fn load_preferences(&self) -> io::Result<UserPreferencesSnapshot>;
    async fn save_theme_preference(&self, theme: &str) -> io::Result<()>;
    async fn save_keybinding_preference(&self, action: &str, binding: &str) -> io::Result<()>;
    async fn remove_keybinding_preference(&self, action: &str) -> io::Result<()>;
    async fn save_ui_pref(&self, key: &str, value: &str) -> io::Result<()>;
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
            agent_provider: None,
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

    async fn list_agent_providers(&self) -> io::Result<Vec<AgentProviderInfo>> {
        Ok(Vec::new())
    }

    async fn create_agent_thread(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
        _parent_thread_id: Option<&str>,
        _agent_provider: Option<&str>,
    ) -> io::Result<HydratedThread> {
        self.create_root_in_cwd(title, cwd).await
    }

    async fn fork_branch_with_provider(
        &self,
        source_thread_id: &str,
        title: Option<&str>,
        _agent_provider: Option<&str>,
    ) -> io::Result<HydratedThread> {
        self.fork_branch(source_thread_id, title).await
    }

    async fn switch_thread_provider(&self, _thread_id: &str, _provider: &str) -> io::Result<String> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "switch_thread_provider not supported",
        ))
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

    #[tokio::test]
    async fn preferences_port_is_object_safe() {
        struct MemPrefs;
        #[async_trait::async_trait]
        impl PreferencesPort for MemPrefs {
            async fn load_preferences(&self) -> io::Result<UserPreferencesSnapshot> {
                Ok(UserPreferencesSnapshot::default())
            }

            async fn save_theme_preference(&self, _theme: &str) -> io::Result<()> {
                Ok(())
            }

            async fn save_keybinding_preference(
                &self,
                _action: &str,
                _binding: &str,
            ) -> io::Result<()> {
                Ok(())
            }

            async fn remove_keybinding_preference(&self, _action: &str) -> io::Result<()> {
                Ok(())
            }

            async fn save_ui_pref(&self, _key: &str, _value: &str) -> io::Result<()> {
                Ok(())
            }
        }

        let port: std::sync::Arc<dyn PreferencesPort> = std::sync::Arc::new(MemPrefs);
        assert_eq!(
            port.load_preferences().await.unwrap(),
            UserPreferencesSnapshot::default()
        );
        port.save_theme_preference("nord").await.unwrap();
        port.save_keybinding_preference("quit", "q").await.unwrap();
        port.remove_keybinding_preference("quit").await.unwrap();
        port.save_ui_pref("sidebar_mode", "fixed").await.unwrap();
    }
}
