use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use grein_acp::{AgentDriverFactory, StdioAgentDriverFactory};
use grein_workspace::environment::{EnvironmentDetector, WorkspaceEnvironment};
use grein_workspace::port::{
    GitCliInspector, GitInspectorPort, GitWorktreeDriver, ShadowCopyIsolator,
    WorkspaceIsolationPort,
};

use super::registry::{AgentPlugin, LifecyclePlugin, RegistryError, WorkspacePlugin};
use crate::discovery::{
    claude_acp_args, discover_acp_agents, resolve_agents_configuration, AcpAgent, AcpAgentKind,
    DiscoveryError,
};
use crate::engine::AcpBackendConfig;
use crate::subagent_watch::{AntigravityBrainObserver, NoopSubagentObserver, SubagentObserverPort};

pub struct AgyAgentPlugin {
    id: String,
    executable_path: Option<PathBuf>,
    auth_method: Option<String>,
    driver_factory: Arc<dyn AgentDriverFactory>,
    observer: Arc<AntigravityBrainObserver>,
}

impl AgyAgentPlugin {
    pub fn new(executable_path: Option<PathBuf>, auth_override: Option<Option<String>>) -> Self {
        let brain_root = AntigravityBrainObserver::default_brain_root()
            .unwrap_or_else(|| PathBuf::from(".grein/brain_missing"));
        let auth_method = auth_override.unwrap_or_else(|| Some("oauth-personal".to_string()));
        Self {
            id: "agent.agy".to_string(),
            executable_path,
            auth_method,
            driver_factory: Arc::new(StdioAgentDriverFactory::new()),
            observer: AntigravityBrainObserver::unattached(brain_root),
        }
    }

    pub fn with_brain_root(
        executable_path: Option<PathBuf>,
        brain_root: PathBuf,
        auth_override: Option<Option<String>>,
    ) -> Self {
        let auth_method = auth_override.unwrap_or_else(|| Some("oauth-personal".to_string()));
        Self {
            id: "agent.agy".to_string(),
            executable_path,
            auth_method,
            driver_factory: Arc::new(StdioAgentDriverFactory::new()),
            observer: AntigravityBrainObserver::unattached(brain_root),
        }
    }

    pub fn discover(search_path: Option<&str>, auth_override: Option<Option<String>>) -> Self {
        let found = discover_acp_agents(search_path)
            .into_iter()
            .find(|a| a.kind == AcpAgentKind::Agy)
            .map(|a| a.executable_path);
        Self::new(found, auth_override)
    }
}

#[async_trait]
impl LifecyclePlugin for AgyAgentPlugin {
    fn id(&self) -> &str {
        &self.id
    }

    async fn shutdown(&self) -> Result<(), RegistryError> {
        self.observer.shutdown();
        Ok(())
    }
}

impl AgentPlugin for AgyAgentPlugin {
    fn kind(&self) -> AcpAgentKind {
        AcpAgentKind::Agy
    }

    fn short_name(&self) -> String {
        "agy".into()
    }

    fn display_name(&self) -> String {
        "Antigravity (agy)".into()
    }

    fn executable_path(&self) -> Option<&Path> {
        self.executable_path.as_deref()
    }

    fn driver_factory(&self) -> Arc<dyn AgentDriverFactory> {
        Arc::clone(&self.driver_factory)
    }

    fn acp_backend_config(&self, cwd: &Path) -> Option<AcpBackendConfig> {
        self.executable_path.as_ref().map(|exe| AcpBackendConfig {
            command: exe.to_string_lossy().into_owned(),
            args: vec!["acp".to_string(), "--stdio".to_string()],
            auth_method: self.auth_method.clone(),
            cwd: cwd.to_path_buf(),
        })
    }

    fn subagent_observer(&self) -> Arc<dyn SubagentObserverPort> {
        Arc::clone(&self.observer) as Arc<dyn SubagentObserverPort>
    }
}

pub struct ClaudeAgentPlugin {
    id: String,
    executable_path: Option<PathBuf>,
    auth_method: Option<String>,
    driver_factory: Arc<dyn AgentDriverFactory>,
    observer: Arc<NoopSubagentObserver>,
}

impl ClaudeAgentPlugin {
    pub fn new(executable_path: Option<PathBuf>, auth_override: Option<Option<String>>) -> Self {
        Self {
            id: "agent.claude".to_string(),
            executable_path,
            auth_method: auth_override.unwrap_or(None),
            driver_factory: Arc::new(StdioAgentDriverFactory::new()),
            observer: Arc::new(NoopSubagentObserver::new()),
        }
    }

