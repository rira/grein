use std::collections::HashSet;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use grein_acp::AgentDriverFactory;
use grein_workspace::port::{GitInspectorPort, WorkspaceIsolationPort};
use tokio::sync::Mutex as AsyncMutex;

use crate::discovery::{kind_from_label, AcpAgent, AcpAgentKind};
use crate::engine::AcpBackendConfig;
use crate::subagent_watch::SubagentObserverPort;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("plugin id must not be empty")]
    EmptyPluginId,
    #[error("duplicate plugin id: {0}")]
    DuplicatePluginId(String),
    #[error("plugin `{id}` failed to start: {reason}")]
    StartFailed { id: String, reason: String },
    #[error("plugin `{id}` failed to shutdown: {reason}")]
    ShutdownFailed { id: String, reason: String },
}

#[async_trait]
pub trait LifecyclePlugin: Send + Sync {
    fn id(&self) -> &str;

    async fn start(&self) -> Result<(), RegistryError> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), RegistryError> {
        Ok(())
    }
}

pub trait AgentPlugin: LifecyclePlugin {
    fn kind(&self) -> AcpAgentKind;

    fn short_name(&self) -> String {
        self.id().to_string()
    }

    fn display_name(&self) -> String {
        self.id().to_string()
    }

    fn install_hint(&self) -> Option<String> {
        None
    }

    fn executable_path(&self) -> Option<&Path>;

    fn is_available(&self) -> bool {
        self.executable_path().is_some()
    }

    fn driver_factory(&self) -> Arc<dyn AgentDriverFactory>;

    fn acp_backend_config(&self, cwd: &Path) -> Option<AcpBackendConfig>;

    fn subagent_observer(&self) -> Arc<dyn SubagentObserverPort>;
}

pub trait WorkspacePlugin: LifecyclePlugin {
    fn isolation_mode(&self) -> &'static str;

    fn supports_cwd(&self, cwd: &Path) -> bool;

    fn workspace_isolator(&self) -> Arc<dyn WorkspaceIsolationPort>;

    fn git_inspector(&self) -> Arc<dyn GitInspectorPort>;
}

pub struct Building;
pub struct Frozen;

pub type PluginRegistryBuilder = PluginRegistry<Building>;
pub type FrozenPluginRegistry = PluginRegistry<Frozen>;

pub struct PluginRegistry<State> {
    seen_ids: HashSet<String>,
    lifecycle_order: Vec<Arc<dyn LifecyclePlugin>>,
    agent_plugins: Vec<Arc<dyn AgentPlugin>>,
    workspace_plugins: Vec<Arc<dyn WorkspacePlugin>>,
    started: AsyncMutex<Vec<Arc<dyn LifecyclePlugin>>>,
    _state: PhantomData<State>,
}

impl Default for PluginRegistry<Building> {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry<Building> {
    pub fn new() -> Self {
        Self {
            seen_ids: HashSet::new(),
            lifecycle_order: Vec::new(),
            agent_plugins: Vec::new(),
            workspace_plugins: Vec::new(),
            started: AsyncMutex::new(Vec::new()),
            _state: PhantomData,
        }
    }

    fn reserve_id(&mut self, raw_id: &str) -> Result<String, RegistryError> {
        let id = raw_id.trim();
        if id.is_empty() {
            return Err(RegistryError::EmptyPluginId);
        }
        if !self.seen_ids.insert(id.to_string()) {
            return Err(RegistryError::DuplicatePluginId(id.to_string()));
        }
        Ok(id.to_string())
    }

    pub fn register_agent_plugin(
        &mut self,
        plugin: Arc<dyn AgentPlugin>,
    ) -> Result<(), RegistryError> {
        self.reserve_id(plugin.id())?;
        let lifecycle: Arc<dyn LifecyclePlugin> = Arc::clone(&plugin) as Arc<dyn LifecyclePlugin>;
        self.lifecycle_order.push(lifecycle);
        self.agent_plugins.push(plugin);
        Ok(())
    }

    pub fn register_workspace_plugin(
        &mut self,
        plugin: Arc<dyn WorkspacePlugin>,
    ) -> Result<(), RegistryError> {
        self.reserve_id(plugin.id())?;
        let lifecycle: Arc<dyn LifecyclePlugin> = Arc::clone(&plugin) as Arc<dyn LifecyclePlugin>;
        self.lifecycle_order.push(lifecycle);
        self.workspace_plugins.push(plugin);
        Ok(())
    }

