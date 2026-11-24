use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grein_acp::{
    AcpPermissionOption, AgentDriver, AgentDriverFactory, PermissionPolicyPort,
    StdioAgentDriverFactory, ToolInvocationRequest,
};
use grein_core::model::{AuthorRole, ConversationNode, NodeId, ThreadId};
use grein_core::port::dto::{
    AgentConfigOption, AgentMetaEntry, AgentOptionsSnapshot, AgentSlashCommand, EngineStreamEvent,
    PtyScreenSnapshot, SubagentSnapshot, ThreadMeta, WorktreeEntry,
};
use grein_core::port::outbound::SessionRepository;
use grein_core::storage::sqlite::SqliteStorage;
use grein_workspace::port::{
    GitCliInspector, GitInspectorPort, GitWorktreeDriver, PtyShellExecutor, PtySpawnerPort,
    ShellExecutorPort, TerminalPtyHandle, UnixPtySpawner, WorkspaceIsolationPort,
};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

use super::ports::{
    AgentTurnPort, AutocompleteContextPort, EngineEventPort, ShellUseCasePort,
    TerminalPtyUseCasePort, ThreadUseCasePort, WorktreeUseCasePort,
};
use super::types::{
    storage_err, AcpBackendConfig, EngineError, EngineEvent, PermissionDecisionPayload,
    SessionSnapshot, ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
    UNSOLICITED_QUIET_TIMEOUT,
};
use crate::subagent_watch::{SubagentObserverPort, SubagentWatcher};

mod acp;
mod acp_turns;
mod listener;
mod ports_impl;
mod pty;
#[cfg(test)]
mod tests_listener;
mod threads;
mod worktree;

pub struct EngineDependencies {
    pub storage: Arc<dyn SessionRepository>,
    pub agent_factory: Arc<dyn AgentDriverFactory>,
    pub policy: Option<Arc<dyn PermissionPolicyPort>>,
    pub pty_spawner: Arc<dyn PtySpawnerPort>,
    pub workspace: Arc<dyn WorkspaceIsolationPort>,
    pub git_inspector: Arc<dyn GitInspectorPort>,
    pub shell: Arc<dyn ShellExecutorPort>,
    pub subagent_observer: Arc<dyn SubagentObserverPort>,
    pub cwd: PathBuf,
    pub acp_config: Option<AcpBackendConfig>,
}

impl EngineDependencies {
    pub fn for_storage(
        storage: Arc<dyn SessionRepository>,
        cwd: PathBuf,
        acp_config: Option<AcpBackendConfig>,
    ) -> Self {
        let brain_root = SubagentWatcher::default_brain_root()
            .unwrap_or_else(|| cwd.join(".grein").join("brain_missing"));
        Self {
            storage,
            agent_factory: Arc::new(StdioAgentDriverFactory::new()),
            policy: None,
            pty_spawner: Arc::new(UnixPtySpawner::new()),
            workspace: Arc::new(GitWorktreeDriver::new()),
            git_inspector: Arc::new(GitCliInspector::new()),
            shell: Arc::new(PtyShellExecutor::new()),
            subagent_observer: SubagentWatcher::unattached(brain_root),
            cwd,
            acp_config,
        }
    }
}

#[allow(clippy::enum_variant_names)]
pub(super) enum SessionListenerCmd {
    PrepareSolicited {
        reply_tx: oneshot::Sender<NodeId>,
    },
    StartSolicited {
        user_node_id: NodeId,
        done_tx: oneshot::Sender<Result<NodeId, EngineError>>,
        ack_tx: oneshot::Sender<()>,
    },
    AbortSolicited {
        ack_tx: oneshot::Sender<bool>,
    },
}

pub(super) struct ActiveAcpSession {
    pub(super) session_id: String,
    pub(super) driver_gen: u64,
    pub(super) model: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) config_options: Vec<AgentConfigOption>,
    pub(super) available_commands: Vec<AgentSlashCommand>,
    pub(super) has_legacy_model: bool,
    pub(super) has_legacy_mode: bool,
    pub(super) pending_landmarks: Vec<String>,
    pub(super) cmd_tx: mpsc::UnboundedSender<SessionListenerCmd>,
    pub(super) listener_handle: tokio::task::JoinHandle<()>,
}

impl Drop for ActiveAcpSession {
    fn drop(&mut self) {
        self.listener_handle.abort();
    }
}

#[derive(Clone)]
pub(super) struct ActiveSessionHandle {
    pub(super) session_id: String,
    pub(super) driver_gen: u64,
    pub(super) cmd_tx: mpsc::UnboundedSender<SessionListenerCmd>,
}

pub(super) struct PendingPermissionRecord {
    pub(super) driver: Arc<dyn AgentDriver>,
    pub(super) json_rpc_id: Value,
    pub(super) options: Vec<AcpPermissionOption>,
    pub(super) session_id: String,
    pub(super) thread_id: String,
    pub(super) request: ToolInvocationRequest,
}

