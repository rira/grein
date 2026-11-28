pub mod adapter;
pub mod plugins;
pub mod registry;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use grein_acp::StdioAgentDriverFactory;
use grein_core::port::outbound::{PreferenceRepository, SessionRepository};
use grein_core::prefs::{default_prefs_db_path, JsonSettingsCodec, TomlSettingsCodec, UserPrefsStore};
use grein_core::storage::sqlite::SqliteStorage;
use grein_workspace::port::{GitCliInspector, GitWorktreeDriver, PtyShellExecutor, UnixPtySpawner};

use crate::args::{AttachArgs, Cli, Command, DaemonArgs, KeysArgs, SettingsCommand, ThemeCommand};
use crate::daemon::{resolve_state_dir, run_daemon_with_engine};
use crate::discovery::{discover_acp_agents, resolve_agents_configuration, AcpAgent, AcpAgentKind};
use crate::doctor::{handle_doctor_with, DoctorDeps};
use crate::engine::{
    EngineDependencies, EngineError, GreinEngine, InProcessEngine, RegisteredAgentProvider,
    ThreadUseCasePort, UnixSocketEngineClient,
};
use crate::headless::{handle_agents_with, handle_status_with, handle_threads_with, HeadlessDeps};
use crate::settings_cmd::{handle_settings_with, handle_theme_with, SettingsDeps};
use crate::subagent_watch::{NoopSubagentObserver, SubagentObserverPort};
use crate::terminal::cmd::handle_terminal_command;

use adapter::{EngineClientPort, InitialSessionMode, LazyPrefsRepository};
use plugins::{
    AgyAgentPlugin, ClaudeAgentPlugin, CustomAcpAgentPlugin, GitWorktreePlugin, ShadowCopyPlugin,
};
use registry::{FrozenPluginRegistry, PluginRegistryBuilder, RegistryError};

#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    pub cwd: PathBuf,
    pub search_path: Option<String>,
    pub acp_config_path: Option<PathBuf>,
    pub agent_bin_env: Option<String>,
    pub agent_args_env: Option<String>,
    pub agent_auth_env: Option<String>,
    pub prefs_db_path: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub default_db_path: PathBuf,
    pub default_socket_path: PathBuf,
    pub initial_mode: InitialSessionMode,
}

impl BootstrapConfig {
    pub fn from_env(initial_mode: InitialSessionMode) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let state_dir = resolve_state_dir(&cwd);
        Self {
            default_db_path: state_dir.join("session.db"),
            default_socket_path: state_dir.join("daemon.sock"),
            cwd,
            search_path: std::env::var("PATH").ok(),
            acp_config_path: None,
            agent_bin_env: std::env::var("GREIN_ACP_BIN").ok(),
            agent_args_env: std::env::var("GREIN_ACP_ARGS").ok(),
            agent_auth_env: std::env::var("GREIN_ACP_AUTH_METHOD").ok(),
            prefs_db_path: default_prefs_db_path(),
            state_dir,
            initial_mode,
        }
    }

    fn acp_explicitly_disabled(&self) -> bool {
        self.agent_bin_env
            .as_deref()
            .is_some_and(|raw| raw.trim().is_empty())
    }

    fn auth_override(&self) -> Option<Option<String>> {
        self.agent_auth_env.as_deref().map(|raw| {
            let trimmed = raw.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        })
    }
}

