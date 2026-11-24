use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use grein_tui::engine_port::{
    AgentOptionsSnapshot, PtyScreenSnapshot as TuiPtyScreenSnapshot, ThreadMeta, TuiEngineEvent,
    WorktreeEntry,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use super::rpc::to_json;
use super::types::{
    AgentMetaEntry, DaemonEngine, EngineError, EngineEvent, PermissionDecisionPayload,
    SessionSnapshot, ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};

struct SocketIo {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

/// Remote engine speaking newline-delimited JSON-RPC 2.0 over a Unix socket.
pub struct UnixSocketEngineClient {
    socket_path: PathBuf,
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

    fn is_optional_autocomplete_rpc(method: &str) -> bool {
        matches!(
            method,
            "session.list_recent_prompts"
                | "list_recent_prompts"
                | "session.list_workspace_files"
                | "list_workspace_files"
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
            if code == Some(-32601) && Self::is_optional_autocomplete_rpc(method) {
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

    async fn call(&self, method: &str, params: Value) -> Result<Value, EngineError> {
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

    async fn call_dedicated(&self, method: &str, params: Value) -> Result<Value, EngineError> {
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

    async fn call_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, EngineError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
    }

    async fn call_dedicated_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, EngineError> {
        let value = self.call_dedicated(method, params).await?;
        serde_json::from_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
    }

    pub async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        DaemonEngine::thread_metadata(self).await
    }

    pub async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        DaemonEngine::delete_thread(self, thread_id).await
    }

    pub async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        DaemonEngine::agent_options(self, thread_id).await
    }

    pub async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        DaemonEngine::set_agent_config(self, thread_id, config_id, value, set_default).await
    }

    pub async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::create_terminal(self, parent_thread_id, title, cwd, rows, cols).await
    }

    pub async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        DaemonEngine::pty_write(self, thread_id, bytes).await
    }

    pub async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        DaemonEngine::pty_resize(self, thread_id, rows, cols).await
    }

    pub async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_snapshot(self, thread_id, rows, cols).await
    }

    pub async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        DaemonEngine::ensure_thread_worktree(self, thread_id).await
    }

    pub async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        DaemonEngine::list_worktrees(self).await
    }

    pub async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        DaemonEngine::switch_thread_worktree(self, thread_id, target_path).await
    }

    pub async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), EngineError> {
        DaemonEngine::remove_worktree(self, path, force).await
    }

    pub async fn worktree_diff_preview(&self, path: &str) -> Result<String, EngineError> {
        DaemonEngine::worktree_diff_preview(self, path).await
    }

    pub async fn prune_worktrees(&self) -> Result<String, EngineError> {
        DaemonEngine::prune_worktrees(self).await
    }

    pub async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_scroll(self, thread_id, delta).await
    }

    pub async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        DaemonEngine::pty_restart(self, thread_id, rows, cols).await
    }

    pub async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::create_root_thread_in_cwd(self, title, cwd).await
    }

    pub async fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, EngineError> {
        DaemonEngine::list_recent_user_prompts(self, limit).await
    }

    pub async fn list_workspace_files(&self, root: Option<&str>) -> Result<Vec<String>, EngineError> {
        DaemonEngine::list_workspace_files(self, root).await
    }
}

