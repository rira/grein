use async_trait::async_trait;
use grein_core::port::dto::{
    AgentOptionsSnapshot, AgentProviderInfo, EngineStreamEvent, PtyScreenSnapshot, ThreadMeta,
    WorktreeEntry,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use super::ports::{
    AgentTurnPort, AutocompleteContextPort, EngineEventPort, ShellUseCasePort,
    TerminalPtyUseCasePort, ThreadUseCasePort, WorktreeUseCasePort,
};
use super::rpc::to_json;
use super::socket_client::UnixSocketEngineClient;
use super::types::{
    AgentMetaEntry, EngineError, EngineEvent, PermissionDecisionPayload, SessionSnapshot,
    ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};

#[async_trait]
impl ThreadUseCasePort for UnixSocketEngineClient {
    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        self.call_typed("session.ensure_main_thread", json!({}))
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
        self.fork_thread_with_provider(source_thread_or_node, title, None)
            .await
    }

    async fn fork_thread_with_provider(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
        agent_provider: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(source) = source_thread_or_node {
            params.insert("parent_node_id".to_string(), json!(source));
        }
        if let Some(title) = title {
            params.insert("title".to_string(), json!(title));
        }
        if let Some(provider) = agent_provider {
            params.insert("agent_provider".to_string(), json!(provider));
        }
        self.call_typed("session.fork_thread", Value::Object(params))
            .await
    }

    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::create_root_thread_in_cwd(self, title, None).await
    }

    async fn create_root_thread_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        ThreadUseCasePort::create_agent_thread(self, title, cwd, None, None).await
    }

    async fn create_agent_thread(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
        parent_thread_id: Option<&str>,
        agent_provider: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let mut params = serde_json::Map::new();
        if let Some(title) = title {
            params.insert("title".to_string(), json!(title));
        }
        if let Some(cwd) = cwd {
            params.insert("cwd".to_string(), json!(cwd));
        }
        if let Some(pid) = parent_thread_id {
            params.insert("parent_thread_id".to_string(), json!(pid));
        }
        if let Some(provider) = agent_provider {
            params.insert("agent_provider".to_string(), json!(provider));
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

    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        let threads = self.list_threads().await?;
        let transcript = self.get_transcript().await?;
        Ok(SessionSnapshot {
            node_count: transcript.len(),
            thread_count: threads.len(),
            threads,
        })
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
}

#[async_trait]
impl AgentTurnPort for UnixSocketEngineClient {
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

    async fn list_agent_providers(&self) -> Result<Vec<AgentProviderInfo>, EngineError> {
        self.call_dedicated_typed("session.list_agent_providers", json!({}))
            .await
    }

    async fn switch_thread_provider(
        &self,
        thread_id: &str,
        provider: &str,
    ) -> Result<String, EngineError> {
        let val = self
            .call_dedicated(
                "session.switch_thread_provider",
                json!({"thread_id": thread_id, "provider": provider}),
            )
            .await?;
        val.get("provider")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("response missing provider".into()))
    }
}

#[async_trait]
impl ShellUseCasePort for UnixSocketEngineClient {
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
}

#[async_trait]
impl TerminalPtyUseCasePort for UnixSocketEngineClient {
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
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_snapshot",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
    }

    async fn pty_scroll(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<PtyScreenSnapshot, EngineError> {
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
    ) -> Result<PtyScreenSnapshot, EngineError> {
        self.call_dedicated_typed(
            "session.pty_restart",
            json!({"thread_id": thread_id, "rows": rows, "cols": cols}),
        )
        .await
    }
}

#[async_trait]
impl WorktreeUseCasePort for UnixSocketEngineClient {
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
}

#[async_trait]
impl AutocompleteContextPort for UnixSocketEngineClient {
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

#[async_trait]
impl EngineEventPort for UnixSocketEngineClient {
    async fn ping(&self) -> Result<(), EngineError> {
        self.call("session.ping", json!({})).await.map(|_| ())
    }

    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<EngineStreamEvent>>, EngineError> {
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
                        if let Ok(event) = serde_json::from_str::<EngineStreamEvent>(trimmed) {
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
}
