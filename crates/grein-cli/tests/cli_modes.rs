use std::fs::{self, File};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use grein_cli::discovery::{
    AcpAgent, AcpAgentKind, discover_acp_agents, resolve_agents_configuration,
};

#[test]
fn test_first_run_discovery_inspects_path_for_acp_agents() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("failed to create bin dir");

    let agy_path = bin_dir.join("agy");
    let claude_path = bin_dir.join("claude");

    File::create(&agy_path).expect("failed to create agy binary mock");
    File::create(&claude_path).expect("failed to create claude binary mock");

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&agy_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&agy_path, perms).unwrap();

        let mut perms = fs::metadata(&claude_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude_path, perms).unwrap();
    }

    let search_path = bin_dir.to_str().expect("valid utf-8 path");
    let discovered = discover_acp_agents(Some(search_path));

    assert_eq!(discovered.len(), 2);
    assert!(
        discovered
            .iter()
            .any(|a| a.kind == AcpAgentKind::Agy && a.executable_path == agy_path)
    );
    assert!(
        discovered
            .iter()
            .any(|a| a.kind == AcpAgentKind::Claude && a.executable_path == claude_path)
    );
}

#[test]
fn test_first_run_discovery_returns_empty_when_no_agents_found() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("failed to create bin dir");

    let other_bin = bin_dir.join("unrelated-tool");
    File::create(&other_bin).expect("failed to create unrelated tool");
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&other_bin).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&other_bin, perms).unwrap();
    }

    let search_path = bin_dir.to_str().expect("valid utf-8 path");
    let discovered = discover_acp_agents(Some(search_path));

    assert!(discovered.is_empty());
}

#[test]
fn test_first_run_discovery_ignores_non_executable_files() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("failed to create bin dir");

    let agy_path = bin_dir.join("agy");
    File::create(&agy_path).expect("failed to create agy mock");
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&agy_path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&agy_path, perms).unwrap();
    }

    let search_path = bin_dir.to_str().expect("valid utf-8 path");
    let discovered = discover_acp_agents(Some(search_path));

    #[cfg(unix)]
    assert!(discovered.is_empty());
}

#[test]
fn test_resolve_agents_configuration_triggers_discovery_when_config_absent() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let non_existent_config = temp_dir.path().join("non_existent_config.toml");
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("failed to create bin dir");

    let agy_path = bin_dir.join("agy");
    File::create(&agy_path).expect("failed to create agy mock");
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&agy_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&agy_path, perms).unwrap();
    }

    let search_path = bin_dir.to_str().expect("valid utf-8 path");
    let result = resolve_agents_configuration(Some(&non_existent_config), Some(search_path));

    assert!(result.is_ok());
    let agents = result.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, AcpAgentKind::Agy);
    assert_eq!(agents[0].executable_path, agy_path);
}
