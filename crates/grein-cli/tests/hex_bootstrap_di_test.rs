use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use grein_acp::{
    AcpConfigChoice, AcpConfigOption, AcpInitializeResult, AcpSessionInfo, AcpSpawnConfig,
    AgentDriver, AgentDriverError, AgentDriverFactory, DriverEvent, PermissionDecision,
    PermissionPolicyPort, PolicyDecision, ToolInvocationRequest,
};
use grein_cli::bootstrap::registry::{
    AgentPlugin, FrozenPluginRegistry, LifecyclePlugin, PluginRegistryBuilder, RegistryError,
    WorkspacePlugin,
};
use grein_cli::discovery::AcpAgentKind;
use grein_cli::engine::{
    AcpBackendConfig, AgentTurnPort, AutocompleteContextPort, EngineDependencies,
    InProcessEngine, PermissionDecisionPayload, ShellUseCasePort, TerminalPtyUseCasePort,
    ThreadUseCasePort, WorktreeUseCasePort,
};
use grein_cli::subagent_watch::{EventEmitter, SubagentObserverPort};
use grein_core::blackboard::BlackboardEntry;
use grein_core::dag::DagStorage;
use grein_core::error::{CoreError, Result as CoreResult};
use grein_core::model::{
    Anchor, AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadId, ThreadKind,
    ThreadStatus,
};
use grein_core::port::dto::{
    EngineStreamEvent, PtyScreenSnapshot, ShellCommandResult, SubagentSnapshot, SubagentStatus,
};
use grein_core::port::outbound::SessionRepository;
use grein_tui::clipboard::{
    ChainedClipboardDriver, ClipboardPort, ClipboardReaderBackend, ClipboardWriterBackend,
    Osc52Backend,
};
use grein_workspace::allocator::WorkspaceError;
use grein_workspace::git_status::RepoGitStatus;
use grein_workspace::port::{
    GitInspectorPort, PtySpawnerPort, ShellExecutorPort, TerminalPtyHandle,
    WorkspaceIsolationPort,
};
use grein_workspace::worktrees::DiscoveredWorktree;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::timeout;

type CoreUuid = <NodeId as std::ops::Deref>::Target;

struct StubAgentPlugin {
    id: String,
    kind: AcpAgentKind,
    executable: Option<PathBuf>,
    log: Arc<Mutex<Vec<String>>>,
    fail_start: bool,
    fail_shutdown: bool,
    driver_factory: Arc<dyn AgentDriverFactory>,
    observer: Arc<dyn SubagentObserverPort>,
}

impl StubAgentPlugin {
    fn new(id: &str, available: bool, log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            id: id.to_string(),
            kind: AcpAgentKind::Custom,
            executable: available.then(|| PathBuf::from(format!("/opt/bin/{id}"))),
            log,
            fail_start: false,
            fail_shutdown: false,
            driver_factory: Arc::new(StubAgentDriverFactory::default()),
            observer: Arc::new(StubSubagentObserver::default()),
        }
    }
}

#[async_trait]
impl LifecyclePlugin for StubAgentPlugin {
    fn id(&self) -> &str {
        &self.id
    }

    async fn start(&self) -> Result<(), RegistryError> {
        self.log.lock().unwrap().push(format!("start:{}", self.id));
        if self.fail_start {
            return Err(RegistryError::StartFailed {
                id: self.id.clone(),
                reason: "injected start failure".into(),
            });
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), RegistryError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("shutdown:{}", self.id));
        if self.fail_shutdown {
            return Err(RegistryError::ShutdownFailed {
                id: self.id.clone(),
                reason: "injected shutdown failure".into(),
            });
        }
        Ok(())
    }
}

impl AgentPlugin for StubAgentPlugin {
    fn kind(&self) -> AcpAgentKind {
        self.kind
    }

    fn executable_path(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    fn driver_factory(&self) -> Arc<dyn AgentDriverFactory> {
        Arc::clone(&self.driver_factory)
    }

    fn acp_backend_config(&self, cwd: &Path) -> Option<AcpBackendConfig> {
        self.executable.as_ref().map(|exe| AcpBackendConfig {
            command: exe.to_string_lossy().into_owned(),
            args: vec!["--stdio".to_string()],
            auth_method: None,
            cwd: cwd.to_path_buf(),
        })
    }

    fn subagent_observer(&self) -> Arc<dyn SubagentObserverPort> {
        Arc::clone(&self.observer)
    }
}

struct StubWorkspacePlugin {
    id: String,
    mode: &'static str,
    match_suffix: &'static str,
    log: Arc<Mutex<Vec<String>>>,
    fail_start: bool,
    fail_shutdown: bool,
    isolator: Arc<dyn WorkspaceIsolationPort>,
    inspector: Arc<dyn GitInspectorPort>,
}

impl StubWorkspacePlugin {
    fn new(
        id: &str,
        mode: &'static str,
        match_suffix: &'static str,
        log: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            id: id.to_string(),
            mode,
            match_suffix,
            log,
            fail_start: false,
            fail_shutdown: false,
            isolator: Arc::new(StubWorkspaceIsolator::default()),
            inspector: Arc::new(StubGitInspector::default()),
        }
    }
}

#[async_trait]
impl LifecyclePlugin for StubWorkspacePlugin {
    fn id(&self) -> &str {
        &self.id
    }

    async fn start(&self) -> Result<(), RegistryError> {
        self.log.lock().unwrap().push(format!("start:{}", self.id));
        if self.fail_start {
            return Err(RegistryError::StartFailed {
                id: self.id.clone(),
                reason: "injected workspace start failure".into(),
            });
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), RegistryError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("shutdown:{}", self.id));
        if self.fail_shutdown {
            return Err(RegistryError::ShutdownFailed {
                id: self.id.clone(),
                reason: "injected workspace shutdown failure".into(),
            });
        }
        Ok(())
    }
}

impl WorkspacePlugin for StubWorkspacePlugin {
    fn isolation_mode(&self) -> &'static str {
        self.mode
    }

    fn supports_cwd(&self, cwd: &Path) -> bool {
        cwd.to_string_lossy().ends_with(self.match_suffix)
    }

    fn workspace_isolator(&self) -> Arc<dyn WorkspaceIsolationPort> {
        Arc::clone(&self.isolator)
    }

    fn git_inspector(&self) -> Arc<dyn GitInspectorPort> {
        Arc::clone(&self.inspector)
    }
}