pub(super) struct DriverSlot {
    pub(super) driver: Arc<dyn AgentDriver>,
    pub(super) generation: u64,
}

#[derive(Default)]
pub(super) struct EngineState {
    pub(super) last_node: Option<NodeId>,
    pub(super) decisions: Vec<PermissionDecisionPayload>,
    pub(super) active_acp_sessions: HashMap<String, ActiveAcpSession>,
    pub(super) pending_permissions: HashMap<String, PendingPermissionRecord>,
    pub(super) thread_turn_locks: HashMap<String, Arc<AsyncMutex<()>>>,
    pub(super) default_config: HashMap<String, String>,
    pub(super) pending_thread_config: HashMap<String, HashMap<String, String>>,
}

pub(super) struct ConfigTarget<'a> {
    pub(super) config_id: &'a str,
    pub(super) value: &'a str,
    pub(super) is_model_hint: bool,
    pub(super) is_mode_hint: bool,
}

#[derive(Clone)]
pub(super) struct TrackedLayer1Subagent {
    pub(super) snapshot: SubagentSnapshot,
    pub(super) launched_turn_seq: u64,
    pub(super) launched_unsolicited: bool,
    pub(super) launched_wakeup_seq: u64,
}

pub(in crate::engine) struct EngineRuntime {
    pub(in crate::engine) storage: Arc<dyn SessionRepository>,
    pub(super) agent_factory: Arc<dyn AgentDriverFactory>,
    pub(super) policy: Option<Arc<dyn PermissionPolicyPort>>,
    pub(super) pty_spawner: Arc<dyn PtySpawnerPort>,
    pub(super) workspace: Arc<dyn WorkspaceIsolationPort>,
    pub(super) git_inspector: Arc<dyn GitInspectorPort>,
    pub(super) shell: Arc<dyn ShellExecutorPort>,
    pub(super) subagent_observer: Arc<dyn SubagentObserverPort>,
    pub(super) cwd: PathBuf,
    pub(super) acp_config: Option<AcpBackendConfig>,
    pub(super) acp_driver: AsyncMutex<Option<DriverSlot>>,
    pub(super) driver_gen_seq: AtomicU64,
    pub(super) state: Mutex<EngineState>,
    pub(super) tui_subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<EngineStreamEvent>>>>,
    pub(super) next_perm_seq: AtomicU64,
    pub(super) unsolicited_quiet_timeout: Mutex<Duration>,
    pub(super) active_pty_sessions: Arc<Mutex<HashMap<String, Arc<dyn TerminalPtyHandle>>>>,
    pub(in crate::engine) pending_pty_sizes: Arc<Mutex<HashMap<String, (u16, u16)>>>,
    pub(super) pty_exited_threads: Mutex<HashSet<String>>,
    pub(super) pty_notify_tx: mpsc::UnboundedSender<String>,
    pub(super) pty_notify_rx: Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    pub(super) pty_notifier_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl EngineRuntime {
    pub(super) fn emit_tui(&self, event: EngineStreamEvent) {
        if let Ok(mut subs) = self.tui_subscribers.lock() {
            subs.retain(|tx| tx.send(event.clone()).is_ok());
        }
    }

    pub(super) fn lock(&self) -> Result<std::sync::MutexGuard<'_, EngineState>, EngineError> {
        self.state
            .lock()
            .map_err(|_| EngineError::Storage("engine state poisoned".to_string()))
    }

