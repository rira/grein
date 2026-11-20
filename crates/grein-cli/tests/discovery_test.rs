use grein_cli::discovery::{
    discover_acp_agents, resolve_agents_configuration, AcpAgent, AcpAgentKind, DiscoveryError,
};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Plain test fixture: a temp directory with fake executables and an optional config file.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn bin_dir(&self) -> PathBuf {
        let p = self.dir.path().join("bin");
        fs::create_dir_all(&p).expect("mkdir bin");
        p
    }

    fn add_binary(&self, name: &str) -> PathBuf {
        let p = self.bin_dir().join(name);
        fs::write(&p, "#!/bin/sh\nexit 0\n").expect("write bin");
        make_executable(&p);
        p
    }

    fn search_path(&self) -> String {
        self.bin_dir().to_string_lossy().into_owned()
    }

    fn write_config(&self, contents: &str) -> PathBuf {
        let p = self.dir.path().join("config.toml");
        fs::write(&p, contents).expect("write config");
        p
    }

    fn missing_config(&self) -> PathBuf {
        self.dir.path().join("does-not-exist.toml")
    }
}

fn kinds(agents: &[AcpAgent]) -> Vec<AcpAgentKind> {
    agents.iter().map(|a| a.kind.clone()).collect()
}

#[test]
fn test_path_discovery_preserves_agy_and_claude() {
    let fx = Fixture::new();
    let agy = fx.add_binary("agy");
    let claude = fx.add_binary("claude");
    let search = fx.search_path();

    let agents = discover_acp_agents(Some(&search));

    assert_eq!(agents.len(), 2);
    let agy_found = agents
        .iter()
        .find(|a| a.kind == AcpAgentKind::Agy)
        .expect("agy discovered");
    assert_eq!(agy_found.executable_path, agy);
    let claude_found = agents
        .iter()
        .find(|a| a.kind == AcpAgentKind::Claude)
        .expect("claude discovered");
    assert_eq!(claude_found.executable_path, claude);
}

#[test]
fn test_none_config_path_falls_back_to_path_discovery() {
    let fx = Fixture::new();
    let agy = fx.add_binary("agy");
    let search = fx.search_path();

    let agents = resolve_agents_configuration(None::<&Path>, Some(&search)).expect("resolve");

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, AcpAgentKind::Agy);
    assert_eq!(agents[0].executable_path, agy);
}

#[test]
fn test_missing_config_file_falls_back_to_path_discovery() {
    let fx = Fixture::new();
    let claude = fx.add_binary("claude");
    let search = fx.search_path();
    let missing = fx.missing_config();

    let agents = resolve_agents_configuration(Some(&missing), Some(&search)).expect("resolve");

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, AcpAgentKind::Claude);
    assert_eq!(agents[0].executable_path, claude);
}

#[test]
fn test_empty_config_falls_back_to_path_discovery() {
    let fx = Fixture::new();
    let agy = fx.add_binary("agy");
    let claude = fx.add_binary("claude");
    let search = fx.search_path();
    let cfg = fx.write_config("# no agents declared\n\n[ui]\ntheme = \"dark\"\n");

    let agents = resolve_agents_configuration(Some(&cfg), Some(&search)).expect("resolve");

    assert_eq!(agents.len(), 2);
    assert!(agents
        .iter()
        .any(|a| a.kind == AcpAgentKind::Agy && a.executable_path == agy));
    assert!(agents
        .iter()
        .any(|a| a.kind == AcpAgentKind::Claude && a.executable_path == claude));
}

#[test]
fn test_agents_array_of_tables_parsed_with_executable_and_path_fields() {
    let fx = Fixture::new();
    // PATH contains an agent that must NOT show up, since config declares agents.
    fx.add_binary("claude");
    let search = fx.search_path();
    let cfg = fx.write_config(
        r#"
[[agents]]
name = "agy"
kind = "agy"
executable = "/opt/agents/agy-bin"

[[agents]]
name = "claude"
kind = "claude"
path = "/opt/agents/claude-bin"
"#,
    );

    let agents = resolve_agents_configuration(Some(&cfg), Some(&search)).expect("resolve");

    assert_eq!(
        kinds(&agents),
        vec![AcpAgentKind::Agy, AcpAgentKind::Claude]
    );
    assert_eq!(
        agents[0].executable_path,
        PathBuf::from("/opt/agents/agy-bin")
    );
    assert_eq!(
        agents[1].executable_path,
        PathBuf::from("/opt/agents/claude-bin")
    );
}