#[test]
fn plugin_registry_builder_validates_ids_and_freezes_lookup_immutably() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut builder = PluginRegistryBuilder::new();

    assert_eq!(
        builder.register_agent_plugin(Arc::new(StubAgentPlugin::new("", true, Arc::clone(&log)))),
        Err(RegistryError::EmptyPluginId)
    );
    assert_eq!(
        builder.register_workspace_plugin(Arc::new(StubWorkspacePlugin::new(
            "   \t ",
            "none",
            "never",
            Arc::clone(&log)
        ))),
        Err(RegistryError::EmptyPluginId)
    );

    builder
        .register_agent_plugin(Arc::new(StubAgentPlugin::new(
            "agent.offline",
            false,
            Arc::clone(&log),
        )))
        .expect("register offline agent");
    builder
        .register_workspace_plugin(Arc::new(StubWorkspacePlugin::new(
            "ws.git",
            "git_worktree",
            "git-project",
            Arc::clone(&log),
        )))
        .expect("register git workspace");
    builder
        .register_agent_plugin(Arc::new(StubAgentPlugin::new(
            "agent.primary",
            true,
            Arc::clone(&log),
        )))
        .expect("register primary agent");
    builder
        .register_workspace_plugin(Arc::new(StubWorkspacePlugin::new(
            "ws.shadow",
            "shadow_copy",
            "plain-project",
            Arc::clone(&log),
        )))
        .expect("register shadow workspace");

    assert_eq!(
        builder.register_agent_plugin(Arc::new(StubAgentPlugin::new(
            "agent.primary",
            true,
            Arc::clone(&log)
        ))),
        Err(RegistryError::DuplicatePluginId("agent.primary".into()))
    );
    assert_eq!(
        builder.register_workspace_plugin(Arc::new(StubWorkspacePlugin::new(
            "  ws.git  ",
            "dup",
            "x",
            Arc::clone(&log)
        ))),
        Err(RegistryError::DuplicatePluginId("ws.git".into()))
    );
    assert_eq!(
        builder.register_workspace_plugin(Arc::new(StubWorkspacePlugin::new(
            "agent.offline",
            "cross-kind-dup",
            "x",
            Arc::clone(&log)
        ))),
        Err(RegistryError::DuplicatePluginId("agent.offline".into()))
    );

    let frozen: FrozenPluginRegistry = builder.freeze();
    assert_eq!(
        frozen.plugin_ids(),
        vec!["agent.offline", "ws.git", "agent.primary", "ws.shadow"]
    );
    assert_eq!(frozen.agent_plugins().len(), 2);
    assert_eq!(frozen.workspace_plugins().len(), 2);

    assert_eq!(
        frozen.agent_plugin("agent.primary").map(|p| p.id()),
        Some("agent.primary")
    );
    assert!(frozen.agent_plugin("ws.git").is_none());
    assert_eq!(
        frozen
            .workspace_plugin("ws.shadow")
            .map(|p| p.isolation_mode()),
        Some("shadow_copy")
    );
    assert!(frozen.workspace_plugin("missing").is_none());

    assert_eq!(frozen.active_agent().map(|p| p.id()), Some("agent.primary"));
    let discovered = frozen.discovered_agents();
    assert_eq!(discovered.len(), 1);
    assert_eq!(
        discovered[0].executable_path,
        PathBuf::from("/opt/bin/agent.primary")
    );

    assert_eq!(
        frozen
            .select_workspace_plugin(Path::new("/tmp/plain-project"))
            .map(|p| p.isolation_mode()),
        Some("shadow_copy")
    );
    assert_eq!(
        frozen
            .select_workspace_plugin(Path::new("/tmp/unmatched"))
            .map(|p| p.isolation_mode()),
        Some("git_worktree")
    );
}

#[tokio::test]
async fn frozen_plugin_registry_orders_startup_rolls_back_and_drains_shutdown() {
    let log = Arc::new(Mutex::new(Vec::new()));

    // 1. Startup failure on third plugin rolls back first two in reverse and does not leave stale started entries.
    let mut failing_builder = PluginRegistryBuilder::new();
    failing_builder
        .register_agent_plugin(Arc::new(StubAgentPlugin::new("a1", true, Arc::clone(&log))))
        .unwrap();
    failing_builder
        .register_workspace_plugin(Arc::new(StubWorkspacePlugin::new(
            "w1",
            "m1",
            "x",
            Arc::clone(&log),
        )))
        .unwrap();
    let mut failing_a2 = StubAgentPlugin::new("a2", true, Arc::clone(&log));
    failing_a2.fail_start = true;
    failing_builder
        .register_agent_plugin(Arc::new(failing_a2))
        .unwrap();

    let frozen_fail = failing_builder.freeze();
    let start_err = frozen_fail.start_all().await.expect_err("a2 start fails");
    assert!(matches!(
        start_err,
        RegistryError::StartFailed { ref id, .. } if id == "a2"
    ));
    frozen_fail
        .shutdown_all()
        .await
        .expect("shutdown after rollback is empty drain");
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "start:a1",
            "start:w1",
            "start:a2",
            "shutdown:w1",
            "shutdown:a1",
        ]
    );

    // 2. Successful start_all -> shutdown_all drains in reverse even when a plugin fails shutdown,
    // returning the first shutdown error and remaining idempotent afterwards.
    log.lock().unwrap().clear();
    let mut drain_builder = PluginRegistryBuilder::new();
    let mut p1 = StubAgentPlugin::new("p1", true, Arc::clone(&log));
    p1.fail_shutdown = true;
    let mut p2 = StubWorkspacePlugin::new("p2", "m2", "y", Arc::clone(&log));
    p2.fail_shutdown = true;
    let p3 = StubAgentPlugin::new("p3", false, Arc::clone(&log));

    drain_builder.register_agent_plugin(Arc::new(p1)).unwrap();
    drain_builder
        .register_workspace_plugin(Arc::new(p2))
        .unwrap();
    drain_builder.register_agent_plugin(Arc::new(p3)).unwrap();

    let frozen_ok = drain_builder.freeze();
    frozen_ok.start_all().await.expect("start_all succeeds");
    let shutdown_err = frozen_ok
        .shutdown_all()
        .await
        .expect_err("first shutdown error returned");
    assert!(matches!(
        shutdown_err,
        RegistryError::ShutdownFailed { ref id, .. } if id == "p2"
    ));
    frozen_ok
        .shutdown_all()
        .await
        .expect("second shutdown_all is drained no-op");

    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "start:p1",
            "start:p2",
            "start:p3",
            "shutdown:p3",
            "shutdown:p2",
            "shutdown:p1",
        ]
    );
}

#[derive(Default)]
struct RepoState {
    nodes: Vec<ConversationNode>,
    anchors: HashMap<CoreUuid, Anchor>,
    node_anchors: HashMap<CoreUuid, Vec<CoreUuid>>,
    threads: Vec<Thread>,
    blackboard: BTreeMap<String, BlackboardEntry>,
}

#[derive(Default)]
struct StubSessionRepository {
    state: Mutex<RepoState>,
}

