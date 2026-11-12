use grein_acp::permissions::{
    EvaluationResult, PermissionGate, PermissionRequest, PermissionScope,
};

#[test]
fn test_read_tools_are_automatically_allowed() {
    let mut gate = PermissionGate::new();
    let result = gate.evaluate("session-1", "thread-1", "read_file", "crates/test.rs");

    assert_eq!(result, EvaluationResult::Allowed);
}

#[test]
fn test_intercepted_tools_require_permission_by_default() {
    let mut gate = PermissionGate::new();

    let intercepted_tools = ["run_command", "write_file", "edit_file"];
    for tool in intercepted_tools {
        let result = gate.evaluate("session-1", "thread-1", tool, "cargo test");
        match result {
            EvaluationResult::PromptRequired(req) => {
                assert_eq!(req.session_id, "session-1");
                assert_eq!(req.thread_id, "thread-1");
                assert_eq!(req.tool_name, tool);
                assert_eq!(req.argument, "cargo test");
            }
            _ => panic!("Expected PromptRequired for intercepted tool {tool}"),
        }
    }
}

#[test]
fn test_allow_once_scope_permits_single_invocation_only() {
    let mut gate = PermissionGate::new();

    let prompt = gate.evaluate("session-1", "thread-1", "run_command", "cargo test");
    assert!(matches!(prompt, EvaluationResult::PromptRequired(_)));

    let allowed = gate.resolve_prompt(
        "session-1",
        "thread-1",
        "run_command",
        "cargo test",
        PermissionScope::AllowOnce,
    );
    assert!(allowed);

    // Second call must prompt again because scope was AllowOnce
    let next = gate.evaluate("session-1", "thread-1", "run_command", "cargo test");
    assert!(matches!(next, EvaluationResult::PromptRequired(_)));
}

#[test]
fn test_deny_scope_rejects_invocation() {
    let mut gate = PermissionGate::new();

    let allowed = gate.resolve_prompt(
        "session-1",
        "thread-1",
        "write_file",
        "secrets.env",
        PermissionScope::Deny,
    );
    assert!(!allowed);

    let next = gate.evaluate("session-1", "thread-1", "write_file", "secrets.env");
    assert_eq!(next, EvaluationResult::Denied);
}

#[test]
fn test_always_thread_scope_persists_within_thread_only() {
    let mut gate = PermissionGate::new();

    let allowed = gate.resolve_prompt(
        "session-1",
        "thread-1",
        "edit_file",
        "src/main.rs",
        PermissionScope::AlwaysThread,
    );
    assert!(allowed);

    // Same thread: automatically allowed
    let same_thread = gate.evaluate("session-1", "thread-1", "edit_file", "src/main.rs");
    assert_eq!(same_thread, EvaluationResult::Allowed);

    // Different thread in same session: must prompt
    let other_thread = gate.evaluate("session-1", "thread-2", "edit_file", "src/main.rs");
    assert!(matches!(other_thread, EvaluationResult::PromptRequired(_)));
}

#[test]
fn test_always_session_scope_persists_across_threads_in_session() {
    let mut gate = PermissionGate::new();

    let allowed = gate.resolve_prompt(
        "session-1",
        "thread-1",
        "run_command",
        "git status",
        PermissionScope::AlwaysSession,
    );
    assert!(allowed);

    // Different thread in same session: automatically allowed
    let other_thread = gate.evaluate("session-1", "thread-2", "run_command", "git status");
    assert_eq!(other_thread, EvaluationResult::Allowed);

    // Different session: must prompt
    let other_session = gate.evaluate("session-2", "thread-1", "run_command", "git status");
    assert!(matches!(other_session, EvaluationResult::PromptRequired(_)));
}
