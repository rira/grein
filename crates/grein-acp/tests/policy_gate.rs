use grein_acp::permissions::{PermissionDecision, ToolInvocationRequest};
use grein_acp::policy::{PolicyDecision, PolicyGate, SecurityPolicy};
use grein_acp::session::{SessionId, SessionManager, ThreadId};

#[test]
fn test_auto_approves_preauthorized_read_tools() {
    let policy = SecurityPolicy::builder().allow_read_tools(true).build();
    let gate = PolicyGate::new(policy);

    let request = ToolInvocationRequest::new(
        "read_file",
        serde_json::json!({
            "Path": "crates/grein-acp/src/lib.rs"
        }),
    );

    let decision = gate.evaluate(&request);
    assert_eq!(decision, PolicyDecision::Allow);
}

#[test]
fn test_approves_allowlisted_command_prefixes() {
    let policy = SecurityPolicy::builder()
        .allowed_command_prefixes(vec!["cargo test".to_string(), "git status".to_string()])
        .build();
    let gate = PolicyGate::new(policy);

    let allowed_request = ToolInvocationRequest::new(
        "run_command",
        serde_json::json!({
            "CommandLine": "cargo test -p grein-acp",
            "toolSummary": "run tests"
        }),
    );

    assert_eq!(gate.evaluate(&allowed_request), PolicyDecision::Allow);

    let unlisted_request = ToolInvocationRequest::new(
        "run_command",
        serde_json::json!({
            "CommandLine": "rm -rf /tmp/test",
            "toolSummary": "delete files"
        }),
    );

    assert_eq!(gate.evaluate(&unlisted_request), PolicyDecision::PromptUser);
}

#[test]
fn test_emits_interactive_permission_decisions_for_unlisted_tools() {
    let policy = SecurityPolicy::default();
    let mut gate = PolicyGate::new(policy);

    let thread_id = ThreadId::from("branch-dag-1");
    let session_id = SessionId::from("session-alpha");

    let write_request = ToolInvocationRequest::new(
        "write_file",
        serde_json::json!({
            "Path": "crates/grein-acp/src/config.rs",
            "Content": "// updated"
        }),
    );

    assert_eq!(gate.evaluate(&write_request), PolicyDecision::PromptUser);

    gate.record_decision(
        &session_id,
        &thread_id,
        &write_request,
        PermissionDecision::AlwaysSession,
    );

    let subsequent_decision = gate.evaluate_scoped(&session_id, &thread_id, &write_request);
    assert_eq!(subsequent_decision, PolicyDecision::Allow);
}

fn run_command(command_line: &str) -> ToolInvocationRequest {
    ToolInvocationRequest::new(
        "run_command",
        serde_json::json!({ "CommandLine": command_line }),
    )
}

#[test]
fn test_parses_section_header_toml_keys() {
    let toml = r#"
        [permissions]
        auto_allow_read_tools = true
        allow_commands = ["cargo test", "git status*"]
        deny_commands = ["git push*"]

        [worktree]
        auto_allow_writes_in_worktrees = true
    "#;

    let policy = SecurityPolicy::from_toml_configs(Some(toml), None).expect("parsed policy");
    let gate = PolicyGate::new(policy);

    let read_req = ToolInvocationRequest::new("read_file", serde_json::json!({"Path": "a.md"}));
    assert_eq!(gate.evaluate(&read_req), PolicyDecision::Allow);
    assert_eq!(
        gate.evaluate(&run_command("cargo test --lib")),
        PolicyDecision::Allow
    );
    assert_eq!(
        gate.evaluate(&run_command("git status -s")),
        PolicyDecision::Allow
    );
    assert_eq!(
        gate.evaluate(&run_command("git push origin main")),
        PolicyDecision::Deny
    );
}

#[test]
fn test_parses_policy_section_header() {
    let toml = r#"
        [policy]
        allow_commands = ["cargo check"]
    "#;

    let policy = SecurityPolicy::from_toml_configs(Some(toml), None).expect("parsed policy");
    let gate = PolicyGate::new(policy);
    assert_eq!(
        gate.evaluate(&run_command("cargo check -p grein-acp")),
        PolicyDecision::Allow
    );
}

#[test]
fn test_deny_commands_take_precedence_over_allow() {
    let policy = SecurityPolicy::from_toml_configs(
        Some(
            r#"
            [permissions]
            allow_commands = ["git*"]
            deny_commands = ["git push*", "git reset --hard"]
        "#,
        ),
        None,
    )
    .expect("parsed policy");
    let gate = PolicyGate::new(policy);

    assert_eq!(
        gate.evaluate(&run_command("git push --force")),
        PolicyDecision::Deny
    );
    assert_eq!(
        gate.evaluate(&run_command("git reset --hard")),
        PolicyDecision::Deny
    );
    assert_eq!(
        gate.evaluate(&run_command("git log")),
        PolicyDecision::Allow
    );
}