impl DagStorage for StubSessionRepository {
    fn insert_node(&self, node: &ConversationNode) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        if let Some(pid) = node.parent_id {
            if pid.0 == node.id.0 {
                return Err(CoreError::CycleDetected(node.id.to_string()));
            }
            if !st.nodes.iter().any(|n| n.id == pid) {
                return Err(CoreError::DanglingParent(pid.to_string()));
            }
        }
        st.nodes.push(node.clone());
        Ok(())
    }

    fn get_node(&self, id: &CoreUuid) -> CoreResult<Option<ConversationNode>> {
        let st = self.state.lock().unwrap();
        Ok(st.nodes.iter().find(|n| n.id.0 == *id).cloned())
    }

    fn get_children(&self, parent_id: &CoreUuid) -> CoreResult<Vec<ConversationNode>> {
        let st = self.state.lock().unwrap();
        Ok(st
            .nodes
            .iter()
            .filter(|n| n.parent_id.is_some_and(|p| p.0 == *parent_id))
            .cloned()
            .collect())
    }

    fn get_trunk_path(&self, leaf_id: &CoreUuid) -> CoreResult<Vec<ConversationNode>> {
        let st = self.state.lock().unwrap();
        let by_id: HashMap<CoreUuid, &ConversationNode> =
            st.nodes.iter().map(|n| (n.id.0, n)).collect();
        let mut cur = Some(*leaf_id);
        let mut out = Vec::new();
        while let Some(id) = cur {
            let Some(node) = by_id.get(&id) else {
                break;
            };
            out.push((*node).clone());
            cur = node.parent_id.map(|p| p.0);
        }
        if out.is_empty() {
            return Err(CoreError::NodeNotFound(leaf_id.to_string()));
        }
        out.reverse();
        Ok(out)
    }

    fn create_thread(&self, parent_node_id: &CoreUuid, anchor_id: &CoreUuid) -> CoreResult<CoreUuid> {
        self.create_thread_with_parent(NodeId(*parent_node_id), AnchorId(*anchor_id), None, None)
            .map(|id| id.0)
    }

    fn get_thread(&self, thread_id: &CoreUuid) -> CoreResult<Option<Thread>> {
        let st = self.state.lock().unwrap();
        Ok(st.threads.iter().find(|t| t.id.0 == *thread_id).cloned())
    }

    fn list_threads(&self) -> CoreResult<Vec<Thread>> {
        Ok(self.state.lock().unwrap().threads.clone())
    }

    fn list_nodes(&self) -> CoreResult<Vec<ConversationNode>> {
        Ok(self.state.lock().unwrap().nodes.clone())
    }

    fn get_anchor(&self, anchor_id: &CoreUuid) -> CoreResult<Option<Anchor>> {
        Ok(self.state.lock().unwrap().anchors.get(anchor_id).cloned())
    }

    fn update_thread_leaf(&self, thread_id: &CoreUuid, leaf_id: &CoreUuid) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .threads
            .iter_mut()
            .find(|t| t.id.0 == *thread_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;
        t.active_leaf_id = NodeId(*leaf_id);
        t.leaf_node_id = NodeId(*leaf_id);
        Ok(())
    }

    fn update_thread_status(&self, thread_id: &CoreUuid, status: ThreadStatus) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .threads
            .iter_mut()
            .find(|t| t.id.0 == *thread_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;
        t.status = status;
        t.state = status;
        Ok(())
    }
}

impl SessionRepository for StubSessionRepository {
    fn insert_anchor(&self, anchor_id: &CoreUuid, anchor: &Anchor) -> CoreResult<()> {
        self.state
            .lock()
            .unwrap()
            .anchors
            .insert(*anchor_id, anchor.clone());
        Ok(())
    }

    fn list_anchors(&self) -> CoreResult<Vec<(AnchorId, Anchor)>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .anchors
            .iter()
            .map(|(k, v)| (AnchorId(*k), v.clone()))
            .collect())
    }

    fn link_node_anchor(&self, node_id: &CoreUuid, anchor_id: &CoreUuid) -> CoreResult<()> {
        self.state
            .lock()
            .unwrap()
            .node_anchors
            .entry(*node_id)
            .or_default()
            .push(*anchor_id);
        Ok(())
    }

    fn list_anchors_for_node(&self, node_id: &CoreUuid) -> CoreResult<Vec<Anchor>> {
        let st = self.state.lock().unwrap();
        let ids = st.node_anchors.get(node_id).cloned().unwrap_or_default();
        Ok(ids
            .into_iter()
            .filter_map(|id| st.anchors.get(&id).cloned())
            .collect())
    }

    fn list_root_nodes(&self) -> CoreResult<Vec<ConversationNode>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .nodes
            .iter()
            .filter(|n| n.parent_id.is_none())
            .cloned()
            .collect())
    }

    fn list_recent_user_prompts(&self, limit: usize) -> CoreResult<Vec<String>> {
        let st = self.state.lock().unwrap();
        let mut out = Vec::new();
        for node in st.nodes.iter().rev() {
            if node.role == AuthorRole::User && !node.content.trim().is_empty() {
                let text = node.content.trim().to_string();
                if !out.contains(&text) {
                    out.push(text);
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    fn insert_thread(&self, thread: &Thread) -> CoreResult<()> {
        self.state.lock().unwrap().threads.push(thread.clone());
        Ok(())
    }

    fn create_thread_with_kind(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
        kind: ThreadKind,
        cwd: Option<&str>,
    ) -> CoreResult<ThreadId> {
        let mut thread = Thread::new(
            ThreadId::new_v7(),
            parent_node_id,
            anchor_id,
            parent_node_id,
            ThreadStatus::Idle,
        )
        .with_kind(kind);
        if let Some(t) = title {
            thread.title = t.to_string();
        }
        thread.parent_thread_id = parent_thread_id;
        thread.cwd = cwd.map(ToOwned::to_owned);
        let tid = thread.id;
        self.insert_thread(&thread)?;
        Ok(tid)
    }

    fn create_thread_with_parent(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
    ) -> CoreResult<ThreadId> {
        self.create_thread_with_kind(
            parent_node_id,
            anchor_id,
            title,
            parent_thread_id,
            ThreadKind::Agent,
            None,
        )
    }

    fn create_thread_with_title(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: &str,
    ) -> CoreResult<ThreadId> {
        self.create_thread_with_parent(parent_node_id, anchor_id, Some(title), None)
    }

    fn reparent_child_threads(
        &self,
        old_parent_id: &ThreadId,
        new_parent_id: Option<&ThreadId>,
    ) -> CoreResult<usize> {
        let mut st = self.state.lock().unwrap();
        let mut count = 0;
        for t in &mut st.threads {
            if t.parent_thread_id == Some(*old_parent_id) {
                t.parent_thread_id = new_parent_id.copied();
                count += 1;
            }
        }
        Ok(count)
    }

    fn update_thread_title(&self, thread_id: &ThreadId, title: &str) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .threads
            .iter_mut()
            .find(|t| t.id == *thread_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;
        t.title = title.to_string();
        Ok(())
    }

    fn update_thread_acp_session_id(
        &self,
        thread_id: &ThreadId,
        session_id: Option<&str>,
    ) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .threads
            .iter_mut()
            .find(|t| t.id == *thread_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;
        t.acp_session_id = session_id.map(ToOwned::to_owned);
        Ok(())
    }

    fn mark_thread_merged(&self, thread_id: &ThreadId, merged: bool) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .threads
            .iter_mut()
            .find(|t| t.id == *thread_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;
        t.merged = merged;
        Ok(())
    }

    fn update_thread_cwd(&self, thread_id: &ThreadId, cwd: Option<&str>) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .threads
            .iter_mut()
            .find(|t| t.id == *thread_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;
        t.cwd = cwd.map(ToOwned::to_owned);
        Ok(())
    }

    fn update_thread_custom_name(
        &self,
        thread_id: &ThreadId,
        custom_name: Option<&str>,
    ) -> CoreResult<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .threads
            .iter_mut()
            .find(|t| t.id == *thread_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;
        t.custom_name = custom_name.map(ToOwned::to_owned);
        Ok(())
    }

    fn delete_thread(&self, thread_id: &ThreadId) -> CoreResult<bool> {
        let mut st = self.state.lock().unwrap();
        let before = st.threads.len();
        st.threads.retain(|t| t.id != *thread_id);
        Ok(st.threads.len() < before)
    }

    fn get_thread_leaf(&self, thread_id: &ThreadId) -> CoreResult<NodeId> {
        self.get_thread(&thread_id.0)?
            .map(|t| t.active_leaf_id)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))
    }

    fn get_thread_state(&self, thread_id: &ThreadId) -> CoreResult<ThreadStatus> {
        self.get_thread(&thread_id.0)?
            .map(|t| t.status)
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))
    }

    fn insert_blackboard_entry(&self, entry: &BlackboardEntry) -> CoreResult<()> {
        self.state
            .lock()
            .unwrap()
            .blackboard
            .insert(entry.key.clone(), entry.clone());
        Ok(())
    }

    fn get_blackboard_entry(&self, key: &str) -> CoreResult<Option<BlackboardEntry>> {
        Ok(self.state.lock().unwrap().blackboard.get(key).cloned())
    }

    fn list_blackboard_entries(&self) -> CoreResult<Vec<BlackboardEntry>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .blackboard
            .values()
            .cloned()
            .collect())
    }

    fn delete_blackboard_entry(&self, key: &str) -> CoreResult<bool> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .blackboard
            .remove(key)
            .is_some())
    }
}

