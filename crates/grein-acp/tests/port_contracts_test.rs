use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use grein_acp::{
    AcpSpawnConfig, AgentDriver, AgentDriverError, AgentDriverFactory, DriverEvent,
    PermissionDecision, PermissionPolicyPort, PolicyDecision, PolicyGate, SecurityPolicy,
    SessionId, StdioAgentDriverFactory, StdioRunnerError, ThreadId, ToolInvocationRequest,
};
use serde_json::json;
use tokio::time::timeout;

#[tokio::test]
async fn agent_driver_is_running_reflects_live_and_exited_process_via_trait_object() {
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());

    let cfg = AcpSpawnConfig::new("sh", ["-c", "sleep 5"]);
    let driver: Arc<dyn AgentDriver> = factory.spawn(&cfg).await.expect("spawn sleeping driver");
    assert!(driver.is_running());
    driver.shutdown().await.expect("shutdown");
    assert!(!driver.is_running());

    let short_cfg = AcpSpawnConfig::new("true", Vec::<String>::new());
    let short_driver: Arc<dyn AgentDriver> =
        factory.spawn(&short_cfg).await.expect("spawn true driver");
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!short_driver.is_running());
    short_driver.shutdown().await.expect("shutdown exited");
}

#[tokio::test]
async fn stdio_agent_driver_factory_spawns_and_handshakes_via_trait_object() {
    let fake_acp_script = r#"
while IFS= read -r line; do
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"fake-acp","version":"1.0"},"authMethods":[{"id":"token"}]}}\n' "$id"
            ;;
        *'"method":"authenticate"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-factory-1"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-factory-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello-from-factory"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
        *'"method":"session/close"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
            break
            ;;
    esac
done
"#;

    let cwd = std::env::temp_dir();
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());
    let cfg = AcpSpawnConfig::new("sh", ["-c", fake_acp_script])
        .with_cwd(&cwd)
        .with_auth_method("token")
        .with_env([("GREIN_FACTORY_TEST".into(), "1".into())]);

    let driver = factory
        .spawn_initialized(&cfg)
        .await
        .expect("spawn_initialized succeeds");
    assert!(driver.is_running());

    let session = driver
        .new_session(&cwd, None)
        .await
        .expect("new_session");
    assert_eq!(session.session_id, "sess-factory-1");

    let mut rx = driver
        .send_prompt(&session.session_id, "hi")
        .await
        .expect("send_prompt");
    let mut events = Vec::new();
    while let Some(ev) = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event timeout")
    {
        let done = matches!(ev, DriverEvent::PromptCompleted { .. });
        events.push(ev);
        if done {
            break;
        }
    }
    assert_eq!(
        events,
        vec![
            DriverEvent::StreamChunk {
                session_id: "sess-factory-1".into(),
                text: "hello-from-factory".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: "sess-factory-1".into(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    driver
        .close_session(&session.session_id)
        .await
        .expect("close_session");
    driver.shutdown().await.expect("shutdown");
    assert!(!driver.is_running());
}

#[tokio::test]
async fn stdio_agent_driver_factory_propagates_spawn_and_auth_errors() {
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());

    let missing_cfg = AcpSpawnConfig::new("/nonexistent/grein-acp-binary-xyz", Vec::<String>::new());
    let spawn_err = match factory.spawn(&missing_cfg).await {
        Ok(_) => panic!("missing binary must fail"),
        Err(err) => err,
    };
    assert!(matches!(spawn_err, AgentDriverError::SpawnFailed(_)));

    let fail_auth_script = r#"
while IFS= read -r line; do
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"authMethods":[{"id":"token"}]}}\n' "$id"
            ;;
        *'"method":"authenticate"'*)
            printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32001,"message":"bad token"}}\n' "$id"
            sleep 5
            ;;
    esac
done
"#;
    let auth_cfg = AcpSpawnConfig::new("sh", ["-c", fail_auth_script]).with_auth_method("token");
    let auth_err = match factory.spawn_initialized(&auth_cfg).await {
        Ok(_) => panic!("auth error must fail spawn_initialized"),
        Err(err) => err,
    };
    assert!(
        matches!(auth_err, AgentDriverError::Transport(ref msg) if msg.contains("bad token")),
        "unexpected error: {auth_err:?}"
    );
}

