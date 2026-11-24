use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use grein_acp::{
    AcpConfigChoice, AcpConfigOption, AcpInitializeResult, AcpSessionInfo, AcpSpawnConfig,
    AgentDriver, AgentDriverError, AgentDriverFactory, DriverEvent,
};
use grein_cli::bootstrap::adapter::{EngineClientPort, InitialSessionMode};
use grein_cli::bootstrap::registry::{
    AgentPlugin, LifecyclePlugin, PluginRegistryBuilder,
};
use grein_cli::bootstrap::{build_engine_dependencies, BootstrapConfig};
use grein_cli::daemon::run_daemon_with_engine;
use grein_cli::discovery::AcpAgentKind;
use grein_cli::engine::{
    dispatch_json_rpc, AcpBackendConfig, AgentTurnPort, EngineDependencies, EngineError,
    GreinEngine, InProcessEngine, RegisteredAgentProvider, ThreadUseCasePort,
    UnixSocketEngineClient,
};
use grein_cli::subagent_watch::{EventEmitter, SubagentObserverPort};
use grein_core::model::NodeId;
use grein_core::port::dto::{SubagentSnapshot, SubagentStatus, TuiEngineEvent};

type CoreUuid = <NodeId as std::ops::Deref>::Target;
use grein_core::port::outbound::{PreferenceRepository, SessionRepository};
use grein_core::storage::sqlite::SqliteStorage;
use grein_core::UserPrefsStore;
use grein_tui::engine_port::TuiEnginePort;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[derive(Default)]
struct ProviderDriverRecords {
    spawned_count: u32,
    shutdown_count: u32,
    prompts: Vec<(String, String)>,
    config_updates: Vec<(String, String, String)>,
    closed_sessions: Vec<String>,
    cancelled_sessions: Vec<String>,
    drivers_running: Vec<Arc<AtomicBool>>,
}

impl ProviderDriverRecords {
    fn alive_count(&self) -> usize {
        self.drivers_running
            .iter()
            .filter(|flag| flag.load(Ordering::SeqCst))
            .count()
    }
}

struct StubProviderDriver {
    provider_label: String,
    default_model: String,
    alt_model: String,
    running: Arc<AtomicBool>,
    session_seq: AtomicU32,
    subscribers: Mutex<HashMap<String, mpsc::UnboundedSender<DriverEvent>>>,
    records: Arc<Mutex<ProviderDriverRecords>>,
}

