use grein_acp::permissions::{PermissionDecision, PermissionGate, ToolInvocation};

#[test]
fn test_safe_read_tools_bypass_interception() {
    let mut gate = PermissionGate::new();
    let invocation = ToolInvocation::new(
        "read_file",
        "session-1",
        "thread-1",
        serde_json::json!({"Path": "/src/main.rs"}),
    );

    assert!(!gate.is_intercepted(&invocation));
    let result = gate.evaluate(&invocation);
    assert_eq!(result, Ok(true));
}

#[test]
fn test_intercepted_tools_require_permission() {
    let gate = PermissionGate::new();
    let intercepted_tools = ["run_command", "write_file", "edit_file"];

    for tool_name in intercepted_tools {
        let invocation =
            ToolInvocation::new(tool_name, "session-1", "thread-1", serde_json::json!({}));
        assert!(
            gate.is_intercepted(&invocation),
            "Expected {tool_name} to be intercepted"
        );
    }
}

#[test]
fn test_deny_decision_rejects_invocation() {
    let mut gate = PermissionGate::new();
    let invocation = ToolInvocation::new(
        "run_command",
        "session-1",
        "thread-1",
        serde_json::json!({"CommandLine": "rm -rf /tmp/data"}),
    );

    assert!(gate.is_intercepted(&invocation));
    gate.record_decision(&invocation, PermissionDecision::Deny);

    let result = gate.evaluate(&invocation);
    assert_eq!(result, Ok(false));
}

#[test]
fn test_allow_once_scope_authorizes_single_invocation_only() {
    let mut gate = PermissionGate::new();
    let invocation = ToolInvocation::new(
        "write_file",
        "session-1",
        "thread-1",
        serde_json::json!({"Path": "/tmp/test.txt", "Content": "hello"}),
    );

    gate.record_decision(&invocation, PermissionDecision::AllowOnce);

    // First evaluation succeeds
    assert_eq!(gate.evaluate(&invocation), Ok(true));

    // Subsequent invocation with same parameters is no longer cached
    assert!(gate.needs_prompt(&invocation));
}

#[test]
fn test_always_thread_scope_remembers_permission_for_thread_only() {
    let mut gate = PermissionGate::new();
    let invocation_t1 = ToolInvocation::new(
        "edit_file",
        "session-1",
        "thread-1",
        serde_json::json!({"Path": "/tmp/file.rs"}),
    );
    let invocation_t2 = ToolInvocation::new(
        "edit_file",
        "session-1",
        "thread-2",
        serde_json::json!({"Path": "/tmp/file.rs"}),
    );

    gate.record_decision(&invocation_t1, PermissionDecision::AlwaysThread);

    // Thread 1 is approved automatically without prompt
    assert!(!gate.needs_prompt(&invocation_t1));
    assert_eq!(gate.evaluate(&invocation_t1), Ok(true));

    // Thread 2 still requires prompt
    assert!(gate.needs_prompt(&invocation_t2));
}

#[test]
fn test_always_session_scope_remembers_permission_across_all_threads() {
    let mut gate = PermissionGate::new();
    let invocation_s1_t1 = ToolInvocation::new(
        "run_command",
        "session-1",
        "thread-1",
        serde_json::json!({"CommandLine": "cargo test"}),
    );
    let invocation_s1_t2 = ToolInvocation::new(
        "run_command",
        "session-1",
        "thread-2",
        serde_json::json!({"CommandLine": "cargo check"}),
    );
    let invocation_s2_t1 = ToolInvocation::new(
        "run_command",
        "session-2",
        "thread-1",
        serde_json::json!({"CommandLine": "cargo test"}),
    );

    gate.record_decision(&invocation_s1_t1, PermissionDecision::AlwaysSession);

    // Both threads within session-1 are authorized
    assert!(!gate.needs_prompt(&invocation_s1_t1));
    assert_eq!(gate.evaluate(&invocation_s1_t1), Ok(true));

    assert!(!gate.needs_prompt(&invocation_s1_t2));
    assert_eq!(gate.evaluate(&invocation_s1_t2), Ok(true));

    // Invocations belonging to a different session still require prompt
    assert!(gate.needs_prompt(&invocation_s2_t1));
}

#[test]
fn test_permission_scopes_are_isolated_by_tool_name() {
    let mut gate = PermissionGate::new();
    let run_cmd = ToolInvocation::new(
        "run_command",
        "session-1",
        "thread-1",
        serde_json::json!({"CommandLine": "ls"}),
    );
    let write_file = ToolInvocation::new(
        "write_file",
        "session-1",
        "thread-1",
        serde_json::json!({"Path": "/tmp/out.txt"}),
    );

    gate.record_decision(&run_cmd, PermissionDecision::AlwaysSession);

    assert!(!gate.needs_prompt(&run_cmd));
    assert_eq!(gate.evaluate(&run_cmd), Ok(true));

    // write_file must not inherit run_command's session grant
    assert!(gate.needs_prompt(&write_file));
}