    pub fn freeze(self) -> FrozenPluginRegistry {
        PluginRegistry {
            seen_ids: self.seen_ids,
            lifecycle_order: self.lifecycle_order,
            agent_plugins: self.agent_plugins,
            workspace_plugins: self.workspace_plugins,
            started: self.started,
            _state: PhantomData,
        }
    }
}

impl PluginRegistry<Frozen> {
    pub fn plugin_ids(&self) -> Vec<&str> {
        self.lifecycle_order.iter().map(|p| p.id()).collect()
    }

    pub fn agent_plugins(&self) -> &[Arc<dyn AgentPlugin>] {
        &self.agent_plugins
    }

    pub fn workspace_plugins(&self) -> &[Arc<dyn WorkspacePlugin>] {
        &self.workspace_plugins
    }

    pub fn agent_plugin(&self, id: &str) -> Option<&Arc<dyn AgentPlugin>> {
        self.agent_plugins.iter().find(|p| p.id() == id)
    }

    pub fn agent_plugin_by_name(&self, name: &str) -> Option<&Arc<dyn AgentPlugin>> {
        let needle = name.trim();
        if needle.is_empty() {
            return None;
        }

        if let Some(exact) = self.agent_plugins.iter().find(|p| {
            p.id().eq_ignore_ascii_case(needle) || p.short_name().eq_ignore_ascii_case(needle)
        }) {
            return Some(exact);
        }

        let resolved_kind = kind_from_label(needle);
        self.agent_plugins.iter().find(|p| {
            p.kind().label().eq_ignore_ascii_case(needle)
                || (resolved_kind != AcpAgentKind::Custom && p.kind() == resolved_kind)
        })
    }

    pub fn workspace_plugin(&self, id: &str) -> Option<&Arc<dyn WorkspacePlugin>> {
        self.workspace_plugins.iter().find(|p| p.id() == id)
    }

    pub fn active_agent(&self) -> Option<&Arc<dyn AgentPlugin>> {
        self.agent_plugins.iter().find(|p| p.is_available())
    }

    pub fn select_workspace_plugin(&self, cwd: &Path) -> Option<&Arc<dyn WorkspacePlugin>> {
        self.workspace_plugins
            .iter()
            .find(|p| p.supports_cwd(cwd))
            .or_else(|| self.workspace_plugins.first())
    }

