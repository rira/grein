use grein_acp::policy::{PolicyError, SecurityPolicy};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_parse_policy_from_toml() {
    let toml_content = r#"
        allowed_tools = ["read_file", "manage_task"]
        allowed_commands = ["cargo test", "git status", "cargo check"]
    "#;

    let policy = SecurityPolicy::from_toml_str(toml_content)
        .expect("Policy should successfully parse from valid TOML");

    assert!(policy.is_tool_allowed("read_file"));
    assert!(policy.is_tool_allowed("manage_task"));
    assert!(!policy.is_tool_allowed("run_command"));
    assert!(!policy.is_tool_allowed("write_file"));

    assert!(policy.is_command_allowed("cargo test"));
    assert!(policy.is_command_allowed("cargo test -p grein-acp --test policy_gate"));
    assert!(policy.is_command_allowed("git status --short"));
    assert!(!policy.is_command_allowed("cargo run"));
    assert!(!policy.is_command_allowed("rm -rf /"));
}

#[test]
fn test_policy_command_prefix_allowlist_matching() {
    let toml_content = r#"
        allowed_commands = ["git log", "cargo test --test"]
    "#;

    let policy =
        SecurityPolicy::from_toml_str(toml_content).expect("Policy should parse successfully");

    assert!(policy.is_command_allowed("git log -n 5"));
    assert!(policy.is_command_allowed("cargo test --test policy_test"));
    assert!(!policy.is_command_allowed("git push origin main"));
    assert!(!policy.is_command_allowed("cargo build"));
    assert!(!policy.is_command_allowed(""));
}

#[test]
fn test_policy_merging_global_and_local() {
    let global_toml = r#"
        allowed_tools = ["read_file"]
        allowed_commands = ["cargo test"]
    "#;

    let local_toml = r#"
        allowed_tools = ["manage_task"]
        allowed_commands = ["pnpm test", "git diff"]
    "#;

    let global_policy =
        SecurityPolicy::from_toml_str(global_toml).expect("Global policy should parse");
    let local_policy =
        SecurityPolicy::from_toml_str(local_toml).expect("Local policy should parse");

    let merged_policy = global_policy.merge(&local_policy);

    // Both tools from global and local should be authorized
    assert!(merged_policy.is_tool_allowed("read_file"));
    assert!(merged_policy.is_tool_allowed("manage_task"));
    assert!(!merged_policy.is_tool_allowed("run_command"));

    // Commands from both global and local should be authorized
    assert!(merged_policy.is_command_allowed("cargo test --all"));
    assert!(merged_policy.is_command_allowed("pnpm test"));
    assert!(merged_policy.is_command_allowed("git diff HEAD~1"));
    assert!(!merged_policy.is_command_allowed("pnpm build"));
}

#[test]
fn test_invalid_toml_returns_policy_error() {
    let invalid_toml = "not a valid toml = = {";
    let result = SecurityPolicy::from_toml_str(invalid_toml);

    match result {
        Err(PolicyError::ParseError(_)) => {}
        Err(other) => panic!("Expected PolicyError::ParseError, got: {:?}", other),
        Ok(_) => panic!("Expected parse failure on invalid TOML"),
    }
}

#[test]
fn test_load_global_and_local_policy_from_disk() {
    let temp_dir = tempdir().expect("Should create temporary directory");
    let global_path = temp_dir.path().join("global_policy.toml");
    let local_path = temp_dir.path().join(".grein-policy.toml");

    fs::write(
        &global_path,
        r#"
            allowed_tools = ["read_file"]
            allowed_commands = ["cargo test"]
        "#,
    )
    .expect("Failed to write mock global policy");

    fs::write(
        &local_path,
        r#"
            allowed_tools = ["ask_question"]
            allowed_commands = ["git status"]
        "#,
    )
    .expect("Failed to write mock local policy");

    let policy = SecurityPolicy::load_from_paths(Some(&global_path), Some(&local_path))
        .expect("Policy should load successfully from files");

    assert!(policy.is_tool_allowed("read_file"));
    assert!(policy.is_tool_allowed("ask_question"));
    assert!(!policy.is_tool_allowed("write_file"));

    assert!(policy.is_command_allowed("cargo test --lib"));
    assert!(policy.is_command_allowed("git status"));
    assert!(!policy.is_command_allowed("git commit"));
}

#[test]
fn test_load_from_disk_handles_missing_files_gracefully() {
    let temp_dir = tempdir().expect("Should create temporary directory");
    let nonexistent_global = temp_dir.path().join("nonexistent_global.toml");
    let nonexistent_local = temp_dir.path().join("nonexistent_local.toml");

    // When configuration files are absent, empty/default policy is returned without error
    let policy =
        SecurityPolicy::load_from_paths(Some(&nonexistent_global), Some(&nonexistent_local))
            .expect("Missing policy files should resolve to default empty policy");

    assert!(!policy.is_tool_allowed("read_file"));
    assert!(!policy.is_command_allowed("cargo test"));
}
