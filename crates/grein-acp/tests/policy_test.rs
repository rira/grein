use grein_acp::policy::{PolicyDecision, PolicyError, SecurityPolicy};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_parse_policy_from_toml() {
    let toml_content = r#"
        allowed_tools = ["read_file", "ask_question"]
        allowed_command_prefixes = ["cargo check", "git status", "cargo test"]
        denied_commands = ["rm -rf /", "shutdown"]
    "#;

    let policy = SecurityPolicy::from_toml(toml_content)
        .expect("Failed to parse valid security policy TOML");

    assert!(policy.allowed_tools().contains(&"read_file".to_string()));
    assert!(policy.allowed_tools().contains(&"ask_question".to_string()));
    assert!(policy
        .allowed_command_prefixes()
        .contains(&"cargo check".to_string()));
    assert!(policy
        .denied_commands()
        .contains(&"rm -rf /".to_string()));
}

#[test]
fn test_policy_evaluate_tool_and_command_allowlist() {
    let toml_content = r#"
        allowed_tools = ["read_file"]
        allowed_command_prefixes = ["cargo test", "git "]
        denied_commands = ["git push --force"]
    "#;

    let policy = SecurityPolicy::from_toml(toml_content).unwrap();

    // Tool allowed
    assert_eq!(
        policy.evaluate_tool("read_file"),
        PolicyDecision::Allowed
    );

    // Tool not explicitly allowed
    assert_eq!(
        policy.evaluate_tool("write_file"),
        PolicyDecision::Prompt
    );

    // Command allowed by prefix
    assert_eq!(policy.evaluate_command("cargo test -p grein-acp"), PolicyDecision::Allowed);
    assert_eq!(policy.evaluate_command("git status"), PolicyDecision::Allowed);

    // Command denied explicitly
    assert_eq!(
        policy.evaluate_command("git push --force"),
        PolicyDecision::Denied
    );

    // Unlisted command prompts
    assert_eq!(
        policy.evaluate_command("bash -c 'evil'"),
        PolicyDecision::Prompt
    );
}

#[test]
fn test_load_and_merge_global_and_local_policies() {
    let dir = tempdir().unwrap();
    let global_path = dir.path().join("global_policy.toml");
    let local_path = dir.path().join(".grein-policy.toml");

    let global_toml = r#"
        allowed_tools = ["read_file"]
        allowed_command_prefixes = ["cargo test"]
    "#;

    let local_toml = r#"
        allowed_tools = ["write_file"]
        allowed_command_prefixes = ["cargo check", "pnpm test"]
    "#;

    fs::write(&global_path, global_toml).unwrap();
    fs::write(&local_path, local_toml).unwrap();

    let merged_policy = SecurityPolicy::load_merged(Some(&global_path), Some(&local_path))
        .expect("Failed to load merged policies");

    // Both tools from global and local should be present
    assert_eq!(merged_policy.evaluate_tool("read_file"), PolicyDecision::Allowed);
    assert_eq!(merged_policy.evaluate_tool("write_file"), PolicyDecision::Allowed);

    // Both command prefixes should be recognized
    assert_eq!(
        merged_policy.evaluate_command("cargo test"),
        PolicyDecision::Allowed
    );
    assert_eq!(
        merged_policy.evaluate_command("pnpm test"),
        PolicyDecision::Allowed
    );
}

#[test]
fn test_invalid_policy_toml_returns_error() {
    let invalid_toml = "[allowed_tools = invalid_syntax";
    let result = SecurityPolicy::from_toml(invalid_toml);
    assert!(result.is_err());
    match result.unwrap_err() {
        PolicyError::ParseError(_) => {},
        other => panic!("Expected PolicyError::ParseError, got: {:?}", other),
    }
}