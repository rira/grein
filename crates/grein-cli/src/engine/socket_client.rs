use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use grein_core::port::dto::{
    AgentOptionsSnapshot, AgentProviderInfo, EngineStreamEvent, PtyScreenSnapshot, ThreadMeta,
    WorktreeEntry,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use super::ports::{
    AgentTurnPort, AutocompleteContextPort, EngineEventPort, ShellUseCasePort,
    TerminalPtyUseCasePort, ThreadUseCasePort, WorktreeUseCasePort,
};
use super::types::{
    AgentMetaEntry, EngineError, EngineEvent, PermissionDecisionPayload, SessionSnapshot,
    ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};

struct SocketIo {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

/// Remote engine speaking newline-delimited JSON-RPC 2.0 over a Unix socket.
pub struct UnixSocketEngineClient {
    pub(super) socket_path: PathBuf,
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
                | "create_terminal"
                | "session.pty_write"
                | "pty_write"
                | "session.pty_resize"
                | "pty_resize"
                | "session.pty_snapshot"
                | "pty_snapshot"
                | "session.pty_scroll"
                | "pty_scroll"
                | "session.pty_restart"
                | "pty_restart"
                | "session.ensure_thread_worktree"
                | "ensure_thread_worktree"
                | "session.list_worktrees"
                | "list_worktrees"
                | "session.switch_thread_worktree"
                | "switch_thread_worktree"
                | "session.remove_worktree"
                | "remove_worktree"
                | "session.worktree_diff_preview"
                | "worktree_diff_preview"
                | "session.prune_worktrees"
                | "prune_worktrees"
        )
    }

    fn is_optional_stale_empty_list_rpc(method: &str) -> bool {
        matches!(
            method,
            "session.list_recent_prompts"
                | "list_recent_prompts"
                | "session.list_workspace_files"
                | "list_workspace_files"
                | "session.list_agent_providers"
                | "list_agent_providers"
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
            if code == Some(-32601) && Self::is_optional_stale_empty_list_rpc(method) {
                return Ok(json!([]));
            }
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

    pub(super) async fn call(&self, method: &str, params: Value) -> Result<Value, EngineError> {
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

    pub(super) async fn call_dedicated(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, EngineError> {
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

    pub(super) async fn call_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, EngineError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
    }

    pub(super) async fn call_dedicated_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, EngineError> {
        let value = self.call_dedicated(method, params).await?;
        serde_json::from_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
    }

    pub async fn ping(&self) -> Result<(), EngineError> {
        EngineEventPort::ping(self).await
    }

    pub async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<EngineStreamEvent>>, EngineError> {
        EngineEventPort::subscribe_events(self).await
    }

    pub async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::ensure_main_thread(self).await
    }

    pub async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        AgentTurnPort::send_turn(self, prompt).await
    }

    pub async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        AgentTurnPort::send_turn_for_thread(self, thread_id, prompt).await
    }

    pub async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::fork_thread(self, parent_node_id).await
    }

    pub async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::fork_thread_with_title(self, source_thread_or_node, title).await
    }

    pub async fn fork_thread_with_provider(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
        agent_provider: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::fork_thread_with_provider(
            self,
            source_thread_or_node,
            title,
            agent_provider,
        )
        .await
    }

    pub async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::create_root_thread(self, title).await
    }

    pub async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::create_root_thread_in_cwd(self, title, cwd).await
    }

    pub async fn create_agent_thread(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
        parent_thread_id: Option<&str>,
        agent_provider: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::create_agent_thread(self, title, cwd, parent_thread_id, agent_provider)
            .await
    }

    pub async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::rename_thread(self, thread_id, title).await
    }

    pub async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        ThreadUseCasePort::list_threads(self).await
    }

    pub async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        ThreadUseCasePort::get_transcript(self).await
    }

    pub async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        ThreadUseCasePort::get_thread_transcript(self, thread_id).await
    }

    pub async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::merge_thread(self, thread_id).await
    }

    pub async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::merge_thread_with_summary(self, source_id, target_id, summary).await
    }

    pub async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        ThreadUseCasePort::snapshot(self).await
    }

    pub async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        AgentTurnPort::resolve_permission(self, decision).await
    }

    pub async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        AgentTurnPort::cancel_turn(self, thread_id).await
    }

    pub async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        ShellUseCasePort::run_shell_command(self, thread_id, command).await
    }

    pub async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        AgentTurnPort::agent_metadata(self).await
    }

    pub async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        ThreadUseCasePort::thread_metadata(self).await
    }

    pub async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        ThreadUseCasePort::delete_thread(self, thread_id).await
    }

    pub async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        AgentTurnPort::agent_options(self, thread_id).await
    }

    pub async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        AgentTurnPort::set_agent_config(self, thread_id, config_id, value, set_default).await
    }

    pub async fn list_agent_providers(&self) -> Result<Vec<AgentProviderInfo>, EngineError> {
        AgentTurnPort::list_agent_providers(self).await
    }

    pub async fn switch_thread_provider(
        &self,
        thread_id: &str,
        provider: &str,
    ) -> Result<String, EngineError> {
        AgentTurnPort::switch_thread_provider(self, thread_id, provider).await
    }

    pub async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        TerminalPtyUseCasePort::create_terminal(self, parent_thread_id, title, cwd, rows, cols)
            .await
    }

    pub async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        TerminalPtyUseCasePort::pty_write(self, thread_id, bytes).await
    }

    pub async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        TerminalPtyUseCasePort::pty_resize(self, thread_id, rows, cols).await
    }

    pub async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        TerminalPtyUseCasePort::pty_snapshot(self, thread_id, rows, cols).await
    }

    pub async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        WorktreeUseCasePort::ensure_thread_worktree(self, thread_id).await
    }

    pub async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        WorktreeUseCasePort::list_worktrees(self).await
    }

    pub async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        WorktreeUseCasePort::switch_thread_worktree(self, thread_id, target_path).await
    }

    pub async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), EngineError> {
        WorktreeUseCasePort::remove_worktree(self, path, force).await
    }

    pub async fn worktree_diff_preview(&self, path: &str) -> Result<String, EngineError> {
        WorktreeUseCasePort::worktree_diff_preview(self, path).await
    }

    pub async fn prune_worktrees(&self) -> Result<String, EngineError> {
        WorktreeUseCasePort::prune_worktrees(self).await
    }

    pub async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        TerminalPtyUseCasePort::pty_scroll(self, thread_id, delta).await
    }

    pub async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        TerminalPtyUseCasePort::pty_restart(self, thread_id, rows, cols).await
    }

    pub async fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, EngineError> {
        AutocompleteContextPort::list_recent_user_prompts(self, limit).await
    }

    pub async fn list_workspace_files(&self, root: Option<&str>) -> Result<Vec<String>, EngineError> {
        AutocompleteContextPort::list_workspace_files(self, root).await
    }
}