#[derive(Default)]
struct DriverRecord {
    prompts: Vec<String>,
    permission_responses: Vec<(serde_json::Value, Option<String>)>,
    config_updates: Vec<(String, String)>,
    cancelled_sessions: Vec<String>,
    closed_sessions: Vec<String>,
}

struct StubAgentDriver {
    running: AtomicBool,
    session_seq: AtomicU32,
    subscribers: Mutex<HashMap<String, mpsc::UnboundedSender<DriverEvent>>>,
    records: Arc<Mutex<DriverRecord>>,
}

impl StubAgentDriver {
    fn new(records: Arc<Mutex<DriverRecord>>) -> Self {
        Self {
            running: AtomicBool::new(true),
            session_seq: AtomicU32::new(1),
            subscribers: Mutex::new(HashMap::new()),
            records,
        }
    }

    fn emit(&self, session_id: &str, event: DriverEvent) {
        if let Some(tx) = self.subscribers.lock().unwrap().get(session_id) {
            let _ = tx.send(event);
        }
    }
}

#[async_trait]
impl AgentDriver for StubAgentDriver {
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn initialize(&self) -> Result<AcpInitializeResult, AgentDriverError> {
        Ok(AcpInitializeResult {
            protocol_version: 1,
            agent_name: Some("stub-acp".into()),
            agent_version: Some("0.1".into()),
            supports_system_prompt_append: true,
            auth_method_ids: Vec::new(),
        })
    }

    async fn authenticate(&self, _method_id: &str) -> Result<(), AgentDriverError> {
        Ok(())
    }