    pub fn discover(search_path: Option<&str>, auth_override: Option<Option<String>>) -> Self {
        let found = discover_acp_agents(search_path)
            .into_iter()
            .find(|a| a.kind == AcpAgentKind::Claude)
            .map(|a| a.executable_path);
        Self::new(found, auth_override)
    }
}

#[async_trait]
impl LifecyclePlugin for ClaudeAgentPlugin {
    fn id(&self) -> &str {
        &self.id
    }
}

impl AgentPlugin for ClaudeAgentPlugin {
    fn kind(&self) -> AcpAgentKind {
        AcpAgentKind::Claude
    }

    fn short_name(&self) -> String {
        "claude".into()
    }

    fn display_name(&self) -> String {
        "Claude Code".into()
    }

    fn install_hint(&self) -> Option<String> {
        Some(
            "npm install -g @agentclientprotocol/claude-agent-acp (or: NIXPKGS_ALLOW_UNFREE=1 nix shell --impure nixpkgs#claude-agent-acp)"
                .into(),
        )
    }

    fn executable_path(&self) -> Option<&Path> {
        self.executable_path.as_deref()
    }

    fn driver_factory(&self) -> Arc<dyn AgentDriverFactory> {
        Arc::clone(&self.driver_factory)
    }

    fn acp_backend_config(&self, cwd: &Path) -> Option<AcpBackendConfig> {
        self.executable_path.as_ref().map(|exe| AcpBackendConfig {
            command: exe.to_string_lossy().into_owned(),
            args: claude_acp_args(exe),
            auth_method: self.auth_method.clone(),
            cwd: cwd.to_path_buf(),
        })
    }

    fn subagent_observer(&self) -> Arc<dyn SubagentObserverPort> {
        Arc::clone(&self.observer) as Arc<dyn SubagentObserverPort>
    }
}

pub struct CustomAcpAgentPlugin {
    id: String,
    kind: AcpAgentKind,
    executable_path: Option<PathBuf>,
    args: Vec<String>,
    auth_method: Option<String>,
    driver_factory: Arc<dyn AgentDriverFactory>,
    observer: Arc<NoopSubagentObserver>,
}

impl CustomAcpAgentPlugin {
    pub fn new(
        id: impl Into<String>,
        kind: AcpAgentKind,
        executable_path: Option<PathBuf>,
        args: Vec<String>,
        auth_method: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            executable_path,
            args,
            auth_method,
            driver_factory: Arc::new(StdioAgentDriverFactory::new()),
            observer: Arc::new(NoopSubagentObserver::new()),
        }
    }

    pub fn from_env(
        bin_env: Option<String>,
        args_env: Option<String>,
        auth_env: Option<String>,
    ) -> Option<Self> {
        let bin = bin_env?;
        let trimmed = bin.trim();
        if trimmed.is_empty() {
            return None;
        }
        let args = args_env
            .map(|raw| raw.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        let auth_method = auth_env
            .map(|raw| raw.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(Self::new(
            "agent.custom.env",
            AcpAgentKind::Custom,
            Some(PathBuf::from(trimmed)),
            args,
            auth_method,
        ))
    }

    pub fn resolve_from_config(
        config_path: Option<&Path>,
        search_path: Option<&str>,
    ) -> Result<Vec<Self>, DiscoveryError> {
        let agents = resolve_agents_configuration(config_path, search_path)?;
        Ok(agents
            .into_iter()
            .enumerate()
            .map(|(idx, agent)| Self::from_discovered(idx, agent))
            .collect())
    }

    pub fn from_discovered(idx: usize, agent: AcpAgent) -> Self {
        let stem = agent
            .executable_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("custom");
        let args = if agent.args.is_empty() {
            match agent.kind {
                AcpAgentKind::Agy => vec!["acp".to_string(), "--stdio".to_string()],
                AcpAgentKind::Claude => claude_acp_args(&agent.executable_path),
                AcpAgentKind::Custom => Vec::new(),
            }
        } else {
            agent.args
        };
        let auth_method = match agent.kind {
            AcpAgentKind::Agy => Some("oauth-personal".to_string()),
            AcpAgentKind::Claude | AcpAgentKind::Custom => None,
        };
        Self::new(
            format!("agent.custom.{idx}.{stem}"),
            agent.kind,
            Some(agent.executable_path),
            args,
            auth_method,
        )
    }

    fn derived_short_name(&self) -> String {
        let trimmed_id = self.id.trim();
        if !trimmed_id.starts_with("agent.custom.") && !trimmed_id.is_empty() {
            return trimmed_id.to_string();
        }
        if let Some(stem) = self
            .executable_path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|s| s.to_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return stem.to_string();
        }
        let stripped = trimmed_id
            .strip_prefix("agent.custom.")
            .unwrap_or(trimmed_id)
            .trim();
        let stripped = match stripped.split_once('.') {
            Some((idx, rest)) if !idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit()) => rest,
            _ => stripped,
        };
        if stripped.is_empty() {
            "custom".to_string()
        } else {
            stripped.to_string()
        }
    }
}