pub fn build_registry(config: &BootstrapConfig) -> Result<FrozenPluginRegistry, RegistryError> {
    let mut builder = PluginRegistryBuilder::new();
    builder.register_workspace_plugin(Arc::new(GitWorktreePlugin::new()))?;
    builder.register_workspace_plugin(Arc::new(ShadowCopyPlugin::new()))?;

    let disabled = config.acp_explicitly_disabled();
    let auth_override = config.auth_override();

    if let Some(custom_env) = CustomAcpAgentPlugin::from_env(
        config.agent_bin_env.clone(),
        config.agent_args_env.clone(),
        auth_override.clone().flatten(),
    ) {
        builder.register_agent_plugin(Arc::new(custom_env))?;
    }

    let resolved: Vec<AcpAgent> = if disabled {
        Vec::new()
    } else {
        resolve_agents_configuration(
            config.acp_config_path.as_deref(),
            config.search_path.as_deref(),
        )
        .unwrap_or_else(|_| discover_acp_agents(config.search_path.as_deref()))
    };

    let mut agy_path = None;
    let mut claude_path = None;
    let mut extras = Vec::new();

    for (idx, agent) in resolved.into_iter().enumerate() {
        match agent.kind {
            AcpAgentKind::Agy if agy_path.is_none() => {
                agy_path = Some(agent.executable_path);
            }
            AcpAgentKind::Claude if claude_path.is_none() => {
                claude_path = Some(agent.executable_path);
            }
            _ => extras.push((idx, agent)),
        }
    }

    builder.register_agent_plugin(Arc::new(AgyAgentPlugin::new(
        agy_path,
        auth_override.clone(),
    )))?;
    builder.register_agent_plugin(Arc::new(ClaudeAgentPlugin::new(claude_path, auth_override)))?;

    for (idx, agent) in extras {
        builder.register_agent_plugin(Arc::new(CustomAcpAgentPlugin::from_discovered(idx, agent)))?;
    }

    Ok(builder.freeze())
}

pub fn build_prefs_repository(config: &BootstrapConfig) -> Arc<dyn PreferenceRepository> {
    Arc::new(LazyPrefsRepository::for_path(config.prefs_db_path.clone()))
}

pub fn build_engine_dependencies(
    storage: Arc<dyn SessionRepository>,
    config: &BootstrapConfig,
    registry: &FrozenPluginRegistry,
) -> EngineDependencies {
    let ws_plugin = registry.select_workspace_plugin(&config.cwd);
    let workspace = ws_plugin
        .map(|p| p.workspace_isolator())
        .unwrap_or_else(|| Arc::new(GitWorktreeDriver::new()));
    let git_inspector = ws_plugin
        .map(|p| p.git_inspector())
        .unwrap_or_else(|| Arc::new(GitCliInspector::new()));

    let active_agent = if config.acp_explicitly_disabled() {
        None
    } else {
        registry.active_agent()
    };

    let (agent_factory, acp_config, subagent_observer) = match active_agent {
        Some(agent) => (
            agent.driver_factory(),
            agent.acp_backend_config(&config.cwd),
            agent.subagent_observer(),
        ),
        None => (
            Arc::new(StdioAgentDriverFactory::new()) as Arc<dyn grein_acp::AgentDriverFactory>,
            None,
            Arc::new(NoopSubagentObserver::new()) as Arc<dyn SubagentObserverPort>,
        ),
    };

    let providers: Vec<RegisteredAgentProvider> = registry
        .agent_plugins()
        .iter()
        .map(|agent| {
            let cfg_opt = if config.acp_explicitly_disabled() {
                None
            } else {
                agent.acp_backend_config(&config.cwd)
            };
            let available =
                !config.acp_explicitly_disabled() && agent.is_available() && cfg_opt.is_some();
            let executable = if config.acp_explicitly_disabled() {
                None
            } else {
                cfg_opt.as_ref().map(|c| c.command.clone()).or_else(|| {
                    agent
                        .executable_path()
                        .map(|p| p.to_string_lossy().into_owned())
                })
            };
            RegisteredAgentProvider {
                id: agent.short_name(),
                plugin_id: agent.id().to_string(),
                display_name: agent.display_name(),
                kind: agent.kind().label().to_string(),
                available,
                executable,
                args: cfg_opt.as_ref().map(|c| c.args.clone()).unwrap_or_default(),
                auth_method: cfg_opt.and_then(|c| c.auth_method),
                install_hint: agent.install_hint(),
                driver_factory: agent.driver_factory(),
                subagent_observer: agent.subagent_observer(),
            }
        })
        .collect();

    EngineDependencies {
        storage,
        agent_factory,
        policy: None,
        pty_spawner: Arc::new(UnixPtySpawner::new()),
        workspace,
        git_inspector,
        shell: Arc::new(PtyShellExecutor::new()),
        subagent_observer,
        cwd: config.cwd.clone(),
        acp_config,
        providers,
        prefs: Some(build_prefs_repository(config)),
    }
}

