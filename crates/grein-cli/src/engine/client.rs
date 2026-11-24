use std::ops::Deref;
use std::sync::Arc;

use async_trait::async_trait;
use grein_core::port::dto::{
    AgentOptionsSnapshot, EngineStreamEvent, PtyScreenSnapshot, ThreadMeta, WorktreeEntry,
};
use tokio::sync::mpsc;

use super::in_process::InProcessEngine;
use super::ports::{
    AgentTurnPort, AutocompleteContextPort, EngineEventPort, GreinEngine, ShellUseCasePort,
    TerminalPtyUseCasePort, ThreadUseCasePort, WorktreeUseCasePort,
};
use super::socket_client::UnixSocketEngineClient;
use super::types::{
    AgentMetaEntry, EngineError, EngineEvent, PermissionDecisionPayload, SessionSnapshot,
    ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};

/// Unified engine facade wrapping any [`GreinEngine`] implementation behind `Arc<dyn GreinEngine>`.
#[derive(Clone)]
pub struct EngineClient {
    inner: Arc<dyn GreinEngine>,
}

impl EngineClient {
    pub fn new(inner: Arc<dyn GreinEngine>) -> Self {
        Self { inner }
    }

    pub fn in_process(engine: impl Into<Arc<InProcessEngine>>) -> Self {
        Self {
            inner: engine.into(),
        }
    }

    pub fn socket(client: impl Into<Arc<UnixSocketEngineClient>>) -> Self {
        Self {
            inner: client.into(),
        }
    }

    pub fn inner(&self) -> &Arc<dyn GreinEngine> {
        &self.inner
    }
}

impl From<Arc<dyn GreinEngine>> for EngineClient {
    fn from(inner: Arc<dyn GreinEngine>) -> Self {
        Self { inner }
    }
}

impl Deref for EngineClient {
    type Target = dyn GreinEngine;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref()
    }
}

#[async_trait]
impl ThreadUseCasePort for EngineClient {
    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        self.inner.ensure_main_thread().await
    }
    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.inner.fork_thread(parent_node_id).await
    }
    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.inner
            .fork_thread_with_title(source_thread_or_node, title)
            .await
    }
    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        self.inner.create_root_thread(title).await
    }
    async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.inner.create_root_thread_in_cwd(title, cwd).await
    }
    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        self.inner.rename_thread(thread_id, title).await
    }
    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        self.inner.list_threads().await
    }
    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.inner.get_transcript().await
    }
    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.inner.get_thread_transcript(thread_id).await
    }
    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        self.inner.merge_thread(thread_id).await
    }
    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.inner
            .merge_thread_with_summary(source_id, target_id, summary)
            .await
    }
    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        self.inner.snapshot().await
    }
    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        self.inner.thread_metadata().await
    }
    async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        self.inner.delete_thread(thread_id).await
    }
}

#[async_trait]
impl AgentTurnPort for EngineClient {
    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        self.inner.send_turn(prompt).await
    }
    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        self.inner.send_turn_for_thread(thread_id, prompt).await
    }
    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        self.inner.resolve_permission(decision).await
    }
    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        self.inner.cancel_turn(thread_id).await
    }
    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        self.inner.agent_metadata().await
    }
    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        self.inner.agent_options(thread_id).await
    }
    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        self.inner
            .set_agent_config(thread_id, config_id, value, set_default)
            .await
    }
}

#[async_trait]
impl ShellUseCasePort for EngineClient {
    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        self.inner.run_shell_command(thread_id, command).await
    }
}

#[async_trait]
impl TerminalPtyUseCasePort for EngineClient {
    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        self.inner
            .create_terminal(parent_thread_id, title, cwd, rows, cols)
            .await
    }
    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        self.inner.pty_write(thread_id, bytes).await
    }
    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        self.inner.pty_resize(thread_id, rows, cols).await
    }
    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.inner.pty_snapshot(thread_id, rows, cols).await
    }
    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.inner.pty_scroll(thread_id, delta).await
    }
    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.inner.pty_restart(thread_id, rows, cols).await
    }
}

#[async_trait]
impl WorktreeUseCasePort for EngineClient {
    async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        self.inner.ensure_thread_worktree(thread_id).await
    }
    async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        self.inner.list_worktrees().await
    }
    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        self.inner
            .switch_thread_worktree(thread_id, target_path)
            .await
    }
    async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), EngineError> {
        self.inner.remove_worktree(path, force).await
    }
    async fn worktree_diff_preview(&self, path: &str) -> Result<String, EngineError> {
        self.inner.worktree_diff_preview(path).await
    }
    async fn prune_worktrees(&self) -> Result<String, EngineError> {
        self.inner.prune_worktrees().await
    }
}

#[async_trait]
impl AutocompleteContextPort for EngineClient {
    async fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, EngineError> {
        self.inner.list_recent_user_prompts(limit).await
    }
    async fn list_workspace_files(&self, root: Option<&str>) -> Result<Vec<String>, EngineError> {
        self.inner.list_workspace_files(root).await
    }
}

#[async_trait]
impl EngineEventPort for EngineClient {
    async fn ping(&self) -> Result<(), EngineError> {
        self.inner.ping().await
    }
    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<EngineStreamEvent>>, EngineError> {
        self.inner.subscribe_events().await
    }
}