    pub(super) async fn acquire_thread_turn_lock(
        &self,
        thread_id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, EngineError> {
        let mutex = {
            let mut state = self.lock()?;
            Arc::clone(
                state
                    .thread_turn_locks
                    .entry(thread_id.to_string())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        Ok(mutex.lock_owned().await)
    }

    pub(super) fn quiet_timeout(&self) -> Duration {
        *self
            .unsolicited_quiet_timeout
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn mark_thread_error(&self, tid: ThreadId, thread_id: &str, message: &str) {
        if self.storage.get_thread(&tid).ok().flatten().is_none() {
            return;
        }
        let _ = self
            .storage
            .update_thread_status(&tid, grein_core::model::ThreadStatus::Error);
        self.emit_tui(EngineStreamEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "error".to_string(),
        });
        self.emit_tui(EngineStreamEvent::Error {
            message: message.to_string(),
        });
    }

    pub(super) fn persist_assistant_node(
        &self,
        tid: ThreadId,
        parent_leaf: NodeId,
        content: String,
    ) -> Result<NodeId, EngineError> {
        let asst_node_id = NodeId::new();
        let asst_node = ConversationNode::new(
            asst_node_id,
            Some(parent_leaf),
            AuthorRole::Assistant,
            content,
        );
        self.storage.insert_node(&asst_node).map_err(storage_err)?;
        self.storage
            .update_thread_leaf(&tid, &asst_node_id)
            .map_err(storage_err)?;
        if let Ok(mut state) = self.state.lock() {
            state.last_node = Some(asst_node_id);
        }
        Ok(asst_node_id)
    }
}

/// Engine backed by constructor-injected hexagonal outbound ports.
pub struct InProcessEngine {
    pub(in crate::engine) runtime: Arc<EngineRuntime>,
}

impl Drop for InProcessEngine {
    fn drop(&mut self) {
        if let Ok(mut state) = self.runtime.state.lock() {
            state.active_acp_sessions.clear();
        }
        if let Ok(mut handle) = self.runtime.pty_notifier_handle.lock() {
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
        let sessions: Vec<Arc<dyn TerminalPtyHandle>> = self
            .runtime
            .active_pty_sessions
            .lock()
            .map(|mut map| map.drain().map(|(_, s)| s).collect())
            .unwrap_or_default();
        drop(sessions);
        self.runtime.subagent_observer.shutdown();
    }
}

impl InProcessEngine {
    pub fn with_dependencies(deps: EngineDependencies) -> Result<Self, EngineError> {
        let nodes = deps.storage.list_nodes().map_err(storage_err)?;
        let threads = deps.storage.list_threads().map_err(storage_err)?;
        let last_node = threads
            .first()
            .map(|t| t.active_leaf_id)
            .or_else(|| nodes.last().map(|n| n.id));

        let tui_subscribers =
            Arc::new(Mutex::new(Vec::<mpsc::UnboundedSender<EngineStreamEvent>>::new()));
        let subs_for_watch = Arc::clone(&tui_subscribers);
        deps.subagent_observer.attach_emitter(Arc::new(move |ev| {
            if let Ok(mut subs) = subs_for_watch.lock() {
                subs.retain(|tx| tx.send(ev.clone()).is_ok());
            }
        }));

        let (pty_notify_tx, pty_notify_rx) = mpsc::unbounded_channel();

        Ok(Self {
            runtime: Arc::new(EngineRuntime {
                storage: deps.storage,
                agent_factory: deps.agent_factory,
                policy: deps.policy,
                pty_spawner: deps.pty_spawner,
                workspace: deps.workspace,
                git_inspector: deps.git_inspector,
                shell: deps.shell,
                subagent_observer: deps.subagent_observer,
                cwd: deps.cwd,
                acp_config: deps.acp_config,
                acp_driver: AsyncMutex::new(None),
                driver_gen_seq: AtomicU64::new(1),
                state: Mutex::new(EngineState {
                    last_node,
                    ..Default::default()
                }),
                tui_subscribers,
                next_perm_seq: AtomicU64::new(1),
                unsolicited_quiet_timeout: Mutex::new(UNSOLICITED_QUIET_TIMEOUT),
                active_pty_sessions: Arc::new(Mutex::new(HashMap::new())),
                pending_pty_sizes: Arc::new(Mutex::new(HashMap::new())),
                pty_exited_threads: Mutex::new(HashSet::new()),
                pty_notify_tx,
                pty_notify_rx: Mutex::new(Some(pty_notify_rx)),
                pty_notifier_handle: Mutex::new(None),
            }),
        })
    }

    pub fn open(db_path: &Path) -> Result<Self, EngineError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let acp_config = AcpBackendConfig::discover_default(cwd.clone());
        Self::open_with_config(db_path, cwd, acp_config)
    }

    pub fn open_with_config(
        db_path: &Path,
        cwd: PathBuf,
        acp_config: Option<AcpBackendConfig>,
    ) -> Result<Self, EngineError> {
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(storage_err)?;
            }
        }
        let storage = SqliteStorage::open(db_path).map_err(storage_err)?;
        Self::from_storage_with_config(storage, cwd, acp_config)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        let storage = SqliteStorage::in_memory().map_err(storage_err)?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::from_storage_with_config(storage, cwd, None)
    }

    pub fn from_storage(storage: SqliteStorage) -> Result<Self, EngineError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::from_storage_with_config(storage, cwd, None)
    }

    pub fn from_storage_with_config(
        storage: SqliteStorage,
        cwd: PathBuf,
        acp_config: Option<AcpBackendConfig>,
    ) -> Result<Self, EngineError> {
        Self::with_dependencies(EngineDependencies::for_storage(
            Arc::new(storage),
            cwd,
            acp_config,
        ))
    }

    #[cfg(test)]
    pub(crate) fn set_test_quiet_timeout(&self, dur: Duration) {
        *self
            .runtime
            .unsolicited_quiet_timeout
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = dur;
    }

    pub fn subscribe_tui_events(&self) -> mpsc::UnboundedReceiver<EngineStreamEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut subs) = self.runtime.tui_subscribers.lock() {
            subs.push(tx);
        }
        rx
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

    pub async fn get_thread_transcript(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        ThreadUseCasePort::get_thread_transcript(self, thread_id).await
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

    pub async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        ThreadUseCasePort::thread_metadata(self).await
    }

    pub async fn delete_thread(&self, thread_id: &str) -> Result<String, EngineError> {
        ThreadUseCasePort::delete_thread(self, thread_id).await
    }

    pub async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        AgentTurnPort::agent_metadata(self).await
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
