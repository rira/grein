use async_trait::async_trait;
use grein_core::port::dto::{
    AgentOptionsSnapshot, AgentProviderInfo, EngineStreamEvent, PtyScreenSnapshot, ThreadMeta,
    WorktreeEntry,
};
use tokio::sync::mpsc;

use super::super::ports::{
    AgentTurnPort, AutocompleteContextPort, EngineEventPort, ShellUseCasePort,
    TerminalPtyUseCasePort, ThreadUseCasePort, WorktreeUseCasePort,
};
use super::super::types::{
    AgentMetaEntry, EngineError, EngineEvent, PermissionDecisionPayload, SessionSnapshot,
    ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};
use super::InProcessEngine;

#[async_trait]
impl ThreadUseCasePort for InProcessEngine {
    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        self.ensure_main_thread_impl().await
    }

    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.fork_thread_with_title_impl(parent_node_id, None).await
    }

    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.fork_thread_with_title_impl(source_thread_or_node, title)
            .await
    }

    async fn fork_thread_with_provider(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
        agent_provider: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.fork_thread_with_provider_impl(source_thread_or_node, title, agent_provider)
            .await
    }

    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        self.create_root_thread_in_cwd_impl(title, None).await
    }

    async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.create_root_thread_in_cwd_impl(title, cwd).await
    }

    async fn create_agent_thread(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
        parent_thread_id: Option<&str>,
        agent_provider: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.create_agent_thread_impl(title, cwd, parent_thread_id, agent_provider)
            .await
    }

    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        self.rename_thread_impl(thread_id, title).await
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        self.list_threads_impl().await
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.get_transcript_impl().await
    }

    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.get_thread_transcript_impl(thread_id).await
    }

    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        self.merge_thread_with_summary_impl(thread_id, None, None)
            .await
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.merge_thread_with_summary_impl(source_id, target_id, summary)
            .await
    }

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        let threads = self.list_threads_impl().await?;
        let transcript = self.get_transcript_impl().await?;
        Ok(SessionSnapshot {
            node_count: transcript.len(),
            thread_count: threads.len(),
            threads,
        })
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        self.runtime.compute_thread_metadata()
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        self.delete_thread_impl(thread_id).await
    }
}

#[async_trait]
impl AgentTurnPort for InProcessEngine {
    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        self.send_turn_impl(prompt).await
    }

    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        self.send_turn_for_thread_impl(thread_id, prompt).await
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        self.resolve_permission_impl(decision).await
    }

    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        self.cancel_turn_impl(thread_id).await
    }

    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        self.agent_metadata_impl().await
    }

    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        self.agent_options_impl(thread_id).await
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        self.set_agent_config_impl(thread_id, config_id, value, set_default)
            .await
    }

    async fn list_agent_providers(&self) -> Result<Vec<AgentProviderInfo>, EngineError> {
        self.list_agent_providers_impl().await
    }

    async fn switch_thread_provider(
        &self,
        thread_id: &str,
        provider: &str,
    ) -> Result<String, EngineError> {
        self.switch_thread_provider_impl(thread_id, provider).await
    }
}

#[async_trait]
impl ShellUseCasePort for InProcessEngine {
    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        self.run_shell_command_impl(thread_id, command).await
    }
}

#[async_trait]
impl TerminalPtyUseCasePort for InProcessEngine {
    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        self.create_terminal_impl(parent_thread_id, title, cwd, rows, cols)
            .await
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        self.pty_write_impl(thread_id, bytes).await
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        self.pty_resize_impl(thread_id, rows, cols).await
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.pty_snapshot_impl(thread_id, rows, cols).await
    }

    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.pty_scroll_impl(thread_id, delta).await
    }

    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.pty_restart_impl(thread_id, rows, cols).await
    }
}

#[async_trait]
impl WorktreeUseCasePort for InProcessEngine {
    async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        self.ensure_thread_worktree_impl(thread_id).await
    }

    async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        self.list_worktrees_impl().await
    }

    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        self.switch_thread_worktree_impl(thread_id, target_path)
            .await
    }

    async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), EngineError> {
        self.remove_worktree_impl(path, force).await
    }

    async fn worktree_diff_preview(&self, path: &str) -> Result<String, EngineError> {
        self.worktree_diff_preview_impl(path).await
    }

    async fn prune_worktrees(&self) -> Result<String, EngineError> {
        self.prune_worktrees_impl().await
    }
}

#[async_trait]
impl AutocompleteContextPort for InProcessEngine {
    async fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, EngineError> {
        self.list_recent_user_prompts_impl(limit).await
    }

    async fn list_workspace_files(&self, root: Option<&str>) -> Result<Vec<String>, EngineError> {
        self.list_workspace_files_impl(root).await
    }
}

#[async_trait]
impl EngineEventPort for InProcessEngine {
    async fn ping(&self) -> Result<(), EngineError> {
        self.runtime.lock().map(|_| ())
    }

    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<EngineStreamEvent>>, EngineError> {
        Ok(Some(InProcessEngine::subscribe_tui_events(self)))
    }
}
