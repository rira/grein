use async_trait::async_trait;
use grein_tui::engine_port::{
    AgentOptionsSnapshot, PtyScreenSnapshot as TuiPtyScreenSnapshot, ThreadMeta, TuiEngineEvent,
    WorktreeEntry,
};
use tokio::sync::mpsc;

use super::in_process::InProcessEngine;
use super::socket_client::UnixSocketEngineClient;
use super::types::{
    AgentMetaEntry, DaemonEngine, EngineError, EngineEvent, PermissionDecisionPayload,
    SessionSnapshot, ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};

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

    async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        match self {
            Self::InProcess(e) => e.create_root_thread_in_cwd(title, cwd).await,
            Self::Socket(e) => e.create_root_thread_in_cwd(title, cwd).await,
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

    async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::list_worktrees(e.as_ref()).await,
            Self::Socket(e) => DaemonEngine::list_worktrees(e).await,
        }
    }

    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        match self {
            Self::InProcess(e) => {
                DaemonEngine::switch_thread_worktree(e.as_ref(), thread_id, target_path).await
            }
            Self::Socket(e) => {
                DaemonEngine::switch_thread_worktree(e, thread_id, target_path).await
            }
        }
    }

    async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::remove_worktree(e.as_ref(), path, force).await,
            Self::Socket(e) => DaemonEngine::remove_worktree(e, path, force).await,
        }
    }

    async fn worktree_diff_preview(&self, path: &str) -> Result<String, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::worktree_diff_preview(e.as_ref(), path).await,
            Self::Socket(e) => DaemonEngine::worktree_diff_preview(e, path).await,
        }
    }

    async fn prune_worktrees(&self) -> Result<String, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::prune_worktrees(e.as_ref()).await,
            Self::Socket(e) => DaemonEngine::prune_worktrees(e).await,
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

    async fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::list_recent_user_prompts(e.as_ref(), limit).await,
            Self::Socket(e) => DaemonEngine::list_recent_user_prompts(e, limit).await,
        }
    }

    async fn list_workspace_files(&self, root: Option<&str>) -> Result<Vec<String>, EngineError> {
        match self {
            Self::InProcess(e) => DaemonEngine::list_workspace_files(e.as_ref(), root).await,
            Self::Socket(e) => DaemonEngine::list_workspace_files(e, root).await,
        }
    }
}