impl StubProviderDriver {
    fn new(
        provider_label: String,
        default_model: String,
        alt_model: String,
        records: Arc<Mutex<ProviderDriverRecords>>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        records
            .lock()
            .unwrap()
            .drivers_running
            .push(Arc::clone(&running));
        Self {
            provider_label,
            default_model,
            alt_model,
            running,
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
impl AgentDriver for StubProviderDriver {
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn initialize(&self) -> Result<AcpInitializeResult, AgentDriverError> {
        Ok(AcpInitializeResult {
            protocol_version: 1,
            agent_name: Some(format!("stub-{}", self.provider_label)),
            agent_version: Some("1.0".into()),
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
            session_id: format!("{}-sess-{seq}", self.provider_label),
            current_model_id: Some(self.default_model.clone()),
            current_mode_id: Some("code".into()),
            available_models: Vec::new(),
            available_modes: Vec::new(),
            config_options: vec![AcpConfigOption {
                id: "model".into(),
                name: "Model".into(),
                category: Some("model".into()),
                current_value: self.default_model.clone(),
                options: vec![
                    AcpConfigChoice {
                        value: self.default_model.clone(),
                        name: format!("{} Default", self.provider_label),
                        description: None,
                    },
                    AcpConfigChoice {
                        value: self.alt_model.clone(),
                        name: format!("{} Alt", self.provider_label),
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
        _request_id: Value,
        _option_id: Option<&str>,
    ) -> Result<(), AgentDriverError> {
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
        self.records
            .lock()
            .unwrap()
            .prompts
            .push((session_id.to_string(), prompt.to_string()));
        let subagent_id = format!("{}-sa-{}", self.provider_label, session_id);
        self.emit(
            session_id,
            DriverEvent::ToolCall {
                session_id: session_id.to_string(),
                tool_call_id: subagent_id.clone(),
                name: "invoke_subagent".into(),
                title: format!("{} worker", self.provider_label),
                args: json!({"Role": "Worker", "TypeName": "research"}),
                status: "pending".into(),
            },
        );
        self.emit(
            session_id,
            DriverEvent::ToolCallUpdate {
                session_id: session_id.to_string(),
                tool_call_id: subagent_id,
                name: "invoke_subagent".into(),
                status: "completed".into(),
                output: "done".into(),
            },
        );
        self.emit(
            session_id,
            DriverEvent::StreamChunk {
                session_id: session_id.to_string(),
                text: format!("{}:{prompt}", self.provider_label),
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
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<AcpConfigOption>, AgentDriverError> {
        self.records.lock().unwrap().config_updates.push((
            session_id.to_string(),
            config_id.to_string(),
            value.to_string(),
        ));
        Ok(vec![AcpConfigOption {
            id: config_id.to_string(),
            name: "Model".into(),
            category: Some("model".into()),
            current_value: value.to_string(),
            options: vec![
                AcpConfigChoice {
                    value: self.default_model.clone(),
                    name: "Default".into(),
                    description: None,
                },
                AcpConfigChoice {
                    value: self.alt_model.clone(),
                    name: "Alt".into(),
                    description: None,
                },
            ],
        }])
    }

    async fn shutdown(&self) -> Result<(), AgentDriverError> {
        self.running.store(false, Ordering::SeqCst);
        self.records.lock().unwrap().shutdown_count += 1;
        Ok(())
    }
}

struct StubProviderFactory {
    provider_label: String,
    default_model: String,
    alt_model: String,
    records: Arc<Mutex<ProviderDriverRecords>>,
}

impl StubProviderFactory {
    fn new(
        provider_label: &str,
        default_model: &str,
        alt_model: &str,
    ) -> (Arc<Self>, Arc<Mutex<ProviderDriverRecords>>) {
        let records = Arc::new(Mutex::new(ProviderDriverRecords::default()));
        let factory = Arc::new(Self {
            provider_label: provider_label.to_string(),
            default_model: default_model.to_string(),
            alt_model: alt_model.to_string(),
            records: Arc::clone(&records),
        });
        (factory, records)
    }
}

#[async_trait]
impl AgentDriverFactory for StubProviderFactory {
    async fn spawn(
        &self,
        _config: &AcpSpawnConfig,
    ) -> Result<Arc<dyn AgentDriver>, AgentDriverError> {
        self.records.lock().unwrap().spawned_count += 1;
        Ok(Arc::new(StubProviderDriver::new(
            self.provider_label.clone(),
            self.default_model.clone(),
            self.alt_model.clone(),
            Arc::clone(&self.records),
        )))
    }
}

#[derive(Default)]
struct StubObserver {
    emitter_attached: AtomicBool,
    registered: Mutex<Vec<(String, String)>>,
    cleared_threads: Mutex<Vec<String>>,
    shut_down: AtomicBool,
}

impl SubagentObserverPort for StubObserver {
    fn attach_emitter(&self, _emit: EventEmitter) {
        self.emitter_attached.store(true, Ordering::SeqCst);
    }

    fn register_subagent(&self, thread_id: &str, snapshot: SubagentSnapshot) {
        self.registered
            .lock()
            .unwrap()
            .push((thread_id.to_string(), snapshot.id));
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

struct TestAgentPlugin {
    plugin_id: String,
    short_name: String,
    display_name: String,
    executable: Option<PathBuf>,
    install_hint: Option<String>,
    factory: Arc<dyn AgentDriverFactory>,
    observer: Arc<dyn SubagentObserverPort>,
}

#[async_trait]
impl LifecyclePlugin for TestAgentPlugin {
    fn id(&self) -> &str {
        &self.plugin_id
    }
}

impl AgentPlugin for TestAgentPlugin {
    fn kind(&self) -> AcpAgentKind {
        AcpAgentKind::Custom
    }

    fn short_name(&self) -> String {
        self.short_name.clone()
    }

    fn display_name(&self) -> String {
        self.display_name.clone()
    }

    fn install_hint(&self) -> Option<String> {
        self.install_hint.clone()
    }

    fn executable_path(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    fn driver_factory(&self) -> Arc<dyn AgentDriverFactory> {
        Arc::clone(&self.factory)
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

struct MultiProviderFixture {
    _temp: tempfile::TempDir,
    db_path: PathBuf,
    storage: Arc<SqliteStorage>,
    prefs: Arc<UserPrefsStore>,
    alpha_records: Arc<Mutex<ProviderDriverRecords>>,
    beta_records: Arc<Mutex<ProviderDriverRecords>>,
    alpha_observer: Arc<StubObserver>,
    beta_observer: Arc<StubObserver>,
    engine: Arc<InProcessEngine>,
}

impl MultiProviderFixture {
    fn new(set_default_pref: Option<&str>) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let storage = Arc::new(SqliteStorage::open(&db_path).expect("open sqlite"));
        let prefs = Arc::new(UserPrefsStore::in_memory().expect("in-memory prefs"));
        if let Some(val) = set_default_pref {
            prefs
                .set("ui", "default_agent_provider", val)
                .expect("set default_agent_provider");
        }

        let (alpha_factory, alpha_records) =
            StubProviderFactory::new("alpha", "alpha-base", "alpha-pro");
        let (beta_factory, beta_records) =
            StubProviderFactory::new("beta", "beta-fast", "beta-deep");
        let (gamma_factory, _) = StubProviderFactory::new("gamma", "gamma-1", "gamma-2");

        let alpha_observer = Arc::new(StubObserver::default());
        let beta_observer = Arc::new(StubObserver::default());
        let gamma_observer = Arc::new(StubObserver::default());

        let mut deps = EngineDependencies::for_storage(
            Arc::clone(&storage) as Arc<dyn SessionRepository>,
            temp.path().to_path_buf(),
            None,
        );
        deps.prefs = Some(Arc::clone(&prefs) as Arc<dyn PreferenceRepository>);
        deps.providers = vec![
            RegisteredAgentProvider {
                id: "gamma".into(),
                plugin_id: "agent.gamma".into(),
                display_name: "Gamma Agent".into(),
                kind: "custom".into(),
                available: false,
                executable: None,
                args: Vec::new(),
                auth_method: None,
                install_hint: Some("cargo install gamma-acp".into()),
                driver_factory: gamma_factory,
                subagent_observer: gamma_observer,
            },
            RegisteredAgentProvider {
                id: "alpha".into(),
                plugin_id: "agent.alpha".into(),
                display_name: "Alpha Agent".into(),
                kind: "custom".into(),
                available: true,
                executable: Some("/opt/bin/alpha-acp".into()),
                args: Vec::new(),
                auth_method: None,
                install_hint: None,
                driver_factory: alpha_factory,
                subagent_observer: Arc::clone(&alpha_observer) as Arc<dyn SubagentObserverPort>,
            },
            RegisteredAgentProvider {
                id: "beta".into(),
                plugin_id: "agent.beta".into(),
                display_name: "Beta Agent".into(),
                kind: "custom".into(),
                available: true,
                executable: Some("/opt/bin/beta-acp".into()),
                args: Vec::new(),
                auth_method: None,
                install_hint: None,
                driver_factory: beta_factory,
                subagent_observer: Arc::clone(&beta_observer) as Arc<dyn SubagentObserverPort>,
            },
        ];

        let engine =
            Arc::new(InProcessEngine::with_dependencies(deps).expect("with_dependencies"));
        assert!(alpha_observer.emitter_attached.load(Ordering::SeqCst));
        assert!(beta_observer.emitter_attached.load(Ordering::SeqCst));

        Self {
            _temp: temp,
            db_path,
            storage,
            prefs,
            alpha_records,
            beta_records,
            alpha_observer,
            beta_observer,
            engine,
        }
    }
}

#[tokio::test]
async fn test_list_agent_providers_and_bootstrap_registry_injection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let storage = Arc::new(SqliteStorage::in_memory().expect("in-memory sqlite"));
    let (alpha_factory, _) = StubProviderFactory::new("alpha", "alpha-base", "alpha-pro");
    let (beta_factory, _) = StubProviderFactory::new("beta", "beta-fast", "beta-deep");
    let (gamma_factory, _) = StubProviderFactory::new("gamma", "gamma-1", "gamma-2");

    let mut builder = PluginRegistryBuilder::new();
    builder
        .register_agent_plugin(Arc::new(TestAgentPlugin {
            plugin_id: "agent.gamma".into(),
            short_name: "gamma".into(),
            display_name: "Gamma Agent".into(),
            executable: None,
            install_hint: Some("cargo install gamma-acp".into()),
            factory: gamma_factory,
            observer: Arc::new(StubObserver::default()),
        }))
        .unwrap();
    builder
        .register_agent_plugin(Arc::new(TestAgentPlugin {
            plugin_id: "agent.alpha".into(),
            short_name: "alpha".into(),
            display_name: "Alpha Agent".into(),
            executable: Some(PathBuf::from("/opt/bin/alpha")),
            install_hint: None,
            factory: alpha_factory,
            observer: Arc::new(StubObserver::default()),
        }))
        .unwrap();
    builder
        .register_agent_plugin(Arc::new(TestAgentPlugin {
            plugin_id: "agent.beta".into(),
            short_name: "beta".into(),
            display_name: "Beta Agent".into(),
            executable: Some(PathBuf::from("/opt/bin/beta")),
            install_hint: None,
            factory: beta_factory,
            observer: Arc::new(StubObserver::default()),
        }))
        .unwrap();
    let frozen = builder.freeze();

    let cfg = BootstrapConfig {
        cwd: temp.path().to_path_buf(),
        search_path: None,
        acp_config_path: None,
        agent_bin_env: None,
        agent_args_env: None,
        agent_auth_env: None,
        prefs_db_path: None,
        state_dir: temp.path().join(".grein"),
        default_db_path: temp.path().join(".grein/session.db"),
        default_socket_path: temp.path().join(".grein/daemon.sock"),
        initial_mode: InitialSessionMode::Agent,
    };

    let deps = build_engine_dependencies(
        Arc::clone(&storage) as Arc<dyn SessionRepository>,
        &cfg,
        &frozen,
    );
    assert_eq!(deps.providers.len(), 3);
    let engine = InProcessEngine::with_dependencies(deps).expect("with_dependencies");

    let providers = engine
        .list_agent_providers()
        .await
        .expect("list_agent_providers");
    assert_eq!(providers.len(), 3);
    // Default available provider ("alpha") sorts first.
    assert_eq!(providers[0].id, "alpha");
    assert!(providers[0].is_default);
    assert!(providers[0].available);
    assert_eq!(
        providers[0].executable.as_deref(),
        Some("/opt/bin/alpha")
    );

    let gamma = providers.iter().find(|p| p.id == "gamma").expect("gamma");
    assert!(!gamma.available);
    assert!(!gamma.is_default);
    assert_eq!(
        gamma.install_hint.as_deref(),
        Some("cargo install gamma-acp")
    );

    let beta = providers.iter().find(|p| p.id == "beta").expect("beta");
    assert!(beta.available);
    assert!(!beta.is_default);

    // Unavailable provider rejects creation with install_hint in InvalidParams.
    let err = engine
        .create_agent_thread(Some("GammaFail"), None, None, Some("gamma"))
        .await
        .expect_err("unavailable provider fails");
    match err {
        EngineError::InvalidParams(msg) => {
            assert!(msg.contains("cargo install gamma-acp"), "unexpected: {msg}");
        }
        other => panic!("expected InvalidParams, got {other:?}"),
    }

    // Unknown provider rejects creation cleanly.
    let err = engine
        .create_agent_thread(Some("UnknownFail"), None, None, Some("unknown-x"))
        .await
        .expect_err("unknown provider fails");
    assert!(matches!(err, EngineError::InvalidParams(_)));

    // Case-insensitive resolution by id, plugin_id, and display_name.
    let t_by_id = engine
        .create_agent_thread(Some("ById"), None, None, Some("BETA"))
        .await
        .expect("create by id");
    assert_eq!(t_by_id.agent_provider.as_deref(), Some("beta"));

    let t_by_plugin = engine
        .create_agent_thread(Some("ByPlugin"), None, None, Some("AGENT.ALPHA"))
        .await
        .expect("create by plugin_id");
    assert_eq!(t_by_plugin.agent_provider.as_deref(), Some("alpha"));

    let t_by_display = engine
        .create_agent_thread(Some("ByDisplay"), None, None, Some("beta agent"))
        .await
        .expect("create by display_name");
    assert_eq!(t_by_display.agent_provider.as_deref(), Some("beta"));
}

#[tokio::test]
async fn test_sibling_threads_concurrent_providers_isolated_defaults_and_shutdown() {
    let fx = MultiProviderFixture::new(None);
    let sub_a = fx._temp.path().join("repo-alpha");
    let sub_b = fx._temp.path().join("repo-beta");
    std::fs::create_dir_all(&sub_a).unwrap();
    std::fs::create_dir_all(&sub_b).unwrap();
    let canon_a = std::fs::canonicalize(&sub_a).unwrap();
    let canon_b = std::fs::canonicalize(&sub_b).unwrap();

    let main = fx.engine.ensure_main_thread().await.expect("ensure main");
    assert_eq!(main.agent_provider.as_deref(), Some("alpha"));

    let sib_a = fx
        .engine
        .create_agent_thread(
            Some("Sibling Alpha"),
            Some(canon_a.to_str().unwrap()),
            Some(&main.thread_id),
            Some("alpha"),
        )
        .await
        .expect("sib alpha");
    let sib_b = fx
        .engine
        .create_agent_thread(
            Some("Sibling Beta"),
            Some(canon_b.to_str().unwrap()),
            Some(&main.thread_id),
            Some("BETA"),
        )
        .await
        .expect("sib beta");

    // Sibling inheriting parent cwd when cwd is omitted.
    let sib_b_inherit_cwd = fx
        .engine
        .create_agent_thread(Some("Beta Inherit"), None, Some(&sib_b.thread_id), Some("beta"))
        .await
        .expect("sib inherit cwd");

    assert_eq!(sib_a.agent_provider.as_deref(), Some("alpha"));
    assert_eq!(sib_b.agent_provider.as_deref(), Some("beta"));
    assert_eq!(sib_b_inherit_cwd.agent_provider.as_deref(), Some("beta"));

    let meta = fx.engine.thread_metadata().await.expect("thread_metadata");
    let m_a = meta.iter().find(|m| m.thread_id == sib_a.thread_id).unwrap();
    let m_b = meta.iter().find(|m| m.thread_id == sib_b.thread_id).unwrap();
    let m_bi = meta
        .iter()
        .find(|m| m.thread_id == sib_b_inherit_cwd.thread_id)
        .unwrap();
    assert_eq!(m_a.parent_thread_id.as_deref(), Some(main.thread_id.as_str()));
    assert_eq!(m_b.parent_thread_id.as_deref(), Some(main.thread_id.as_str()));
    assert_eq!(m_bi.parent_thread_id.as_deref(), Some(sib_b.thread_id.as_str()));
    assert_eq!(m_a.inherited_turn_count, 0);
    assert_eq!(m_b.inherited_turn_count, 0);
    assert_eq!(m_a.agent_provider.as_deref(), Some("alpha"));
    assert_eq!(m_b.agent_provider.as_deref(), Some("beta"));
    assert_eq!(m_a.cwd.as_deref(), Some(canon_a.to_str().unwrap()));
    assert_eq!(m_b.cwd.as_deref(), Some(canon_b.to_str().unwrap()));
    assert_eq!(m_bi.cwd.as_deref(), Some(canon_b.to_str().unwrap()));

    // Verify SQLite reload preserves parent_thread_id + agent_provider.
    {
        let reopened = SqliteStorage::open(&fx.db_path).expect("reopen sqlite");
        let tid_b = CoreUuid::parse_str(&sib_b.thread_id).unwrap();
        let row_b = reopened.get_thread(tid_b).unwrap().unwrap();
        assert_eq!(row_b.agent_provider.as_deref(), Some("beta"));
        assert_eq!(
            row_b.parent_thread_id.map(|id| id.0.to_string()).as_deref(),
            Some(main.thread_id.as_str())
        );
    }

    // Hydrate options on both siblings concurrently and set per-provider default on alpha.
    let opts_a = fx.engine.agent_options(&sib_a.thread_id).await.unwrap();
    let opts_b = fx.engine.agent_options(&sib_b.thread_id).await.unwrap();
    assert_eq!(opts_a.config_options[0].current_value, "alpha-base");
    assert_eq!(opts_b.config_options[0].current_value, "beta-fast");

    fx.engine
        .set_agent_config(&sib_a.thread_id, "model", "alpha-pro", true)
        .await
        .expect("set alpha default");

    // Forking without provider inherits alpha + gets alpha's default_config.
    let fork_alpha = fx
        .engine
        .fork_thread_with_provider(Some(&sib_a.thread_id), Some("Alpha Fork"), None)
        .await
        .expect("fork inherit");
    assert_eq!(fork_alpha.agent_provider.as_deref(), Some("alpha"));
    let fork_alpha_opts = fx.engine.agent_options(&fork_alpha.thread_id).await.unwrap();
    assert_eq!(fork_alpha_opts.config_options[0].current_value, "alpha-pro");

    // Forking from alpha with explicit beta uses beta and MUST NOT inherit alpha's default_config!
    let fork_beta = fx
        .engine
        .fork_thread_with_provider(Some(&sib_a.thread_id), Some("Beta Fork"), Some("beta"))
        .await
        .expect("fork override");
    assert_eq!(fork_beta.agent_provider.as_deref(), Some("beta"));
    let fork_beta_opts = fx.engine.agent_options(&fork_beta.thread_id).await.unwrap();
    assert_eq!(fork_beta_opts.config_options[0].current_value, "beta-fast");
    assert!(
        !fx.beta_records
            .lock()
            .unwrap()
            .config_updates
            .iter()
            .any(|(_, _, v)| v == "alpha-pro"),
        "alpha default_config leaked onto beta provider!"
    );

    // Run turns concurrently across alpha & beta siblings.
    let (res_a, res_b) = tokio::join!(
        fx.engine
            .send_turn_for_thread(&sib_a.thread_id, "hello-alpha"),
        fx.engine
            .send_turn_for_thread(&sib_b.thread_id, "hello-beta"),
    );
    res_a.expect("alpha turn");
    res_b.expect("beta turn");

    {
        let alpha_rec = fx.alpha_records.lock().unwrap();
        let beta_rec = fx.beta_records.lock().unwrap();
        assert_eq!(alpha_rec.spawned_count, 1);
        assert_eq!(beta_rec.spawned_count, 1);
        assert_eq!(alpha_rec.alive_count(), 1);
        assert_eq!(beta_rec.alive_count(), 1);
        assert_eq!(alpha_rec.shutdown_count, 0);
        assert_eq!(beta_rec.shutdown_count, 0);
        assert!(alpha_rec.prompts.iter().any(|(_, p)| p == "hello-alpha"));
        assert!(!alpha_rec.prompts.iter().any(|(_, p)| p == "hello-beta"));
        assert!(beta_rec.prompts.iter().any(|(_, p)| p == "hello-beta"));
        assert!(!beta_rec.prompts.iter().any(|(_, p)| p == "hello-alpha"));
    }

    assert!(
        fx.alpha_observer
            .registered
            .lock()
            .unwrap()
            .iter()
            .any(|(tid, _)| tid == &sib_a.thread_id)
    );
    assert!(
        fx.beta_observer
            .registered
            .lock()
            .unwrap()
            .iter()
            .any(|(tid, _)| tid == &sib_b.thread_id)
    );

    // Shutdown shuts down ALL provider drivers & observers.
    fx.engine.shutdown().await;
    assert_eq!(fx.alpha_records.lock().unwrap().shutdown_count, 1);
    assert_eq!(fx.beta_records.lock().unwrap().shutdown_count, 1);
    assert_eq!(fx.alpha_records.lock().unwrap().alive_count(), 0);
    assert_eq!(fx.beta_records.lock().unwrap().alive_count(), 0);
    assert!(fx.alpha_observer.shut_down.load(Ordering::SeqCst));
    assert!(fx.beta_observer.shut_down.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_switch_thread_provider_persists_resets_session_and_reroutes_turns() {
    let fx = MultiProviderFixture::new(None);
    let mut rx = fx.engine.subscribe_tui_events();

    let thread = fx
        .engine
        .create_agent_thread(Some("Switchable"), None, None, Some("alpha"))
        .await
        .expect("create");
    fx.engine
        .send_turn_for_thread(&thread.thread_id, "turn-on-alpha")
        .await
        .expect("alpha turn");

    let tid = CoreUuid::parse_str(&thread.thread_id).unwrap();
    let old_sid = fx
        .storage
        .get_thread(tid)
        .unwrap()
        .unwrap()
        .acp_session_id
        .expect("alpha acp_session_id recorded");

    // Drain prior events.
    while rx.try_recv().is_ok() {}

    let switched = fx
        .engine
        .switch_thread_provider(&thread.thread_id, "Beta Agent")
        .await
        .expect("switch provider");
    assert_eq!(switched, "beta");

    assert!(
        fx.alpha_records
            .lock()
            .unwrap()
            .closed_sessions
            .contains(&old_sid)
    );
    let updated_row = fx.storage.get_thread(tid).unwrap().unwrap();
    assert_eq!(updated_row.agent_provider.as_deref(), Some("beta"));
    assert_eq!(updated_row.acp_session_id, None);

    let mut saw_meta_update = false;
    while let Ok(ev) = rx.try_recv() {
        if let TuiEngineEvent::ThreadMetadataUpdated { meta } = ev {
            if meta
                .iter()
                .any(|m| m.thread_id == thread.thread_id && m.agent_provider.as_deref() == Some("beta"))
            {
                saw_meta_update = true;
            }
        }
    }
    assert!(saw_meta_update, "expected ThreadMetadataUpdated with beta");

    let opts = fx.engine.agent_options(&thread.thread_id).await.unwrap();
    assert_eq!(opts.config_options[0].current_value, "beta-fast");

    fx.engine
        .send_turn_for_thread(&thread.thread_id, "turn-on-beta")
        .await
        .expect("beta turn");

    assert_eq!(fx.alpha_records.lock().unwrap().prompts.len(), 1);
    assert!(
        fx.beta_records
            .lock()
            .unwrap()
            .prompts
            .iter()
            .any(|(_, p)| p == "turn-on-beta")
    );

    // Terminal thread cannot switch provider.
    let term = fx
        .engine
        .create_terminal(None, Some("Shell"), None, 24, 80)
        .await
        .expect("create terminal");
    let err = fx
        .engine
        .switch_thread_provider(&term.thread_id, "alpha")
        .await
        .expect_err("terminal switch rejected");
    assert!(matches!(err, EngineError::InvalidParams(_)));
    let _ = fx.engine.delete_thread(&term.thread_id).await;
}

#[tokio::test]
async fn test_default_provider_preference_unix_socket_rpcs_stale_fallback_and_tui_adapter() {
    let fx = MultiProviderFixture::new(Some("beta"));

    // Preference selects beta as default and puts it first.
    let listed = fx.engine.list_agent_providers().await.unwrap();
    assert_eq!(listed[0].id, "beta");
    assert!(listed[0].is_default);
    assert!(!listed[1].is_default);

    let default_created = fx
        .engine
        .create_agent_thread(Some("DefaultBeta"), None, None, None)
        .await
        .unwrap();
    assert_eq!(default_created.agent_provider.as_deref(), Some("beta"));

    // Unavailable pref falls back gracefully to first available provider ("alpha").
    fx.prefs
        .set("ui", "default_agent_provider", "gamma")
        .unwrap();
    let fallback_listed = fx.engine.list_agent_providers().await.unwrap();
    assert_eq!(fallback_listed[0].id, "alpha");
    assert!(fallback_listed[0].is_default);

    // Restore beta pref for RPC & TUI tests.
    fx.prefs
        .set("ui", "default_agent_provider", "beta")
        .unwrap();

    // Direct JSON-RPC dispatcher tests.
    let rpc_list_str = dispatch_json_rpc(
        fx.engine.as_ref(),
        r#"{"jsonrpc":"2.0","id":1,"method":"session.list_agent_providers","params":{}}"#,
    )
    .await;
    let rpc_list: Value = serde_json::from_str(&rpc_list_str).unwrap();
    assert_eq!(rpc_list["result"][0]["id"], "beta");
    assert_eq!(rpc_list["result"][0]["is_default"], true);

    let rpc_create_str = dispatch_json_rpc(
        fx.engine.as_ref(),
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session.create_root_thread",
            "params": {
                "title": "RpcSibling",
                "parent_thread_id": default_created.thread_id,
                "agent_provider": "alpha"
            }
        })
        .to_string(),
    )
    .await;
    let rpc_create: Value = serde_json::from_str(&rpc_create_str).unwrap();
    assert_eq!(rpc_create["result"]["agent_provider"], "alpha");
    let rpc_sib_id = rpc_create["result"]["thread_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Live Unix socket server + UnixSocketEngineClient roundtrip.
    let socket_path = fx._temp.path().join("multi-provider.sock");
    let engine_dyn: Arc<dyn GreinEngine> = Arc::clone(&fx.engine) as Arc<dyn GreinEngine>;
    let sock_for_srv = socket_path.clone();
    let daemon_task = tokio::spawn(async move {
        let _ = run_daemon_with_engine(sock_for_srv, engine_dyn).await;
    });
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let client = UnixSocketEngineClient::connect(&socket_path)
        .await
        .expect("connect socket client");
    let sock_providers = client.list_agent_providers().await.expect("sock list");
    assert_eq!(sock_providers[0].id, "beta");

    let sock_sib = client
        .create_agent_thread(
            Some("SockChild"),
            None,
            Some(&rpc_sib_id),
            Some("beta"),
        )
        .await
        .expect("sock create child");
    assert_eq!(sock_sib.agent_provider.as_deref(), Some("beta"));

    let sock_fork = client
        .fork_thread_with_provider(Some(&sock_sib.thread_id), Some("SockFork"), Some("alpha"))
        .await
        .expect("sock fork override");
    assert_eq!(sock_fork.agent_provider.as_deref(), Some("alpha"));

    let sock_switched = client
        .switch_thread_provider(&sock_fork.thread_id, "beta")
        .await
        .expect("sock switch");
    assert_eq!(sock_switched, "beta");

    daemon_task.abort();

    // Stale daemon -32601 graceful fallback on list_agent_providers.
    let stale_socket = fx._temp.path().join("stale-providers.sock");
    let stale_listener = UnixListener::bind(&stale_socket).unwrap();
    let stale_task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = stale_listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let id = req.get("id").cloned().unwrap_or(Value::Null);
                    let resp = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": "Method not found"}
                    });
                    if write.write_all(format!("{resp}\n").as_bytes()).await.is_err()
                        || write.flush().await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    let stale_client = UnixSocketEngineClient::connect(&stale_socket).await.unwrap();
    let stale_list = stale_client
        .list_agent_providers()
        .await
        .expect("stale -32601 degrades to empty list");
    assert!(stale_list.is_empty());
    stale_task.abort();

    // TUI adapter EngineClientPort<InProcessEngine> wiring.
    let port = EngineClientPort::new_with_mode_and_prefs(
        Arc::clone(&fx.engine),
        InitialSessionMode::Agent,
        Arc::clone(&fx.prefs) as Arc<dyn PreferenceRepository>,
    );
    let mut tui_rx = port.subscribe_events().await.expect("subscribe_events");
    let tui_providers = port.list_agent_providers().await.expect("tui list");
    assert_eq!(tui_providers[0].id, "beta");

    let tui_child = port
        .create_agent_thread(
            Some("TuiChild"),
            None,
            Some(&default_created.thread_id),
            Some("alpha"),
        )
        .await
        .expect("tui create");
    assert_eq!(tui_child.agent_provider.as_deref(), Some("alpha"));

    let tui_fork = port
        .fork_branch_with_provider(&tui_child.id, Some("TuiFork"), None)
        .await
        .expect("tui fork");
    assert_eq!(tui_fork.agent_provider.as_deref(), Some("alpha"));

    let tui_switched = port
        .switch_thread_provider(&tui_fork.id, "beta")
        .await
        .expect("tui switch");
    assert_eq!(tui_switched, "beta");

    // Confirm live event forwarding emitted ThreadCreated / ThreadMetadataUpdated.
    let mut saw_child_created = false;
    while let Ok(Some(ev)) = timeout(Duration::from_millis(200), tui_rx.recv()).await {
        if let TuiEngineEvent::ThreadCreated { thread } = ev {
            if thread.id == tui_child.id && thread.agent_provider.as_deref() == Some("alpha") {
                saw_child_created = true;
                break;
            }
        }
    }
    assert!(saw_child_created, "expected TuiEngineEvent::ThreadCreated for tui_child");

    fx.engine.shutdown().await;
}

#[allow(dead_code)]
fn assert_grein_engine_send_sync<T: GreinEngine + ?Sized>() {}

#[test]
fn assert_client_is_grein_engine() {
    assert_grein_engine_send_sync::<InProcessEngine>();
    assert_grein_engine_send_sync::<UnixSocketEngineClient>();
}