#[test]
fn test_agents_entry_with_command_array_uses_first_element_as_executable() {
    let fx = Fixture::new();
    let cfg = fx.write_config(
        r#"
[[agents]]
name = "claude"
command = ["/usr/local/bin/claude", "--acp", "--verbose"]
"#,
    );

    let agents = resolve_agents_configuration(Some(&cfg), None).expect("resolve");

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, AcpAgentKind::Claude);
    assert_eq!(
        agents[0].executable_path,
        PathBuf::from("/usr/local/bin/claude")
    );
}

#[test]
fn test_single_agent_table_command_and_protocol_parsed() {
    let fx = Fixture::new();
    let cfg = fx.write_config(
        r#"
[agent]
command = ["/usr/bin/agy", "serve"]
protocol = "acp"
"#,
    );

    let agents = resolve_agents_configuration(Some(&cfg), None).expect("resolve");

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, AcpAgentKind::Agy);
    assert_eq!(agents[0].executable_path, PathBuf::from("/usr/bin/agy"));
}

#[test]
fn test_unknown_names_and_commands_map_to_custom_kind() {
    let fx = Fixture::new();
    let cfg = fx.write_config(
        r#"
[[agents]]
name = "my-special-agent"
executable = "/opt/special/run"

[[agents]]
name = "another"
command = ["/opt/other/tool", "--flag"]
"#,
    );

    let agents = resolve_agents_configuration(Some(&cfg), None).expect("resolve");

    assert_eq!(
        kinds(&agents),
        vec![AcpAgentKind::Custom, AcpAgentKind::Custom]
    );
    assert_eq!(agents[0].executable_path, PathBuf::from("/opt/special/run"));
    assert_eq!(agents[1].executable_path, PathBuf::from("/opt/other/tool"));
}

#[test]
fn test_single_agent_table_with_custom_command_maps_to_custom() {
    let fx = Fixture::new();
    let cfg =
        fx.write_config("[agent]\ncommand = [\"/opt/bespoke/acp-server\"]\nprotocol = \"acp\"\n");

    let agents = resolve_agents_configuration(Some(&cfg), None).expect("resolve");

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, AcpAgentKind::Custom);
    assert_eq!(
        agents[0].executable_path,
        PathBuf::from("/opt/bespoke/acp-server")
    );
}

#[test]
fn test_comments_and_blank_lines_are_ignored_in_agents_config() {
    let fx = Fixture::new();
    let cfg = fx.write_config(
        r#"
# leading comment

[[agents]] # trailing comment
name = "agy" # inline comment
executable = "/opt/agy"

"#,
    );

    let agents = resolve_agents_configuration(Some(&cfg), None).expect("resolve");

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, AcpAgentKind::Agy);
    assert_eq!(agents[0].executable_path, PathBuf::from("/opt/agy"));
}

#[test]
fn test_malformed_toml_returns_io_error() {
    let fx = Fixture::new();
    let cfg = fx.write_config("[[agents]\nname = \"agy\"\ncommand = [\"unterminated\n");

    let result = resolve_agents_configuration(Some(&cfg), None);

    match result {
        Err(DiscoveryError::IoError(msg)) => assert!(!msg.is_empty()),
        other => panic!("expected IoError, got {other:?}"),
    }
}

#[test]
fn test_unreadable_config_returns_io_error() {
    let fx = Fixture::new();
    // A directory exists at the path but cannot be read as a file.
    let dir_as_config = fx.dir.path().join("config-dir");
    fs::create_dir_all(&dir_as_config).expect("mkdir");

    let result = resolve_agents_configuration(Some(&dir_as_config), None);

    match result {
        Err(DiscoveryError::IoError(msg)) => assert!(!msg.is_empty()),
        other => panic!("expected IoError, got {other:?}"),
    }
}