#[async_trait]
impl DaemonEngine for UnixSocketEngineClient {
    async fn ping(&self) -> Result<(), EngineError> {
        self.call("session.ping", json!({})).await.map(|_| ())
    }

    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        self.call_typed("session.ensure_main_thread", json!({}))
            .await
    }

    async fn send_turn(&self, prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        self.call_typed("session.send_turn", json!({"prompt": prompt}))
            .await
    }

    async fn send_turn_for_thread(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        self.call_typed(
            "session.send_thread_turn",
            json!({"thread_id": thread_id, "prompt": prompt}),
        )
        .await
    }

    async fn fork_thread(
        &self,
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.fork_thread_with_title(parent_node_id, None).await
    }

    async fn fork_thread_with_title(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(source) = source_thread_or_node {
            params.insert("parent_node_id".to_string(), json!(source));
        }
        if let Some(title) = title {
            params.insert("title".to_string(), json!(title));
        }
        self.call_typed("session.fork_thread", Value::Object(params))
            .await
    }

    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        DaemonEngine::create_root_thread_in_cwd(self, title, None).await
    }

    async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(title) = title {
            params.insert("title".to_string(), json!(title));
        }
        if let Some(cwd) = cwd {
            params.insert("cwd".to_string(), json!(cwd));
        }
        self.call_dedicated_typed("session.create_root_thread", Value::Object(params))
            .await
    }

    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        self.call_dedicated_typed(
            "session.rename_thread",
            json!({"thread_id": thread_id, "title": title}),
        )
        .await
    }

    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        self.call_typed("session.list_threads", json!({})).await
    }

    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.call_typed("session.get_transcript", json!({})).await
    }

    async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        self.call_typed(
            "session.get_transcript",
            json!({"thread_id": thread_id}),
        )
        .await
    }

    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        self.merge_thread_with_summary(thread_id, None, None).await
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        params.insert("thread_id".to_string(), json!(source_id));
        if let Some(target_id) = target_id {
            params.insert("target_thread_id".to_string(), json!(target_id));
        }
        if let Some(summary) = summary {
            params.insert("summary".to_string(), json!(summary));
        }
        self.call_typed("session.merge_thread", Value::Object(params))
            .await
    }

    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        self.call_dedicated_typed("permissions.resolve", to_json(&decision)?)
            .await
    }

    async fn cancel_turn(&self, thread_id: &str) -> Result<(), EngineError> {
        self.call_dedicated("session.cancel", json!({"thread_id": thread_id}))
            .await
            .map(|_| ())
    }

    async fn run_shell_command(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        self.call_typed(
            "session.shell",
            json!({"thread_id": thread_id, "command": command}),
        )
        .await
    }

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        let threads = self.list_threads().await?;
        let transcript = self.get_transcript().await?;
        Ok(SessionSnapshot {
            node_count: transcript.len(),
            thread_count: threads.len(),
            threads,
        })
    }

    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        let (read, mut writer) = stream.into_split();
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "session.subscribe_events", "params": {}});
        writer
            .write_all(format!("{req}\n").as_bytes())
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;

        let mut reader = BufReader::new(read);
        let mut ack = String::new();
        let n = reader
            .read_line(&mut ack)
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        let ack_val: Value =
            serde_json::from_str(ack.trim()).map_err(|e| EngineError::Protocol(e.to_string()))?;
        if ack_val.get("error").is_some() {
            return Ok(None);
        }

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let _keep_writer = writer;
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Ok(event) = serde_json::from_str::<TuiEngineEvent>(trimmed) {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        Ok(Some(rx))
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        self.call_dedicated_typed("session.thread_metadata", json!({}))
            .await
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        let val = self
            .call_dedicated("session.delete_thread", json!({"thread_id": thread_id}))
            .await?;
        val.get("next_active_thread_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing next_active_thread_id".into()))
    }

    async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        self.call_typed("session.agent_metadata", json!({})).await
    }

    async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
        self.call_typed("session.agent_options", json!({"thread_id": thread_id}))
            .await
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        self.call_typed(
            "session.set_agent_config",
            json!({
                "thread_id": thread_id,
                "config_id": config_id,
                "value": value,
                "set_default": set_default,
            }),
        )
        .await
    }

    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(pid) = parent_thread_id {
            params.insert("parent_thread_id".to_string(), json!(pid));
        }
        if let Some(t) = title {
            params.insert("title".to_string(), json!(t));
        }
        if let Some(dir) = cwd {
            params.insert("cwd".to_string(), json!(dir));
        }
        params.insert("rows".to_string(), json!(rows));
        params.insert("cols".to_string(), json!(cols));
        self.call_dedicated_typed("session.create_terminal", Value::Object(params))
            .await
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> Result<(), EngineError> {
        self.call_dedicated(
            "session.pty_write",
            json!({"thread_id": thread_id, "bytes": bytes}),
        )
        .await
        .map(|_| ())
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> Result<(), EngineError> {
        self.call_dedicated(
            "session.pty_resize",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
        .map(|_| ())
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_snapshot",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
    }

    async fn ensure_thread_worktree(&self, thread_id: &str) -> Result<String, EngineError> {
        let val = self
            .call_dedicated(
                "session.ensure_thread_worktree",
                json!({"thread_id": thread_id}),
            )
            .await?;
        val.get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing cwd".into()))
    }

    async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        self.call_dedicated_typed("session.list_worktrees", json!({}))
            .await
    }

    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        let val = self
            .call_dedicated(
                "session.switch_thread_worktree",
                json!({"thread_id": thread_id, "target_path": target_path}),
            )
            .await?;
        val.get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing cwd".into()))
    }

    async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), EngineError> {
        self.call_dedicated(
            "session.remove_worktree",
            json!({"path": path, "force": force}),
        )
        .await
        .map(|_| ())
    }

    async fn worktree_diff_preview(&self, path: &str) -> Result<String, EngineError> {
        let val = self
            .call_dedicated("session.worktree_diff_preview", json!({"path": path}))
            .await?;
        val.get("diff")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing diff".into()))
    }

    async fn prune_worktrees(&self) -> Result<String, EngineError> {
        let val = self
            .call_dedicated("session.prune_worktrees", json!({}))
            .await?;
        val.get("summary")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing summary".into()))
    }

    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_scroll",
            json!({"thread_id": thread_id, "delta": delta}),
        )
        .await
    }

    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_restart",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
    }

    async fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, EngineError> {
        self.call_dedicated_typed("session.list_recent_prompts", json!({"limit": limit}))
            .await
    }

    async fn list_workspace_files(&self, root: Option<&str>) -> Result<Vec<String>, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(root) = root {
            params.insert("root".to_string(), json!(root));
        }
        self.call_dedicated_typed("session.list_workspace_files", Value::Object(params))
            .await
    }
}