#[test]
fn agent_driver_error_roundtrips_with_stdio_runner_error() {
    let io_err: AgentDriverError = std::io::Error::other("io boom").into();
    assert!(matches!(io_err, AgentDriverError::SpawnFailed(ref m) if m.contains("io boom")));

    let cases = [
        (
            StdioRunnerError::SpawnFailed(std::io::Error::other("no bin")),
            AgentDriverError::SpawnFailed("no bin".into()),
        ),
        (
            StdioRunnerError::ProcessTerminated("eof".into()),
            AgentDriverError::Terminated("eof".into()),
        ),
        (StdioRunnerError::Timeout, AgentDriverError::Timeout),
        (
            StdioRunnerError::CommunicationError("broken".into()),
            AgentDriverError::Transport("broken".into()),
        ),
    ];

    for (stdio_err, expected_driver_err) in cases {
        let converted: AgentDriverError = stdio_err.into();
        assert_eq!(converted, expected_driver_err);
        let back: StdioRunnerError = converted.into();
        assert!(!back.to_string().is_empty());
    }

    let proto = AgentDriverError::Protocol("bad payload".into());
    let proto_back: StdioRunnerError = proto.into();
    assert!(matches!(proto_back, StdioRunnerError::CommunicationError(m) if m == "bad payload"));

    let unsup = AgentDriverError::Unsupported("not supported".into());
    assert!(unsup.to_string().contains("unsupported"));
    let unsup_back: StdioRunnerError = unsup.into();
    assert!(matches!(unsup_back, StdioRunnerError::CommunicationError(m) if m == "not supported"));
}

#[test]
fn permission_policy_port_trait_object_evaluates_and_records_scoped_decisions() {
    let policy = SecurityPolicy::builder()
        .allow_read_tools(true)
        .allow_commands(vec!["cargo test".into()])
        .deny_commands(vec!["rm -rf*".into()])
        .auto_allow_writes_in_worktrees(true)
        .build();
    let gate: Arc<dyn PermissionPolicyPort> = Arc::new(PolicyGate::new(policy));

    let read_req = ToolInvocationRequest::new("read_file", json!({"Path": "src/lib.rs"}));
    assert_eq!(gate.evaluate(&read_req), PolicyDecision::Allow);

    let deny_req = ToolInvocationRequest::new("run_command", json!({"CommandLine": "rm -rf /tmp"}));
    assert_eq!(gate.evaluate(&deny_req), PolicyDecision::Deny);

    let wt_write = ToolInvocationRequest::new("write_file", json!({"Path": "src/a.rs"}))
        .with_isolated_worktree(true);
    assert_eq!(gate.evaluate(&wt_write), PolicyDecision::Allow);

    let s1 = SessionId::new("sess-1");
    let t1 = ThreadId::new("thread-1");
    let t2 = ThreadId::new("thread-2");
    let write_req = ToolInvocationRequest::new("write_file", json!({"Path": "src/main.rs"}));

    assert_eq!(
        gate.evaluate_scoped(&s1, &t1, &write_req),
        PolicyDecision::PromptUser
    );

    gate.record_decision(&s1, &t1, &write_req, PermissionDecision::AllowOnce);
    assert_eq!(
        gate.evaluate_scoped(&s1, &t1, &write_req),
        PolicyDecision::Allow
    );
    assert_eq!(
        gate.evaluate_scoped(&s1, &t1, &write_req),
        PolicyDecision::PromptUser
    );

    gate.record_decision(&s1, &t1, &write_req, PermissionDecision::AlwaysThread);
    assert_eq!(
        gate.evaluate_scoped(&s1, &t1, &write_req),
        PolicyDecision::Allow
    );
    assert_eq!(
        gate.evaluate_scoped(&s1, &t2, &write_req),
        PolicyDecision::PromptUser
    );

    gate.record_decision(&s1, &t2, &write_req, PermissionDecision::AlwaysSession);
    assert_eq!(
        gate.evaluate_scoped(&s1, &t2, &write_req),
        PolicyDecision::Allow
    );
}

#[allow(dead_code)]
fn assert_driver_object_safety(_driver: Arc<dyn AgentDriver>, _factory: Arc<dyn AgentDriverFactory>) {
    let _cwd = Path::new(".");
}