    pub fn discovered_agents(&self) -> Vec<AcpAgent> {
        self.agent_plugins
            .iter()
            .filter_map(|p| {
                p.executable_path().map(|path| AcpAgent {
                    kind: p.kind(),
                    executable_path: path.to_path_buf(),
                    args: p
                        .acp_backend_config(Path::new("."))
                        .map(|cfg| cfg.args)
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    pub async fn start_all(&self) -> Result<(), RegistryError> {
        let mut started = self.started.lock().await;
        for plugin in &self.lifecycle_order {
            if let Err(err) = plugin.start().await {
                while let Some(prev) = started.pop() {
                    let _ = prev.shutdown().await;
                }
                return Err(err);
            }
            started.push(Arc::clone(plugin));
        }
        Ok(())
    }

    pub async fn shutdown_all(&self) -> Result<(), RegistryError> {
        let mut started = self.started.lock().await;
        let mut first_err: Option<RegistryError> = None;
        while let Some(plugin) = started.pop() {
            if let Err(err) = plugin.shutdown().await {
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use grein_acp::StdioAgentDriverFactory;
    use grein_workspace::port::{GitCliInspector, GitWorktreeDriver};

    use super::*;
    use crate::subagent_watch::NoopSubagentObserver;

    struct MockAgentPlugin {
        id: String,
        log: Arc<Mutex<Vec<String>>>,
        fail_start: bool,
        fail_shutdown: bool,
        available: bool,
    }

    #[async_trait]
    impl LifecyclePlugin for MockAgentPlugin {
        fn id(&self) -> &str {
            &self.id
        }

        async fn start(&self) -> Result<(), RegistryError> {
            self.log.lock().unwrap().push(format!("start:{}", self.id));
            if self.fail_start {
                return Err(RegistryError::StartFailed {
                    id: self.id.clone(),
                    reason: "boom".into(),
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
                    reason: "fail".into(),
                });
            }
            Ok(())
        }
    }

    impl AgentPlugin for MockAgentPlugin {
        fn kind(&self) -> AcpAgentKind {
            AcpAgentKind::Custom
        }

        fn executable_path(&self) -> Option<&Path> {
            self.available.then(|| Path::new("/bin/echo"))
        }

        fn driver_factory(&self) -> Arc<dyn AgentDriverFactory> {
            Arc::new(StdioAgentDriverFactory::new())
        }

        fn acp_backend_config(&self, cwd: &Path) -> Option<AcpBackendConfig> {
            self.available.then(|| AcpBackendConfig {
                command: "/bin/echo".into(),
                args: Vec::new(),
                auth_method: None,
                cwd: cwd.to_path_buf(),
            })
        }

        fn subagent_observer(&self) -> Arc<dyn SubagentObserverPort> {
            Arc::new(NoopSubagentObserver::new())
        }
    }

    struct MockWorkspacePlugin {
        id: String,
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl LifecyclePlugin for MockWorkspacePlugin {
        fn id(&self) -> &str {
            &self.id
        }

        async fn start(&self) -> Result<(), RegistryError> {
            self.log.lock().unwrap().push(format!("start:{}", self.id));
            Ok(())
        }

        async fn shutdown(&self) -> Result<(), RegistryError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("shutdown:{}", self.id));
            Ok(())
        }
    }

    impl WorkspacePlugin for MockWorkspacePlugin {
        fn isolation_mode(&self) -> &'static str {
            "mock"
        }

        fn supports_cwd(&self, _cwd: &Path) -> bool {
            true
        }

        fn workspace_isolator(&self) -> Arc<dyn WorkspaceIsolationPort> {
            Arc::new(GitWorktreeDriver::new())
        }

        fn git_inspector(&self) -> Arc<dyn GitInspectorPort> {
            Arc::new(GitCliInspector::new())
        }
    }

    #[test]
    fn builder_rejects_empty_and_duplicate_plugin_ids() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut builder = PluginRegistryBuilder::new();

        assert_eq!(
            builder.register_agent_plugin(Arc::new(MockAgentPlugin {
                id: "   ".into(),
                log: Arc::clone(&log),
                fail_start: false,
                fail_shutdown: false,
                available: true,
            })),
            Err(RegistryError::EmptyPluginId)
        );

        builder
            .register_agent_plugin(Arc::new(MockAgentPlugin {
                id: "dup-id".into(),
                log: Arc::clone(&log),
                fail_start: false,
                fail_shutdown: false,
                available: false,
            }))
            .expect("first register");

        assert_eq!(
            builder.register_agent_plugin(Arc::new(MockAgentPlugin {
                id: "dup-id".into(),
                log: Arc::clone(&log),
                fail_start: false,
                fail_shutdown: false,
                available: true,
            })),
            Err(RegistryError::DuplicatePluginId("dup-id".into()))
        );

        assert_eq!(
            builder.register_workspace_plugin(Arc::new(MockWorkspacePlugin {
                id: "dup-id".into(),
                log: Arc::clone(&log),
            })),
            Err(RegistryError::DuplicatePluginId("dup-id".into()))
        );
    }

    #[tokio::test]
    async fn frozen_registry_starts_in_order_and_drains_shutdown_in_reverse() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut builder = PluginRegistryBuilder::new();
        builder
            .register_agent_plugin(Arc::new(MockAgentPlugin {
                id: "p1".into(),
                log: Arc::clone(&log),
                fail_start: false,
                fail_shutdown: false,
                available: false,
            }))
            .unwrap();
        builder
            .register_workspace_plugin(Arc::new(MockWorkspacePlugin {
                id: "p2".into(),
                log: Arc::clone(&log),
            }))
            .unwrap();
        builder
            .register_agent_plugin(Arc::new(MockAgentPlugin {
                id: "p3".into(),
                log: Arc::clone(&log),
                fail_start: false,
                fail_shutdown: false,
                available: true,
            }))
            .unwrap();

        let frozen: FrozenPluginRegistry = builder.freeze();
        assert_eq!(frozen.plugin_ids(), vec!["p1", "p2", "p3"]);
        assert_eq!(frozen.active_agent().map(|a| a.id()), Some("p3"));
        assert_eq!(frozen.discovered_agents().len(), 1);
        assert_eq!(
            frozen
                .select_workspace_plugin(&PathBuf::from("."))
                .map(|w| w.id()),
            Some("p2")
        );

        frozen.start_all().await.expect("start_all");
        frozen.shutdown_all().await.expect("shutdown_all");
        frozen.shutdown_all().await.expect("second shutdown drains safely");

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

    #[tokio::test]
    async fn start_all_failure_rolls_back_already_started_plugins_in_reverse() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut builder = PluginRegistryBuilder::new();
        builder
            .register_agent_plugin(Arc::new(MockAgentPlugin {
                id: "first".into(),
                log: Arc::clone(&log),
                fail_start: false,
                fail_shutdown: false,
                available: true,
            }))
            .unwrap();
        builder
            .register_workspace_plugin(Arc::new(MockWorkspacePlugin {
                id: "second".into(),
                log: Arc::clone(&log),
            }))
            .unwrap();
        builder
            .register_agent_plugin(Arc::new(MockAgentPlugin {
                id: "third".into(),
                log: Arc::clone(&log),
                fail_start: true,
                fail_shutdown: false,
                available: true,
            }))
            .unwrap();

        let frozen = builder.freeze();
        let err = frozen.start_all().await.expect_err("start_all must fail");
        assert!(matches!(err, RegistryError::StartFailed { ref id, .. } if id == "third"));
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "start:first",
                "start:second",
                "start:third",
                "shutdown:second",
                "shutdown:first",
            ]
        );
    }

    #[test]
    fn agent_plugin_defaults_and_lookup_by_name_work_case_insensitively() {
        use crate::bootstrap::plugins::{AgyAgentPlugin, ClaudeAgentPlugin, CustomAcpAgentPlugin};

        let log = Arc::new(Mutex::new(Vec::new()));
        let mock = MockAgentPlugin {
            id: "mock.agent".into(),
            log,
            fail_start: false,
            fail_shutdown: false,
            available: false,
        };
        assert_eq!(mock.short_name(), "mock.agent");
        assert_eq!(mock.display_name(), "mock.agent");
        assert_eq!(mock.install_hint(), None);

        let mut builder = PluginRegistryBuilder::new();
        builder
            .register_agent_plugin(Arc::new(AgyAgentPlugin::new(
                Some(PathBuf::from("/usr/bin/agy")),
                None,
            )))
            .unwrap();
        builder
            .register_agent_plugin(Arc::new(ClaudeAgentPlugin::new(
                Some(PathBuf::from("/usr/bin/claude-agent-acp")),
                None,
            )))
            .unwrap();
        builder
            .register_agent_plugin(Arc::new(CustomAcpAgentPlugin::from_discovered(
                0,
                AcpAgent {
                    kind: AcpAgentKind::Custom,
                    executable_path: PathBuf::from("/opt/alpha"),
                    args: Vec::new(),
                },
            )))
            .unwrap();
        builder
            .register_agent_plugin(Arc::new(CustomAcpAgentPlugin::from_discovered(
                1,
                AcpAgent {
                    kind: AcpAgentKind::Custom,
                    executable_path: PathBuf::from("/opt/beta"),
                    args: Vec::new(),
                },
            )))
            .unwrap();

        let frozen = builder.freeze();
        assert_eq!(
            frozen.agent_plugin_by_name("AGY").map(|p| p.id()),
            Some("agent.agy")
        );
        assert_eq!(
            frozen.agent_plugin_by_name("agent.agy").map(|p| p.id()),
            Some("agent.agy")
        );
        assert_eq!(
            frozen.agent_plugin_by_name("claude").map(|p| p.id()),
            Some("agent.claude")
        );
        assert_eq!(
            frozen.agent_plugin_by_name("AGENT.CLAUDE").map(|p| p.id()),
            Some("agent.claude")
        );
        assert_eq!(
            frozen
                .agent_plugin_by_name("claude-agent-acp")
                .map(|p| p.id()),
            Some("agent.claude")
        );
        assert_eq!(
            frozen
                .agent_plugin_by_name("CLAUDE-CODE-ACP")
                .map(|p| p.id()),
            Some("agent.claude")
        );
        assert_eq!(
            frozen.agent_plugin_by_name("BETA").map(|p| p.id()),
            Some("agent.custom.1.beta")
        );
        assert_eq!(
            frozen.agent_plugin_by_name("custom").map(|p| p.id()),
            Some("agent.custom.0.alpha")
        );
        assert!(frozen.agent_plugin_by_name("nonexistent").is_none());
        assert!(frozen.agent_plugin_by_name("   ").is_none());
    }
}
