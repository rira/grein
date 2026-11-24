use async_trait::async_trait;
use grein_core::port::dto::{
    AgentOptionsSnapshot, EngineStreamEvent, PtyScreenSnapshot, ThreadMeta, WorktreeEntry,
};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::types::{
    AgentMetaEntry, EngineError, EngineEvent, PermissionDecisionPayload, SessionSnapshot,
    ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};

#[async_trait]
pub trait ThreadUseCasePort: Send + Sync {
    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError>;
    async fn fork_thread(&self, parent_node_id: Option<&str>)
        -> Result<ThreadSummary, EngineError>;
    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError>;
    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError>;
    async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        _cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.create_root_thread(title).await
    }
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
    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError>;
    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        Ok(Vec::new())
    }
    async fn delete_thread(&self, _thread_id: &str) -> Result<String, EngineError> {
        Err(EngineError::Protocol(
            "delete_thread not supported".to_string(),
        ))
    }
}

#[async_trait]
pub trait AgentTurnPort: Send + Sync {
    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError>;
    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError>;
    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError>;
    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError>;
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
}

#[async_trait]
pub trait ShellUseCasePort: Send + Sync {
    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError>;
}

#[async_trait]
pub trait TerminalPtyUseCasePort: Send + Sync {
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
    ) -> Result<PtyScreenSnapshot, EngineError> {
        Err(EngineError::Protocol(
            "pty_snapshot not supported".to_string(),
        ))
    }
    async fn pty_scroll(
        &self,
        _thread_id: &str,
        _delta: isize,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        Err(EngineError::Protocol(
            "pty_scroll not supported".to_string(),
        ))
    }
    async fn pty_restart(
        &self,
        _thread_id: &str,
        _rows: u16,
        _cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        Err(EngineError::Protocol(
            "pty_restart not supported".to_string(),
        ))
    }
}

#[async_trait]
pub trait WorktreeUseCasePort: Send + Sync {
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
}

#[async_trait]
pub trait AutocompleteContextPort: Send + Sync {
    async fn list_recent_user_prompts(&self, _limit: usize) -> Result<Vec<String>, EngineError> {
        Ok(Vec::new())
    }
    async fn list_workspace_files(&self, _root: Option<&str>) -> Result<Vec<String>, EngineError> {
        Ok(Vec::new())
    }
}

#[async_trait]
pub trait EngineEventPort: Send + Sync {
    async fn ping(&self) -> Result<(), EngineError>;
    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<EngineStreamEvent>>, EngineError> {
        Ok(None)
    }
}

#[async_trait]
impl<T: ThreadUseCasePort + ?Sized> ThreadUseCasePort for Arc<T> {
    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        (**self).ensure_main_thread().await
    }
    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        (**self).fork_thread(parent_node_id).await
    }
    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        (**self)
            .fork_thread_with_title(source_thread_or_node, title)
            .await
    }
    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        (**self).create_root_thread(title).await
    }
    async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        (**self).create_root_thread_in_cwd(title, cwd).await
    }
    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        (**self).rename_thread(thread_id, title).await
    }
    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        (**self).list_threads().await
    }
    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        (**self).get_transcript().await
    }
    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        (**self).get_thread_transcript(thread_id).await
    }
    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        (**self).merge_thread(thread_id).await
    }
    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        (**self)
            .merge_thread_with_summary(source_id, target_id, summary)
            .await
    }
    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        (**self).snapshot().await
    }
    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        (**self).thread_metadata().await
    }
    async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        (**self).delete_thread(thread_id).await
    }
}

#[async_trait]
impl<T: AgentTurnPort + ?Sized> AgentTurnPort for Arc<T> {
    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        (**self).send_turn(prompt).await
    }
    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        (**self).send_turn_for_thread(thread_id, prompt).await
    }
    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        (**self).resolve_permission(decision).await
    }
    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        (**self).cancel_turn(thread_id).await
    }
    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        (**self).agent_metadata().await
    }
    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        (**self).agent_options(thread_id).await
    }
    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        (**self)
            .set_agent_config(thread_id, config_id, value, set_default)
            .await
    }
}

#[async_trait]
impl<T: ShellUseCasePort + ?Sized> ShellUseCasePort for Arc<T> {
    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        (**self).run_shell_command(thread_id, command).await
    }
}

#[async_trait]
impl<T: TerminalPtyUseCasePort + ?Sized> TerminalPtyUseCasePort for Arc<T> {
    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        (**self)
            .create_terminal(parent_thread_id, title, cwd, rows, cols)
            .await
    }
    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        (**self).pty_write(thread_id, bytes).await
    }
    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        (**self).pty_resize(thread_id, rows, cols).await
    }
    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        (**self).pty_snapshot(thread_id, rows, cols).await
    }
    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        (**self).pty_scroll(thread_id, delta).await
    }
    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        (**self).pty_restart(thread_id, rows, cols).await
    }
}

#[async_trait]
impl<T: WorktreeUseCasePort + ?Sized> WorktreeUseCasePort for Arc<T> {
    async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        (**self).ensure_thread_worktree(thread_id).await
    }
    async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        (**self).list_worktrees().await
    }
    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        (**self)
            .switch_thread_worktree(thread_id, target_path)
            .await
    }
    async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), EngineError> {
        (**self).remove_worktree(path, force).await
    }
    async fn worktree_diff_preview(&self, path: &str) -> Result<String, EngineError> {
        (**self).worktree_diff_preview(path).await
    }
    async fn prune_worktrees(&self) -> Result<String, EngineError> {
        (**self).prune_worktrees().await
    }
}

#[async_trait]
impl<T: AutocompleteContextPort + ?Sized> AutocompleteContextPort for Arc<T> {
    async fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, EngineError> {
        (**self).list_recent_user_prompts(limit).await
    }
    async fn list_workspace_files(&self, root: Option<&str>) -> Result<Vec<String>, EngineError> {
        (**self).list_workspace_files(root).await
    }
}

#[async_trait]
impl<T: EngineEventPort + ?Sized> EngineEventPort for Arc<T> {
    async fn ping(&self) -> Result<(), EngineError> {
        (**self).ping().await
    }
    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<EngineStreamEvent>>, EngineError> {
        (**self).subscribe_events().await
    }
}

/// Composed inbound engine port combining all 7 segregated use-case ports.
pub trait GreinEngine:
    ThreadUseCasePort
    + AgentTurnPort
    + ShellUseCasePort
    + TerminalPtyUseCasePort
    + WorktreeUseCasePort
    + AutocompleteContextPort
    + EngineEventPort
    + Send
    + Sync
{
}

impl<T> GreinEngine for T where
    T: ThreadUseCasePort
        + AgentTurnPort
        + ShellUseCasePort
        + TerminalPtyUseCasePort
        + WorktreeUseCasePort
        + AutocompleteContextPort
        + EngineEventPort
        + Send
        + Sync
        + ?Sized
{
}

/// Backwards-compatible supertrait alias for callers importing `DaemonEngine`.
pub trait DaemonEngine: GreinEngine {}

impl<T: GreinEngine + ?Sized> DaemonEngine for T {}