#[async_trait]
impl LifecyclePlugin for CustomAcpAgentPlugin {
    fn id(&self) -> &str {
        &self.id
    }
}

impl AgentPlugin for CustomAcpAgentPlugin {
    fn kind(&self) -> AcpAgentKind {
        self.kind
    }

    fn short_name(&self) -> String {
        self.derived_short_name()
    }

    fn display_name(&self) -> String {
        let short = self.short_name();
        if short == "custom" {
            "Custom".to_string()
        } else {
            format!("Custom ({short})")
        }
    }

    fn executable_path(&self) -> Option<&Path> {
        self.executable_path.as_deref()
    }

    fn driver_factory(&self) -> Arc<dyn AgentDriverFactory> {
        Arc::clone(&self.driver_factory)
    }

    fn acp_backend_config(&self, cwd: &Path) -> Option<AcpBackendConfig> {
        self.executable_path.as_ref().map(|exe| AcpBackendConfig {
            command: exe.to_string_lossy().into_owned(),
            args: self.args.clone(),
            auth_method: self.auth_method.clone(),
            cwd: cwd.to_path_buf(),
        })
    }

    fn subagent_observer(&self) -> Arc<dyn SubagentObserverPort> {
        Arc::clone(&self.observer) as Arc<dyn SubagentObserverPort>
    }
}

pub struct GitWorktreePlugin {
    id: String,
    isolator: Arc<GitWorktreeDriver>,
    inspector: Arc<GitCliInspector>,
}

impl Default for GitWorktreePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl GitWorktreePlugin {
    pub fn new() -> Self {
        Self {
            id: "workspace.git_worktree".to_string(),
            isolator: Arc::new(GitWorktreeDriver::new()),
            inspector: Arc::new(GitCliInspector::new()),
        }
    }
}

#[async_trait]
impl LifecyclePlugin for GitWorktreePlugin {
    fn id(&self) -> &str {
        &self.id
    }
}

impl WorkspacePlugin for GitWorktreePlugin {
    fn isolation_mode(&self) -> &'static str {
        "git_worktree"
    }

    fn supports_cwd(&self, cwd: &Path) -> bool {
        EnvironmentDetector::default().detect(cwd).environment == WorkspaceEnvironment::GitRepository
    }

    fn workspace_isolator(&self) -> Arc<dyn WorkspaceIsolationPort> {
        Arc::clone(&self.isolator) as Arc<dyn WorkspaceIsolationPort>
    }

    fn git_inspector(&self) -> Arc<dyn GitInspectorPort> {
        Arc::clone(&self.inspector) as Arc<dyn GitInspectorPort>
    }
}

pub struct ShadowCopyPlugin {
    id: String,
    isolator: Arc<ShadowCopyIsolator>,
    inspector: Arc<GitCliInspector>,
}

impl Default for ShadowCopyPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ShadowCopyPlugin {
    pub fn new() -> Self {
        Self {
            id: "workspace.shadow_copy".to_string(),
            isolator: Arc::new(ShadowCopyIsolator::new()),
            inspector: Arc::new(GitCliInspector::new()),
        }
    }
}

#[async_trait]
impl LifecyclePlugin for ShadowCopyPlugin {
    fn id(&self) -> &str {
        &self.id
    }
}