pub fn build_engine_for_db(
    db_path: &Path,
    config: &BootstrapConfig,
    registry: &FrozenPluginRegistry,
) -> Result<InProcessEngine, EngineError> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::Storage(e.to_string()))?;
        }
    }
    let storage =
        SqliteStorage::open(db_path).map_err(|e| EngineError::Storage(e.to_string()))?;
    InProcessEngine::with_dependencies(build_engine_dependencies(
        Arc::new(storage),
        config,
        registry,
    ))
}

pub fn build_in_memory_engine(
    config: &BootstrapConfig,
    registry: &FrozenPluginRegistry,
) -> Result<Arc<dyn GreinEngine>, EngineError> {
    let storage =
        SqliteStorage::in_memory().map_err(|e| EngineError::Storage(e.to_string()))?;
    let engine = InProcessEngine::with_dependencies(build_engine_dependencies(
        Arc::new(storage),
        config,
        registry,
    ))?;
    Ok(Arc::new(engine))
}

pub fn build_doctor_deps(config: &BootstrapConfig, registry: &FrozenPluginRegistry) -> DoctorDeps {
    let discovered = registry.discovered_agents();
    let cfg_path = config.acp_config_path.clone();
    let search_path = config.search_path.clone();
    DoctorDeps {
        cwd: config.cwd.clone(),
        prefs_db_path: config.prefs_db_path.clone(),
        session_db_probe: Arc::new(|path| {
            SqliteStorage::open(path)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }),
        prefs_db_probe: Arc::new(|path| {
            UserPrefsStore::open(path)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }),
        agent_discoverer: Arc::new(move || match cfg_path.as_deref() {
            Some(path) => resolve_agents_configuration(Some(path), search_path.as_deref()),
            None => Ok(discovered.clone()),
        }),
    }
}

pub fn build_headless_deps(
    config: &BootstrapConfig,
    registry: &FrozenPluginRegistry,
) -> HeadlessDeps {
    let ws_plugin = registry.select_workspace_plugin(&config.cwd);
    let workspace = ws_plugin
        .map(|p| p.workspace_isolator())
        .unwrap_or_else(|| Arc::new(GitWorktreeDriver::new()));
    let git_inspector = ws_plugin
        .map(|p| p.git_inspector())
        .unwrap_or_else(|| Arc::new(GitCliInspector::new()));
    let search_path = config.search_path.clone();

    HeadlessDeps {
        cwd: config.cwd.clone(),
        default_db_path: config.default_db_path.clone(),
        default_socket_path: config.default_socket_path.clone(),
        open_thread_engine: Arc::new(move |db_path, cwd| {
            let storage = SqliteStorage::open(db_path)
                .map_err(|e| EngineError::Storage(e.to_string()))?;
            let deps = EngineDependencies {
                storage: Arc::new(storage),
                agent_factory: Arc::new(StdioAgentDriverFactory::new()),
                policy: None,
                pty_spawner: Arc::new(UnixPtySpawner::new()),
                workspace: Arc::clone(&workspace),
                git_inspector: Arc::clone(&git_inspector),
                shell: Arc::new(PtyShellExecutor::new()),
                subagent_observer: Arc::new(NoopSubagentObserver::new()),
                cwd: cwd.to_path_buf(),
                acp_config: None,
                providers: Vec::new(),
                prefs: None,
            };
            let engine = InProcessEngine::with_dependencies(deps)?;
            Ok(Arc::new(engine) as Arc<dyn ThreadUseCasePort>)
        }),
        resolve_agents: Arc::new(move |cfg| resolve_agents_configuration(cfg, search_path.as_deref())),
    }
}