    async fn new_session(
        &self,
        _cwd: &Path,
        _system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, AgentDriverError> {
        let seq = self.session_seq.fetch_add(1, Ordering::Relaxed);
        Ok(AcpSessionInfo {
            session_id: format!("stub-sess-{seq}"),
            current_model_id: Some("stub-model".into()),
            current_mode_id: Some("code".into()),
            available_models: Vec::new(),
            available_modes: Vec::new(),
            config_options: vec![AcpConfigOption {
                id: "model".into(),
                name: "Model".into(),
                category: Some("model".into()),
                current_value: "stub-model".into(),
                options: vec![
                    AcpConfigChoice {
                        value: "stub-model".into(),
                        name: "Stub Model".into(),
                        description: None,
                    },
                    AcpConfigChoice {
                        value: "stub-pro".into(),
                        name: "Stub Pro".into(),
                        description: None,
                    },
                ],
            }],
        })
    }

    async fn send_prompt(
        &self,
        session_id: &str,
        _prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, AgentDriverError> {
        Ok(self.subscribe_session(session_id))
    }

    async fn respond_permission(
        &self,
        request_id: serde_json::Value,
        option_id: Option<&str>,
    ) -> Result<(), AgentDriverError> {
        self.records
            .lock()
            .unwrap()
            .permission_responses
            .push((request_id, option_id.map(ToOwned::to_owned)));
        let active_sessions: Vec<String> = self
            .subscribers
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for sid in active_sessions {
            self.emit(
                &sid,
                DriverEvent::StreamChunk {
                    session_id: sid.clone(),
                    text: format!("approved:{}", option_id.unwrap_or("none")),
                },
            );
            self.emit(
                &sid,
                DriverEvent::PromptCompleted {
                    session_id: sid.clone(),
                    stop_reason: "end_turn".into(),
                    usage: None,
                },
            );
        }
        Ok(())
    }

    async fn cancel_session(&self, session_id: &str) -> Result<(), AgentDriverError> {
        self.records
            .lock()
            .unwrap()
            .cancelled_sessions
            .push(session_id.to_string());
        Ok(())
    }

    async fn close_session(&self, session_id: &str) -> Result<(), AgentDriverError> {
        self.records
            .lock()
            .unwrap()
            .closed_sessions
            .push(session_id.to_string());
        Ok(())
    }

    fn subscribe_session(&self, session_id: &str) -> mpsc::UnboundedReceiver<DriverEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribers
            .lock()
            .unwrap()
            .insert(session_id.to_string(), tx);
        rx
    }

    async fn submit_prompt(&self, session_id: &str, prompt: &str) -> Result<(), AgentDriverError> {
        self.records.lock().unwrap().prompts.push(prompt.to_string());
        if prompt == "needs-permission" {
            self.emit(
                session_id,
                DriverEvent::PermissionRequested {
                    request_id: json!(77),
                    session_id: session_id.to_string(),
                    tool_call_id: "tc-perm".into(),
                    tool_name: "run_command".into(),
                    title: "Run check".into(),
                    args: json!({"CommandLine": "cargo --version"}),
                    options: Vec::new(),
                },
            );
            return Ok(());
        }

        self.emit(
            session_id,
            DriverEvent::ToolCall {
                session_id: session_id.to_string(),
                tool_call_id: "sa-1".into(),
                name: "invoke_subagent".into(),
                title: "Stub Subagent".into(),
                args: json!({"Role": "DI Verifier", "TypeName": "research"}),
                status: "pending".into(),
            },
        );
        self.emit(
            session_id,
            DriverEvent::ToolCallUpdate {
                session_id: session_id.to_string(),
                tool_call_id: "sa-1".into(),
                name: "invoke_subagent".into(),
                status: "completed".into(),
                output: "finished".into(),
            },
        );
        self.emit(
            session_id,
            DriverEvent::StreamChunk {
                session_id: session_id.to_string(),
                text: format!("echo:{prompt}"),
            },
        );
        self.emit(
            session_id,
            DriverEvent::PromptCompleted {
                session_id: session_id.to_string(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        );
        Ok(())
    }

    async fn set_config_option(
        &self,
        _session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<AcpConfigOption>, AgentDriverError> {
        self.records
            .lock()
            .unwrap()
            .config_updates
            .push((config_id.to_string(), value.to_string()));
        Ok(vec![AcpConfigOption {
            id: config_id.to_string(),
            name: "Model".into(),
            category: Some("model".into()),
            current_value: value.to_string(),
            options: Vec::new(),
        }])
    }

    async fn shutdown(&self) -> Result<(), AgentDriverError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct StubAgentDriverFactory {
    records: Arc<Mutex<DriverRecord>>,
}

#[async_trait]
impl AgentDriverFactory for StubAgentDriverFactory {
    async fn spawn(
        &self,
        _config: &AcpSpawnConfig,
    ) -> Result<Arc<dyn AgentDriver>, AgentDriverError> {
        Ok(Arc::new(StubAgentDriver::new(Arc::clone(&self.records))))
    }
}

#[derive(Default)]
struct StubPermissionPolicy {
    recorded: Mutex<Vec<(String, PermissionDecision)>>,
}

impl PermissionPolicyPort for StubPermissionPolicy {
    fn evaluate(&self, _request: &ToolInvocationRequest) -> PolicyDecision {
        PolicyDecision::PromptUser
    }

    fn evaluate_scoped(
        &self,
        _session_id: &grein_acp::SessionId,
        _thread_id: &grein_acp::ThreadId,
        _request: &ToolInvocationRequest,
    ) -> PolicyDecision {
        PolicyDecision::PromptUser
    }

    fn record_decision(
        &self,
        _session_id: &grein_acp::SessionId,
        _thread_id: &grein_acp::ThreadId,
        request: &ToolInvocationRequest,
        decision: PermissionDecision,
    ) {
        self.recorded
            .lock()
            .unwrap()
            .push((request.tool_name().to_string(), decision));
    }
}

struct StubPtyHandle {
    cwd: PathBuf,
    rows: Mutex<u16>,
    cols: Mutex<u16>,
    scrollback: Mutex<usize>,
    writes: Mutex<Vec<Vec<u8>>>,
    on_update: Arc<dyn Fn() + Send + Sync>,
}

impl TerminalPtyHandle for StubPtyHandle {
    fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        self.writes.lock().unwrap().push(bytes.to_vec());
        (self.on_update)();
        Ok(())
    }

    fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        *self.rows.lock().unwrap() = rows;
        *self.cols.lock().unwrap() = cols;
        Ok(())
    }

    fn scroll(&self, delta: isize) -> usize {
        let mut sb = self.scrollback.lock().unwrap();
        let next = (*sb as isize + delta).max(0) as usize;
        *sb = next;
        next
    }

    fn set_scrollback(&self, offset: usize) -> usize {
        let mut sb = self.scrollback.lock().unwrap();
        *sb = offset;
        offset
    }

    fn snapshot(&self) -> PtyScreenSnapshot {
        PtyScreenSnapshot {
            rows: *self.rows.lock().unwrap(),
            cols: *self.cols.lock().unwrap(),
            scrollback_offset: *self.scrollback.lock().unwrap(),
            cwd: Some(self.cwd.to_string_lossy().into_owned()),
            ..PtyScreenSnapshot::default()
        }
    }

    fn restart_in_cwd(&self, cwd: &Path) -> io::Result<Arc<dyn TerminalPtyHandle>> {
        Ok(Arc::new(Self {
            cwd: cwd.to_path_buf(),
            rows: Mutex::new(*self.rows.lock().unwrap()),
            cols: Mutex::new(*self.cols.lock().unwrap()),
            scrollback: Mutex::new(0),
            writes: Mutex::new(Vec::new()),
            on_update: Arc::clone(&self.on_update),
        }))
    }

    fn is_exited(&self) -> bool {
        false
    }

    fn child_pid(&self) -> u32 {
        4242
    }

    fn live_cwd(&self) -> Option<PathBuf> {
        Some(self.cwd.clone())
    }

    fn poll_os_cwd(&self) -> Option<String> {
        Some(self.cwd.to_string_lossy().into_owned())
    }

    fn terminal_title(&self) -> Option<String> {
        Some("stub-pty".into())
    }
}

#[derive(Default)]
struct StubPtySpawner {
    spawn_count: AtomicU32,
    handles: Mutex<Vec<Arc<StubPtyHandle>>>,
}

impl PtySpawnerPort for StubPtySpawner {
    fn spawn_with_env(
        &self,
        cwd: &Path,
        rows: u16,
        cols: u16,
        _shell: Option<&str>,
        _extra_env: &[(&str, &str)],
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Arc<dyn TerminalPtyHandle>> {
        self.spawn_count.fetch_add(1, Ordering::Relaxed);
        let handle = Arc::new(StubPtyHandle {
            cwd: cwd.to_path_buf(),
            rows: Mutex::new(rows),
            cols: Mutex::new(cols),
            scrollback: Mutex::new(0),
            writes: Mutex::new(Vec::new()),
            on_update,
        });
        self.handles.lock().unwrap().push(Arc::clone(&handle));
        Ok(handle)
    }
}

#[derive(Default)]
struct StubWorkspaceIsolator {
    primary_root: Option<PathBuf>,
    worktree_dir: Option<PathBuf>,
    removed_paths: Mutex<Vec<PathBuf>>,
    pruned: AtomicBool,
}

impl WorkspaceIsolationPort for StubWorkspaceIsolator {
    fn open_thread_worktree(
        &self,
        _workspace_root: &Path,
        _thread_id: &str,
        _current_cwd: Option<&Path>,
    ) -> Result<PathBuf, WorkspaceError> {
        self.worktree_dir
            .clone()
            .ok_or_else(|| WorkspaceError::GitError("no worktree configured".into()))
    }

    fn list_worktrees(
        &self,
        _search_paths: &[PathBuf],
        _preferred_repo_hint: Option<&Path>,
    ) -> Vec<DiscoveredWorktree> {
        let Some(root) = self.primary_root.clone() else {
            return Vec::new();
        };
        let mut out = vec![DiscoveredWorktree {
            repo_root: root.clone(),
            repo_name: "stub-repo".into(),
            path: root.clone(),
            branch: Some("main".into()),
            head_short: Some("abc1234".into()),
            is_primary: true,
            is_detached: false,
            is_grein_managed: false,
            dirty_count: 1,
        }];
        if let Some(wt) = self.worktree_dir.clone() {
            out.push(DiscoveredWorktree {
                repo_root: root,
                repo_name: "stub-repo".into(),
                path: wt,
                branch: Some("feature/di".into()),
                head_short: Some("def5678".into()),
                is_primary: false,
                is_detached: false,
                is_grein_managed: true,
                dirty_count: 0,
            });
        }
        out
    }

    fn diff_preview(&self, path: &Path, _max_lines: usize) -> String {
        format!("stub-diff:{}\n", path.display())
    }

    fn remove_worktree(
        &self,
        _repo_root: &Path,
        target_path: &Path,
        _force: bool,
        _delete_branch: bool,
    ) -> Result<Option<String>, WorkspaceError> {
        self.removed_paths
            .lock()
            .unwrap()
            .push(target_path.to_path_buf());
        Ok(None)
    }

    fn prune_worktrees(&self, _repo_hint: &Path) -> Result<String, WorkspaceError> {
        self.pruned.store(true, Ordering::SeqCst);
        Ok("stub-pruned".into())
    }
}

#[derive(Default)]
struct StubGitInspector {
    branch: String,
    dirty_count: usize,
    clean_path: Option<PathBuf>,
    files: Vec<String>,
}

impl GitInspectorPort for StubGitInspector {
    fn resolve_primary_repo_root(&self, _path: &Path) -> Option<PathBuf> {
        None
    }

    fn resolve_worktree_toplevel(&self, path: &Path) -> Option<PathBuf> {
        Some(path.to_path_buf())
    }

    fn inspect_git_status(&self, path: &Path) -> Option<RepoGitStatus> {
        let dirty_count = if self.clean_path.as_deref() == Some(path) {
            0
        } else {
            self.dirty_count
        };
        Some(RepoGitStatus {
            branch: (!self.branch.is_empty()).then(|| self.branch.clone()),
            dirty_count,
            is_worktree: false,
        })
    }

    fn list_workspace_files(&self, _root: &Path, max_entries: usize) -> Vec<String> {
        self.files.iter().take(max_entries).cloned().collect()
    }
}

#[derive(Default)]
struct StubShellExecutor {
    commands: Mutex<Vec<String>>,
}

impl ShellExecutorPort for StubShellExecutor {
    fn run_shell_command(
        &self,
        _cwd: &Path,
        command: &str,
        _timeout: Duration,
        _max_output_bytes: usize,
    ) -> io::Result<ShellCommandResult> {
        self.commands.lock().unwrap().push(command.to_string());
        Ok(ShellCommandResult {
            command: command.to_string(),
            stdout: format!("stub-out:{command}"),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }
}

#[derive(Default)]
struct StubSubagentObserver {
    registered: Mutex<Vec<String>>,
    cleared_threads: Mutex<Vec<String>>,
    shut_down: AtomicBool,
    emitter_attached: AtomicBool,
}

impl SubagentObserverPort for StubSubagentObserver {
    fn attach_emitter(&self, _emit: EventEmitter) {
        self.emitter_attached.store(true, Ordering::SeqCst);
    }

    fn register_subagent(&self, _thread_id: &str, snapshot: SubagentSnapshot) {
        self.registered.lock().unwrap().push(snapshot.id);
    }

    fn ingest_tool_output(&self, _thread_id: &str, _subagent_id: &str, _output: &str) {}

    fn update_activity_if_unlinked(&self, _thread_id: &str, _subagent_id: &str, _activity: &str) {}

    fn is_confirmed_running(&self, _thread_id: &str, _subagent_id: &str) -> bool {
        false
    }

    fn mark_terminal(
        &self,
        _thread_id: &str,
        _subagent_id: &str,
        _status: SubagentStatus,
        _activity: Option<String>,
    ) {
    }

    fn clear_thread(&self, thread_id: &str) {
        self.cleared_threads
            .lock()
            .unwrap()
            .push(thread_id.to_string());
    }

    fn shutdown(&self) {
        self.shut_down.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn engine_dependencies_constructor_di_isolates_engine_from_disk_pty_and_git() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_dir = std::fs::canonicalize(temp.path().join("repo")).unwrap_or_else(|_| {
        let p = temp.path().join("repo");
        fs_create(&p);
        std::fs::canonicalize(&p).unwrap()
    });
    let wt_dir = temp.path().join("wt-isolated");
    fs_create(&wt_dir);
    let wt_canon = std::fs::canonicalize(&wt_dir).unwrap();

    let storage = Arc::new(StubSessionRepository::default());
    let agent_factory = Arc::new(StubAgentDriverFactory::default());
    let policy = Arc::new(StubPermissionPolicy::default());
    let pty_spawner = Arc::new(StubPtySpawner::default());
    let workspace = Arc::new(StubWorkspaceIsolator {
        primary_root: Some(repo_dir.clone()),
        worktree_dir: Some(wt_canon.clone()),
        removed_paths: Mutex::new(Vec::new()),
        pruned: AtomicBool::new(false),
    });
    let git_inspector = Arc::new(StubGitInspector {
        branch: "stub/di-branch".into(),
        dirty_count: 3,
        clean_path: Some(wt_canon.clone()),
        files: vec!["src/lib.rs".into(), "tests/arch.rs".into()],
    });
    let shell = Arc::new(StubShellExecutor::default());
    let observer = Arc::new(StubSubagentObserver::default());

    let deps = EngineDependencies {
        storage: Arc::clone(&storage) as Arc<dyn SessionRepository>,
        agent_factory: Arc::clone(&agent_factory) as Arc<dyn AgentDriverFactory>,
        policy: Some(Arc::clone(&policy) as Arc<dyn PermissionPolicyPort>),
        pty_spawner: Arc::clone(&pty_spawner) as Arc<dyn PtySpawnerPort>,
        workspace: Arc::clone(&workspace) as Arc<dyn WorkspaceIsolationPort>,
        git_inspector: Arc::clone(&git_inspector) as Arc<dyn GitInspectorPort>,
        shell: Arc::clone(&shell) as Arc<dyn ShellExecutorPort>,
        subagent_observer: Arc::clone(&observer) as Arc<dyn SubagentObserverPort>,
        cwd: repo_dir.clone(),
        acp_config: Some(AcpBackendConfig {
            command: "/nonexistent/fake-acp-binary".into(),
            args: Vec::new(),
            auth_method: None,
            cwd: repo_dir.clone(),
        }),
    };

    let engine = Arc::new(InProcessEngine::with_dependencies(deps).expect("with_dependencies"));
    assert!(observer.emitter_attached.load(Ordering::SeqCst));

    let mut events_rx = engine.subscribe_tui_events();

    // Thread use-cases + GitInspectorPort metadata
    let main = ThreadUseCasePort::ensure_main_thread(engine.as_ref())
        .await
        .expect("ensure_main_thread");
    let meta = ThreadUseCasePort::thread_metadata(engine.as_ref())
        .await
        .expect("thread_metadata");
    assert_eq!(meta[0].git_branch.as_deref(), Some("stub/di-branch"));
    assert_eq!(meta[0].git_dirty, 3);

    // ACP turn + SubagentObserverPort + AgentTurnPort config/cancel
    let turn = AgentTurnPort::send_turn_for_thread(engine.as_ref(), &main.thread_id, "hello di")
        .await
        .expect("send_turn_for_thread");
    assert!(!turn.node_id.is_empty());
    assert_eq!(
        *observer.registered.lock().unwrap(),
        vec!["sa-1".to_string()]
    );
    assert_eq!(
        *agent_factory
            .records
            .lock()
            .unwrap()
            .prompts,
        vec!["hello di".to_string()]
    );

    let updated_opts =
        AgentTurnPort::set_agent_config(engine.as_ref(), &main.thread_id, "model", "stub-pro", true)
            .await
            .expect("set_agent_config");
    assert!(updated_opts
        .config_options
        .iter()
        .any(|o| o.id == "model" && o.current_value == "stub-pro"));

    // Permission request + policy record + resolve_permission
    let engine_bg = Arc::clone(&engine);
    let tid_bg = main.thread_id.clone();
    let perm_turn_task = tokio::spawn(async move {
        AgentTurnPort::send_turn_for_thread(engine_bg.as_ref(), &tid_bg, "needs-permission").await
    });

    let req_id = timeout(Duration::from_secs(2), async {
        loop {
            match events_rx.recv().await {
                Some(EngineStreamEvent::PermissionRequested { request_id, .. }) => {
                    break request_id;
                }
                Some(_) => continue,
                None => panic!("event stream ended before PermissionRequested"),
            }
        }
    })
    .await
    .expect("permission event timeout");

    let resolved = AgentTurnPort::resolve_permission(
        engine.as_ref(),
        PermissionDecisionPayload {
            request_id: req_id,
            allow: true,
        },
    )
    .await
    .expect("resolve_permission");
    assert!(matches!(
        resolved,
        grein_cli::engine::EngineEvent::PermissionResolved { allow: true, .. }
    ));
    perm_turn_task.await.unwrap().expect("perm turn completes");
    assert_eq!(
        *policy.recorded.lock().unwrap(),
        vec![("run_command".to_string(), PermissionDecision::AllowOnce)]
    );

    AgentTurnPort::cancel_turn(engine.as_ref(), &main.thread_id)
        .await
        .expect("cancel_turn");
    assert_eq!(
        agent_factory.records.lock().unwrap().cancelled_sessions.len(),
        1
    );

    // Shell + AutocompleteContextPort
    let shell_res = ShellUseCasePort::run_shell_command(engine.as_ref(), &main.thread_id, "echo hi")
        .await
        .expect("run_shell_command");
    assert_eq!(shell_res.stdout, "stub-out:echo hi");
    assert_eq!(*shell.commands.lock().unwrap(), vec!["echo hi".to_string()]);

    let prompts = AutocompleteContextPort::list_recent_user_prompts(engine.as_ref(), 10)
        .await
        .expect("recent prompts");
    assert!(prompts.contains(&"hello di".to_string()));
    let files = AutocompleteContextPort::list_workspace_files(engine.as_ref(), None)
        .await
        .expect("workspace files");
    assert_eq!(files, vec!["src/lib.rs", "tests/arch.rs"]);

    // TerminalPtyUseCasePort via StubPtySpawner
    let term = TerminalPtyUseCasePort::create_terminal(
        engine.as_ref(),
        Some(&main.thread_id),
        Some("StubShell"),
        None,
        30,
        100,
    )
    .await
    .expect("create_terminal");
    TerminalPtyUseCasePort::pty_write(engine.as_ref(), &term.thread_id, b"ls\n".to_vec())
        .await
        .expect("pty_write");
    TerminalPtyUseCasePort::pty_resize(engine.as_ref(), &term.thread_id, 40, 120)
        .await
        .expect("pty_resize");
    let scrolled = TerminalPtyUseCasePort::pty_scroll(engine.as_ref(), &term.thread_id, 5)
        .await
        .expect("pty_scroll");
    assert_eq!(scrolled.scrollback_offset, 5);
    let snap = TerminalPtyUseCasePort::pty_snapshot(engine.as_ref(), &term.thread_id, 40, 120)
        .await
        .expect("pty_snapshot");
    assert_eq!((snap.rows, snap.cols), (40, 120));
    TerminalPtyUseCasePort::pty_restart(engine.as_ref(), &term.thread_id, 24, 80)
        .await
        .expect("pty_restart");
    assert_eq!(pty_spawner.spawn_count.load(Ordering::Relaxed), 2);
    assert_eq!(
        *pty_spawner.handles.lock().unwrap()[0].writes.lock().unwrap(),
        vec![b"ls\n".to_vec()]
    );

    // WorktreeUseCasePort via StubWorkspaceIsolator
    let forked = ThreadUseCasePort::fork_thread_with_title(
        engine.as_ref(),
        Some(&main.thread_id),
        Some("DI Branch"),
    )
    .await
    .expect("fork");
    let wt_path = WorktreeUseCasePort::ensure_thread_worktree(engine.as_ref(), &forked.thread_id)
        .await
        .expect("ensure_thread_worktree");
    assert_eq!(wt_path, wt_canon.to_string_lossy());
    let listed = WorktreeUseCasePort::list_worktrees(engine.as_ref())
        .await
        .expect("list_worktrees");
    assert_eq!(listed.len(), 2);
    let switched = WorktreeUseCasePort::switch_thread_worktree(
        engine.as_ref(),
        &forked.thread_id,
        &wt_canon.to_string_lossy(),
    )
    .await
    .expect("switch_thread_worktree");
    assert_eq!(switched, wt_canon.to_string_lossy());
    let diff = WorktreeUseCasePort::worktree_diff_preview(engine.as_ref(), &wt_path)
        .await
        .expect("diff_preview");
    assert!(diff.starts_with("stub-diff:"));
    WorktreeUseCasePort::remove_worktree(engine.as_ref(), &wt_path, false)
        .await
        .expect("remove_worktree");
    assert_eq!(*workspace.removed_paths.lock().unwrap(), vec![wt_canon]);
    let pruned = WorktreeUseCasePort::prune_worktrees(engine.as_ref())
        .await
        .expect("prune_worktrees");
    assert_eq!(pruned, "stub-pruned");
    assert!(workspace.pruned.load(Ordering::SeqCst));

    // Delete & merge thread + drop shutdown propagation
    let merged = ThreadUseCasePort::merge_thread_with_summary(
        engine.as_ref(),
        &forked.thread_id,
        Some(&main.thread_id),
        Some("DI verified"),
    )
    .await
    .expect("merge_thread_with_summary");
    assert!(merged.merged);

    ThreadUseCasePort::delete_thread(engine.as_ref(), &term.thread_id)
        .await
        .expect("delete_thread");
    assert!(observer
        .cleared_threads
        .lock()
        .unwrap()
        .contains(&term.thread_id));

    drop(engine);
    assert!(observer.shut_down.load(Ordering::SeqCst));
}

fn fs_create(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create dir");
}

struct StubWriter {
    name: &'static str,
    fallback: bool,
    outcome: Result<(), String>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl ClipboardWriterBackend for StubWriter {
    fn name(&self) -> &str {
        self.name
    }

    fn write(&self, text: &str) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:{text}", self.name));
        self.outcome.clone()
    }

    fn is_fallback(&self) -> bool {
        self.fallback
    }
}

struct StubReader {
    name: &'static str,
    outcome: Result<String, String>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl ClipboardReaderBackend for StubReader {
    fn name(&self) -> &str {
        self.name
    }

    fn read(&self) -> Result<String, String> {
        self.calls.lock().unwrap().push(self.name.to_string());
        self.outcome.clone()
    }
}

#[test]
fn chained_clipboard_driver_trait_objects_respect_fallback_short_circuit_and_errors() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let osc_sink = Arc::new(Mutex::new(Vec::new()));

    let writers: Vec<Arc<dyn ClipboardWriterBackend>> = vec![
        Arc::new(Osc52Backend::with_sink(false, Arc::clone(&osc_sink))),
        Arc::new(StubWriter {
            name: "wl-copy",
            fallback: false,
            outcome: Err("wl-copy: unavailable".into()),
            calls: Arc::clone(&calls),
        }),
        Arc::new(StubWriter {
            name: "xclip",
            fallback: false,
            outcome: Ok(()),
            calls: Arc::clone(&calls),
        }),
        Arc::new(StubWriter {
            name: "xsel",
            fallback: false,
            outcome: Ok(()),
            calls: Arc::clone(&calls),
        }),
    ];
    let clip_readers: Vec<Arc<dyn ClipboardReaderBackend>> = vec![
        Arc::new(StubReader {
            name: "wl-paste",
            outcome: Err("wl-paste: missing".into()),
            calls: Arc::clone(&calls),
        }),
        Arc::new(StubReader {
            name: "xclip",
            outcome: Ok("copied-value".into()),
            calls: Arc::clone(&calls),
        }),
        Arc::new(StubReader {
            name: "xsel",
            outcome: Ok("unused".into()),
            calls: Arc::clone(&calls),
        }),
    ];
    let primary_readers: Vec<Arc<dyn ClipboardReaderBackend>> = vec![
        Arc::new(StubReader {
            name: "xclip-prim",
            outcome: Err("xclip-prim: empty".into()),
            calls: Arc::clone(&calls),
        }),
        Arc::new(StubReader {
            name: "xsel-prim",
            outcome: Err("xsel-prim: unavailable".into()),
            calls: Arc::clone(&calls),
        }),
    ];

    let driver = ChainedClipboardDriver::from_slices(&writers, &clip_readers, &primary_readers);
    assert_eq!(
        driver.writer_names(),
        vec!["osc52", "wl-copy", "xclip", "xsel"]
    );
    assert_eq!(
        driver.clipboard_reader_names(),
        vec!["wl-paste", "xclip", "xsel"]
    );
    assert_eq!(
        driver.primary_reader_names(),
        vec!["xclip-prim", "xsel-prim"]
    );

    let boxed_port: Box<dyn ClipboardPort> = Box::new(driver.clone());
    assert_eq!(boxed_port.copy("hello"), Ok("xclip".into()));
    assert_eq!(*osc_sink.lock().unwrap(), b"\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(boxed_port.read_clipboard(), Ok("copied-value".into()));
    assert_eq!(
        boxed_port.read_primary(),
        Err("xclip-prim: empty; xsel-prim: unavailable".into())
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            "wl-copy:hello",
            "xclip:hello",
            "wl-paste",
            "xclip",
            "xclip-prim",
            "xsel-prim",
        ]
    );

    // Custom fallback writer takes effect when all native writers fail
    let fallback_only: Arc<dyn ClipboardPort> = Arc::new(ChainedClipboardDriver::new(
        vec![
            Arc::new(StubWriter {
                name: "custom-fallback",
                fallback: true,
                outcome: Ok(()),
                calls: Arc::clone(&calls),
            }),
            Arc::new(StubWriter {
                name: "native",
                fallback: false,
                outcome: Err("native: missing".into()),
                calls: Arc::clone(&calls),
            }),
        ],
        Vec::new(),
        Vec::new(),
    ));
    assert_eq!(fallback_only.copy("fb"), Ok("custom-fallback".into()));

    // All fallback writers failing when no native writers exist reports fallback errors
    let failing_fallback = ChainedClipboardDriver::new(
        vec![Arc::new(StubWriter {
            name: "fb-err",
            fallback: true,
            outcome: Err("fb-err: closed".into()),
            calls: Arc::clone(&calls),
        })],
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(failing_fallback.copy("x"), Err("fb-err: closed".into()));
}