impl WorkspacePlugin for ShadowCopyPlugin {
    fn isolation_mode(&self) -> &'static str {
        "shadow_copy"
    }

    fn supports_cwd(&self, cwd: &Path) -> bool {
        EnvironmentDetector::default().detect(cwd).environment
            == WorkspaceEnvironment::NonGitDirectory
    }

    fn workspace_isolator(&self) -> Arc<dyn WorkspaceIsolationPort> {
        Arc::clone(&self.isolator) as Arc<dyn WorkspaceIsolationPort>
    }

    fn git_inspector(&self) -> Arc<dyn GitInspectorPort> {
        Arc::clone(&self.inspector) as Arc<dyn GitInspectorPort>
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn agy_and_claude_plugins_build_expected_acp_configs() {
        let cwd = Path::new("/tmp");

        let agy = AgyAgentPlugin::new(Some(PathBuf::from("/usr/bin/agy")), None);
        assert_eq!(agy.kind(), AcpAgentKind::Agy);
        let agy_cfg = agy.acp_backend_config(cwd).expect("agy cfg");
        assert_eq!(agy_cfg.command, "/usr/bin/agy");
        assert_eq!(agy_cfg.args, vec!["acp", "--stdio"]);
        assert_eq!(agy_cfg.auth_method.as_deref(), Some("oauth-personal"));

        let claude = ClaudeAgentPlugin::new(Some(PathBuf::from("/usr/bin/claude")), None);
        assert_eq!(claude.kind(), AcpAgentKind::Claude);
        let claude_cfg = claude.acp_backend_config(cwd).expect("claude cfg");
        assert_eq!(claude_cfg.command, "/usr/bin/claude");
        assert!(claude_cfg.args.is_empty());
        assert_eq!(claude_cfg.auth_method, None);
    }

    #[test]
    fn custom_plugin_bridges_discovery_configuration_and_env() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cfg_path = temp.path().join("grein.toml");
        fs::write(
            &cfg_path,
            "[[agents]]\nname = \"my-acp\"\ncommand = [\"/opt/my-acp\", \"--stdio\"]\n",
        )
        .expect("write cfg");

        let resolved = CustomAcpAgentPlugin::resolve_from_config(Some(&cfg_path), None)
            .expect("resolve custom");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind(), AcpAgentKind::Custom);
        let cfg = resolved[0].acp_backend_config(temp.path()).expect("cfg");
        assert_eq!(cfg.command, "/opt/my-acp");
        assert_eq!(cfg.args, vec!["--stdio"]);

        let from_env = CustomAcpAgentPlugin::from_env(
            Some("/usr/local/bin/custom-acp".into()),
            Some("--flag1 --flag2".into()),
            Some("api-key".into()),
        )
        .expect("from_env");
        let env_cfg = from_env.acp_backend_config(temp.path()).expect("env cfg");
        assert_eq!(env_cfg.command, "/usr/local/bin/custom-acp");
        assert_eq!(env_cfg.args, vec!["--flag1", "--flag2"]);
        assert_eq!(env_cfg.auth_method.as_deref(), Some("api-key"));
    }

    #[test]
    fn workspace_plugins_detect_git_vs_non_git_cwd() {
        let temp = tempfile::tempdir().expect("tempdir");
        let non_git = temp.path().join("plain");
        let git_repo = temp.path().join("repo");
        fs::create_dir_all(&non_git).unwrap();
        fs::create_dir_all(&git_repo).unwrap();
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(&git_repo)
            .status()
            .expect("git init")
            .success());

        let wt = GitWorktreePlugin::new();
        let shadow = ShadowCopyPlugin::new();
        assert!(wt.supports_cwd(&git_repo));
        assert!(!wt.supports_cwd(&non_git));
        assert!(shadow.supports_cwd(&non_git));
        assert!(!shadow.supports_cwd(&git_repo));
    }

    #[test]
    fn agent_plugins_expose_metadata_and_claude_adapter_args() {
        let cwd = Path::new("/tmp");

        let agy = AgyAgentPlugin::new(Some(PathBuf::from("/usr/bin/agy")), None);
        assert_eq!(agy.short_name(), "agy");
        assert_eq!(agy.display_name(), "Antigravity (agy)");
        assert_eq!(agy.install_hint(), None);

        let claude = ClaudeAgentPlugin::new(Some(PathBuf::from("/usr/bin/claude")), None);
        assert_eq!(claude.short_name(), "claude");
        assert_eq!(claude.display_name(), "Claude Code");
        assert_eq!(
            claude.install_hint().as_deref(),
            Some("npm install -g @agentclientprotocol/claude-agent-acp (or: NIXPKGS_ALLOW_UNFREE=1 nix shell --impure nixpkgs#claude-agent-acp)")
        );
        assert!(claude
            .acp_backend_config(cwd)
            .expect("claude cfg")
            .args
            .is_empty());

        let claude_agent_acp =
            ClaudeAgentPlugin::new(Some(PathBuf::from("/usr/local/bin/claude-agent-acp")), None);
        assert!(claude_agent_acp
            .acp_backend_config(cwd)
            .expect("claude-agent-acp cfg")
            .args
            .is_empty());

        let claude_code_acp =
            ClaudeAgentPlugin::new(Some(PathBuf::from("/usr/local/bin/claude-code-acp")), None);
        assert!(claude_code_acp
            .acp_backend_config(cwd)
            .expect("claude-code-acp cfg")
            .args
            .is_empty());

        let custom_env =
            CustomAcpAgentPlugin::from_env(Some("/usr/local/bin/custom-acp".into()), None, None)
                .expect("from_env");
        assert_eq!(custom_env.short_name(), "custom-acp");
        assert_eq!(custom_env.display_name(), "Custom (custom-acp)");
        assert_eq!(custom_env.install_hint(), None);

        let custom_discovered = CustomAcpAgentPlugin::from_discovered(
            0,
            AcpAgent {
                kind: AcpAgentKind::Custom,
                executable_path: PathBuf::from("/opt/my-acp"),
                args: Vec::new(),
            },
        );
        assert_eq!(custom_discovered.short_name(), "my-acp");
        assert_eq!(custom_discovered.display_name(), "Custom (my-acp)");

        let custom_claude_acp = CustomAcpAgentPlugin::from_discovered(
            1,
            AcpAgent {
                kind: AcpAgentKind::Claude,
                executable_path: PathBuf::from("/opt/bin/claude-agent-acp"),
                args: Vec::new(),
            },
        );
        assert!(custom_claude_acp
            .acp_backend_config(cwd)
            .expect("custom claude cfg")
            .args
            .is_empty());

        let custom_agy_default = CustomAcpAgentPlugin::from_discovered(
            2,
            AcpAgent {
                kind: AcpAgentKind::Agy,
                executable_path: PathBuf::from("/opt/bin/agy"),
                args: Vec::new(),
            },
        );
        assert_eq!(
            custom_agy_default
                .acp_backend_config(cwd)
                .expect("custom agy cfg")
                .args,
            vec!["acp", "--stdio"]
        );

        let custom_agy_override = CustomAcpAgentPlugin::from_discovered(
            3,
            AcpAgent {
                kind: AcpAgentKind::Agy,
                executable_path: PathBuf::from("/opt/bin/agy"),
                args: vec!["--custom-arg".to_string()],
            },
        );
        assert_eq!(
            custom_agy_override
                .acp_backend_config(cwd)
                .expect("custom agy override cfg")
                .args,
            vec!["--custom-arg"]
        );

        let explicit_id =
            CustomAcpAgentPlugin::new("bespoke", AcpAgentKind::Custom, None, Vec::new(), None);
        assert_eq!(explicit_id.short_name(), "bespoke");
        assert_eq!(explicit_id.display_name(), "Custom (bespoke)");
    }

    #[test]
    fn claude_discovery_ignores_plain_claude_and_custom_plugin_preserves_npx_args() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("mkdir bin");
        let plain_claude = bin_dir.join("claude");
        fs::write(&plain_claude, "#!/bin/sh\nexit 0\n").expect("write claude");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&plain_claude).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&plain_claude, perms).unwrap();
        }
        let search_plain = bin_dir.to_string_lossy().into_owned();
        let unavailable = ClaudeAgentPlugin::discover(Some(&search_plain), None);
        assert!(!unavailable.is_available());
        assert_eq!(unavailable.executable_path(), None);
        assert_eq!(unavailable.acp_backend_config(temp.path()), None);

        let adapter_bin = bin_dir.join("claude-agent-acp");
        fs::write(&adapter_bin, "#!/bin/sh\nexit 0\n").expect("write adapter");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&adapter_bin).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&adapter_bin, perms).unwrap();
        }
        let available = ClaudeAgentPlugin::discover(Some(&search_plain), None);
        assert!(available.is_available());
        assert_eq!(available.executable_path(), Some(adapter_bin.as_path()));
        assert!(available
            .acp_backend_config(temp.path())
            .expect("adapter cfg")
            .args
            .is_empty());

        let cfg_path = temp.path().join("agents.toml");
        fs::write(
            &cfg_path,
            "[[agents]]\nname = \"claude\"\ncommand = [\"npx\", \"-y\", \"@agentclientprotocol/claude-agent-acp\"]\n",
        )
        .expect("write cfg");
        let resolved = CustomAcpAgentPlugin::resolve_from_config(Some(&cfg_path), None)
            .expect("resolve from config");
        assert_eq!(resolved.len(), 1);
        let cfg = resolved[0].acp_backend_config(temp.path()).expect("npx cfg");
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "@agentclientprotocol/claude-agent-acp"]);
    }
}