pub fn build_settings_deps(
    config: &BootstrapConfig,
    read_only: bool,
) -> Result<SettingsDeps, Box<dyn std::error::Error + Send + Sync>> {
    let read_repo: Arc<dyn PreferenceRepository> = match config.prefs_db_path.as_deref() {
        Some(path) if path.exists() => Arc::new(UserPrefsStore::open(path)?),
        _ => Arc::new(UserPrefsStore::in_memory()?),
    };
    let write_repo: Option<Arc<dyn PreferenceRepository>> = if read_only {
        None
    } else {
        match config.prefs_db_path.as_deref() {
            Some(path) => Some(Arc::new(UserPrefsStore::open(path)?)),
            None => None,
        }
    };
    Ok(SettingsDeps {
        read_repo,
        write_repo,
        toml_codec: Arc::new(TomlSettingsCodec),
        json_codec: Arc::new(JsonSettingsCodec),
    })
}

async fn dispatch_cli(
    command: Option<Command>,
    config: &BootstrapConfig,
    registry: &FrozenPluginRegistry,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        None | Some(Command::Standalone {}) => {
            if std::io::stdout().is_terminal() {
                let engine = Arc::new(build_engine_for_db(
                    &config.default_db_path,
                    config,
                    registry,
                )?);
                let prefs = build_prefs_repository(config);
                let port = Arc::new(EngineClientPort::new_with_mode_and_prefs(
                    engine,
                    config.initial_mode,
                    prefs,
                ));
                let services = grein_tui::TuiServices::default().with_terminal(
                    grein_tui::terminal::providers::default_adapter(
                        &grein_tui::terminal::TerminalEnv::from_env(),
                    ),
                );
                grein_tui::run_with_engine_and_services(port, services).await?;
            }
        }
        Some(Command::Daemon(DaemonArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(|| config.default_socket_path.clone());
            let db_path = socket_path
                .parent()
                .map(|p| p.join("session.db"))
                .unwrap_or_else(|| config.default_db_path.clone());
            let engine: Arc<dyn GreinEngine> =
                Arc::new(build_engine_for_db(&db_path, config, registry)?);
            run_daemon_with_engine(socket_path, engine).await?;
        }
        Some(Command::Attach(AttachArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(|| config.default_socket_path.clone());
            let transport = Arc::new(UnixSocketEngineClient::connect(&socket_path).await?);
            if std::io::stdout().is_terminal() {
                let prefs = build_prefs_repository(config);
                let port = Arc::new(EngineClientPort::new_with_mode_and_prefs(
                    transport,
                    config.initial_mode,
                    prefs,
                ));
                let services = grein_tui::TuiServices::default().with_terminal(
                    grein_tui::terminal::providers::default_adapter(
                        &grein_tui::terminal::TerminalEnv::from_env(),
                    ),
                );
                grein_tui::run_with_engine_and_services(port, services).await?;
            }
        }
        Some(Command::Threads(args)) => {
            let deps = build_headless_deps(config, registry);
            handle_threads_with(&args, &deps).await?;
        }
        Some(Command::Status(args)) => {
            let deps = build_headless_deps(config, registry);
            handle_status_with(&args, &deps).await?;
        }
        Some(Command::Agents(args)) => {
            let deps = build_headless_deps(config, registry);
            handle_agents_with(&args, &deps).await?;
        }
        Some(Command::Doctor(args)) => {
            let deps = build_doctor_deps(config, registry);
            handle_doctor_with(&args, &deps)?;
        }
        Some(Command::Keys(KeysArgs { no_enhance, raw: _ })) => {
            grein_tui::run_key_inspector(grein_tui::KeyInspectorOptions {
                enhance_keyboard: !no_enhance,
                ..Default::default()
            })?;
        }
        Some(Command::Terminal(args)) => {
            handle_terminal_command(&args)?;
        }
        Some(Command::Settings(args)) => {
            let read_only = matches!(args.command, SettingsCommand::Export(_));
            let deps = build_settings_deps(config, read_only)?;
            handle_settings_with(&args, &deps)?;
        }
        Some(Command::Theme(args)) => {
            let read_only = matches!(args.command, ThemeCommand::Export(_));
            let deps = build_settings_deps(config, read_only)?;
            handle_theme_with(&args, &deps)?;
        }
    }
    Ok(())
}

pub async fn run_cli(cli: Cli) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let initial_mode = if cli.agent {
        InitialSessionMode::Agent
    } else {
        InitialSessionMode::Terminal
    };
    let config = BootstrapConfig::from_env(initial_mode);
    let registry = build_registry(&config)?;
    registry.start_all().await?;

    let run_res = dispatch_cli(cli.command, &config, &registry).await;
    let shutdown_res = registry.shutdown_all().await;

    run_res?;
    shutdown_res?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{SettingsExportArgs, StatusArgs, ThreadsArgs};
    use crate::doctor::collect_doctor_report_with;
    use crate::headless::{collect_status_output_with, collect_threads_output_with};
    use crate::standalone::spawn_standalone_pair_with_engine;

    #[tokio::test]
    async fn bootstrap_composes_registry_dependencies_and_command_adapters() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prefs_db = temp.path().join("prefs.db");
        let db_path = temp.path().join("session.db");
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir bin");
        let fake_agy = bin_dir.join("agy");
        std::fs::write(&fake_agy, "#!/bin/sh\n").expect("write fake agy");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_agy).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_agy, perms).expect("chmod");
        }

        let config = BootstrapConfig {
            cwd: temp.path().to_path_buf(),
            search_path: Some(bin_dir.to_string_lossy().into_owned()),
            acp_config_path: None,
            agent_bin_env: None,
            agent_args_env: None,
            agent_auth_env: None,
            prefs_db_path: Some(prefs_db.clone()),
            state_dir: temp.path().join(".grein"),
            default_db_path: db_path.clone(),
            default_socket_path: temp.path().join("daemon.sock"),
            initial_mode: InitialSessionMode::Terminal,
        };

        let registry = build_registry(&config).expect("build_registry");
        assert!(registry.plugin_ids().contains(&"agent.agy"));
        assert!(registry.plugin_ids().contains(&"agent.claude"));
        assert!(registry.plugin_ids().contains(&"workspace.git_worktree"));
        assert!(registry.plugin_ids().contains(&"workspace.shadow_copy"));
        assert_eq!(
            registry.select_workspace_plugin(&config.cwd).map(|p| p.isolation_mode()),
            Some("shadow_copy")
        );
        assert_eq!(registry.active_agent().map(|p| p.kind()), Some(AcpAgentKind::Agy));

        registry.start_all().await.expect("start_all");

        let engine = Arc::new(build_engine_for_db(&db_path, &config, &registry).expect("engine"));
        engine.ensure_main_thread().await.expect("ensure main");

        let headless_deps = build_headless_deps(&config, &registry);
        let threads = collect_threads_output_with(
            &ThreadsArgs {
                json: true,
                db: Some(db_path.clone()),
            },
            &headless_deps,
        )
        .await
        .expect("threads");
        assert_eq!(threads.len(), 1);
        let status = collect_status_output_with(
            &StatusArgs {
                json: true,
                socket: None,
                db: Some(db_path.clone()),
            },
            &headless_deps,
        )
        .await;
        assert_eq!(status.thread_count, 1);

        let doctor_deps = build_doctor_deps(&config, &registry);
        let report = collect_doctor_report_with(&doctor_deps);
        assert!(report
            .checks
            .iter()
            .any(|c| c.name == "acp_agents" && c.detail.contains("agy")));

        let settings_deps = build_settings_deps(&config, true).expect("settings deps");
        handle_settings_with(
            &crate::args::SettingsArgs {
                command: SettingsCommand::Export(SettingsExportArgs::default()),
            },
            &settings_deps,
        )
        .expect("settings export");

        let standalone_engine = build_in_memory_engine(&config, &registry).expect("in_memory");
        let (mut client, shutdown_tx, handle) =
            spawn_standalone_pair_with_engine(standalone_engine);
        client.ping().await.expect("standalone ping");
        let _ = shutdown_tx.send(());
        handle.await.unwrap().expect("standalone exit");

        registry.shutdown_all().await.expect("shutdown_all");
    }
}