#[test]
fn test_shell_metacharacters_are_never_auto_allowed() {
    let policy = SecurityPolicy::from_toml_configs(
        Some(
            r#"
            [permissions]
            allow_commands = ["cargo test"]
            deny_commands = ["rm*"]
        "#,
        ),
        None,
    )
    .expect("parsed policy");
    let gate = PolicyGate::new(policy);

    let injections = [
        "cargo test && rm -rf /",
        "cargo test; ls",
        "cargo test || ls",
        "cargo test | cat",
        "cargo test `id`",
        "cargo test $(id)",
        "cargo test > out.txt",
        "cargo test < in.txt",
        "cargo test & ls",
        "cargo test\nls",
    ];
    for injected in injections {
        assert_eq!(
            gate.evaluate(&run_command(injected)),
            PolicyDecision::PromptUser,
            "command must not be auto-allowed: {injected:?}"
        );
    }

    assert_eq!(
        gate.evaluate(&run_command("rm -rf / && echo hi")),
        PolicyDecision::Deny
    );
}

#[test]
fn test_legacy_prefixes_also_guarded_and_word_boundary() {
    let policy = SecurityPolicy::builder()
        .allowed_command_prefixes(vec!["cargo test".to_string()])
        .build();
    let gate = PolicyGate::new(policy);

    assert_eq!(
        gate.evaluate(&run_command("cargo test && rm -rf /")),
        PolicyDecision::PromptUser
    );
    assert_eq!(
        gate.evaluate(&run_command("cargo test --lib")),
        PolicyDecision::Allow
    );
    assert_eq!(
        gate.evaluate(&run_command("cargo test_evil")),
        PolicyDecision::PromptUser
    );
}

#[test]
fn test_worktree_auto_allow_writes_requires_isolated_worktree() {
    let policy = SecurityPolicy::from_toml_configs(
        Some(
            r#"
            [worktree]
            auto_allow_writes_in_worktrees = true
        "#,
        ),
        None,
    )
    .expect("parsed policy");
    let gate = PolicyGate::new(policy);

    let plain = ToolInvocationRequest::new("write_file", serde_json::json!({"Path": "a.rs"}));
    assert!(!plain.is_in_isolated_worktree());
    assert!(plain.is_write_tool());
    assert_eq!(gate.evaluate(&plain), PolicyDecision::PromptUser);

    let isolated = plain.with_isolated_worktree(true);
    assert!(isolated.is_in_isolated_worktree());
    assert_eq!(gate.evaluate(&isolated), PolicyDecision::Allow);
}

#[test]
fn test_allow_once_is_single_use() {
    let mut gate = PolicyGate::new(SecurityPolicy::default());
    let thread_id = ThreadId::from("branch-once");
    let session_id = SessionId::from("session-once");
    let request = ToolInvocationRequest::new("write_file", serde_json::json!({"Path": "b.rs"}));

    gate.record_decision(
        &session_id,
        &thread_id,
        &request,
        PermissionDecision::AllowOnce,
    );

    assert_eq!(
        gate.evaluate_scoped(&session_id, &thread_id, &request),
        PolicyDecision::Allow
    );
    assert_eq!(
        gate.evaluate_scoped(&session_id, &thread_id, &request),
        PolicyDecision::PromptUser
    );
}

#[test]
fn test_session_multiplexing_maps_dag_branch_threads_to_distinct_sessions() {
    let mut session_manager = SessionManager::new(true);

    let thread_a = ThreadId::from("dag-feature-branch-a");
    let thread_b = ThreadId::from("dag-feature-branch-b");

    let session_a = session_manager
        .get_or_create_session(&thread_a)
        .expect("session A created");
    let session_b = session_manager
        .get_or_create_session(&thread_b)
        .expect("session B created");

    assert_ne!(session_a.id(), session_b.id());
    assert_eq!(session_manager.active_session_count(), 2);

    let resolved_a = session_manager
        .get_session(&thread_a)
        .expect("session A exists");
    assert_eq!(resolved_a.id(), session_a.id());
}

#[test]
fn test_loads_declarative_policy_hierarchy_from_toml() {
    let global_toml = r#"
        allow_read_tools = true
        allowed_command_prefixes = ["cargo check", "git diff"]
    "#;

    let local_toml = r#"
        allowed_command_prefixes = ["cargo test"]
    "#;

    let policy = SecurityPolicy::from_toml_configs(Some(global_toml), Some(local_toml))
        .expect("parsed security policies");

    let gate = PolicyGate::new(policy);

    let read_req =
        ToolInvocationRequest::new("read_file", serde_json::json!({"Path": "README.md"}));
    assert_eq!(gate.evaluate(&read_req), PolicyDecision::Allow);

    let test_req = ToolInvocationRequest::new(
        "run_command",
        serde_json::json!({"CommandLine": "cargo test"}),
    );
    assert_eq!(gate.evaluate(&test_req), PolicyDecision::Allow);

    let diff_req = ToolInvocationRequest::new(
        "run_command",
        serde_json::json!({"CommandLine": "git diff HEAD~1"}),
    );
    assert_eq!(gate.evaluate(&diff_req), PolicyDecision::Allow);
}
