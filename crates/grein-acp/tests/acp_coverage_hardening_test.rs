use grein_acp::framing::{
    AcpError, FramingError, JsonRpcError, JsonRpcId, JsonRpcMessage, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, MessageDecoder, MessageEncoder, StdioFrameCodec,
};
use grein_acp::permissions::{PermissionDecision, ToolInvocationRequest};
use grein_acp::policy::{PolicyDecision, PolicyError, PolicyGate, SecurityPolicy};
use grein_acp::runner::{
    JsonRpcRequest as RunnerRequest, JsonRpcResponse as RunnerResponse, RunnerError,
    SubprocessConfig, SubprocessRunner,
};
use grein_acp::session::{Session, SessionError, SessionId, SessionManager, ThreadId};
use grein_acp::stdio::{
    AcpStdioDriver, DriverEvent, JsonRpcMessage as StdioMessage,
    JsonRpcNotification as StdioNotification, StdioProcessConfig, StdioProcessRunner,
    StdioRunnerError,
};
use serde_json::json;
use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

fn run_cmd_req(params: serde_json::Value) -> ToolInvocationRequest {
    ToolInvocationRequest::new("run_command", params)
}

#[test]
fn policy_parses_multiline_arrays_and_deduplicates_on_merge() {
    let global = r#"
        [permissions]
        allow_read_tools = true
        auto_allow_writes_in_worktrees = true
        allowed_command_prefixes = [
            # comment inside multiline array
            "cargo check",
            'git status',
        ]
        allow_commands = [
            "cargo test",
            "git diff*",
        ]
        deny_commands = [
            "git push*",
            "rm -rf*",
        ]
    "#;

    let local = r#"
        allow_read_tools = false
        auto_allow_writes_in_worktrees = false
        allowed_command_prefixes = [
            "git status",
            "cargo clippy",
        ]
        allow_commands = [
            "cargo test",
            "cargo fmt --check",
        ]
        deny_commands = [
            "rm -rf*",
            "sudo*",
        ]
        unrecognized_key = "ignored"
        line_without_equals
    "#;

    let policy =
        SecurityPolicy::from_toml_configs(Some(global), Some(local)).expect("parse multiline");
    assert!(policy.allow_read_tools());
    assert!(policy.auto_allow_writes_in_worktrees());
    assert_eq!(
        policy.allowed_command_prefixes(),
        &["cargo check", "git status", "cargo clippy"]
    );
    assert_eq!(
        policy.allow_commands(),
        &["cargo test", "git diff*", "cargo fmt --check"]
    );
    assert_eq!(policy.deny_commands(), &["git push*", "rm -rf*", "sudo*"]);

    let gate = PolicyGate::new(policy);
    for read_tool in ["read_file", "read", "list_files", "list_dir", "search"] {
        assert_eq!(
            gate.evaluate(&ToolInvocationRequest::new(read_tool, json!({}))),
            PolicyDecision::Allow
        );
    }
    for write_tool in [
        "write_file",
        "edit_file",
        "delete_file",
        "create_file",
        "write",
    ] {
        let req = ToolInvocationRequest::new(write_tool, json!({})).with_isolated_worktree(true);
        assert_eq!(gate.evaluate(&req), PolicyDecision::Allow);
    }
}

#[test]
fn policy_rejects_malformed_toml_inputs_and_formats_errors() {
    let cases = [
        ("allow_read_tools = yes", "Invalid boolean"),
        ("auto_allow_writes_in_worktrees = 123", "Invalid boolean"),
        ("allow_commands = \"cargo test\"", "Expected array"),
        ("deny_commands = true", "Expected array"),
        ("allowed_command_prefixes = nope", "Expected array"),
        ("allow_commands = [cargo test]", "Malformed string element"),
        (
            "allow_commands = [\n  \"cargo test\",\n  unquoted\n]",
            "Malformed string element",
        ),
        (
            "deny_commands = [\n  \"rm -rf\"\n] trailing",
            "Malformed array",
        ),
    ];

    for (toml, expected_substr) in cases {
        let err = SecurityPolicy::from_toml_configs(Some(toml), None)
            .expect_err("malformed TOML must fail");
        assert!(
            matches!(err, PolicyError::ParseError(_)),
            "expected ParseError for `{toml}`, got {err:?}"
        );
        let display = err.to_string();
        assert!(display.starts_with("Policy parse error: "));
        assert!(
            display.contains(expected_substr),
            "expected `{expected_substr}` in `{display}`"
        );
    }
}

#[test]
fn policy_gate_scoped_always_thread_and_deny_and_param_aliases() {
    let policy = SecurityPolicy::builder()
        .allow_commands(vec!["cargo test".into(), "   ".into()])
        .deny_commands(vec!["rm -rf*".into()])
        .build();
    let mut gate = PolicyGate::new(policy);

    let cmd_alias = run_cmd_req(json!({ "command": "cargo test --lib" }));
    assert_eq!(cmd_alias.command_line(), Some("cargo test --lib"));
    assert_eq!(gate.evaluate(&cmd_alias), PolicyDecision::Allow);

    let short_alias = run_cmd_req(json!({ "cmd": "rm -rf /tmp/x" }));
    assert_eq!(short_alias.command_line(), Some("rm -rf /tmp/x"));
    assert_eq!(gate.evaluate(&short_alias), PolicyDecision::Deny);

    let path_upper = ToolInvocationRequest::new("write_file", json!({ "Path": "src/a.rs" }));
    let path_lower = ToolInvocationRequest::new("write_file", json!({ "path": "src/b.rs" }));
    let path_missing = ToolInvocationRequest::new("write_file", json!({ "path": 42 }));
    assert_eq!(path_upper.path(), Some("src/a.rs"));
    assert_eq!(path_lower.path(), Some("src/b.rs"));
    assert_eq!(path_missing.path(), None);
    assert_eq!(path_missing.command_line(), None);

    let s1 = SessionId::new("sess-1");
    let s2 = SessionId::new("sess-2");
    let t1 = ThreadId::new("thread-1");
    let t2 = ThreadId::new("thread-2");

    let edit_req = ToolInvocationRequest::new("edit_file", json!({ "path": "src/lib.rs" }));
    assert_eq!(
        gate.evaluate_scoped(&s1, &t1, &edit_req),
        PolicyDecision::PromptUser
    );

    gate.record_decision(&s1, &t1, &edit_req, PermissionDecision::AlwaysThread);
    assert_eq!(
        gate.evaluate_scoped(&s1, &t1, &edit_req),
        PolicyDecision::Allow
    );
    assert_eq!(
        gate.evaluate_scoped(&s1, &t2, &edit_req),
        PolicyDecision::PromptUser
    );

    let delete_req = ToolInvocationRequest::new("delete_file", json!({ "path": "src/old.rs" }));
    gate.record_decision(&s1, &t1, &delete_req, PermissionDecision::Deny);
    assert_eq!(
        gate.evaluate_scoped(&s1, &t1, &delete_req),
        PolicyDecision::Deny
    );
    assert_eq!(
        gate.evaluate_scoped(&s1, &t2, &delete_req),
        PolicyDecision::Deny
    );
    assert_eq!(
        gate.evaluate_scoped(&s2, &t1, &delete_req),
        PolicyDecision::PromptUser
    );
}

#[test]
fn framing_encodes_notifications_and_responses_and_rejects_protocol_violations() {
    let codec = StdioFrameCodec;

    let notif = JsonRpcMessage::Notification(JsonRpcNotification::new(
        "progress".to_string(),
        Some(json!({"pct": 50})),
    ));
    let notif_bytes = codec.encode_message(&notif).expect("encode notification");
    assert!(notif_bytes.ends_with(b"\n"));
    let decoded_notif = codec
        .decode_message(&notif_bytes)
        .expect("decode notification");
    assert_eq!(decoded_notif, notif);

    let notif_no_params =
        JsonRpcMessage::Notification(JsonRpcNotification::new("heartbeat".to_string(), None));
    let decoded_no_params = codec
        .decode_message(&codec.encode_message(&notif_no_params).expect("encode"))
        .expect("decode");
    assert_eq!(decoded_no_params, notif_no_params);

    let ok_res = JsonRpcMessage::Response(JsonRpcResponse::new_result(
        JsonRpcId::String("req-9".into()),
        json!({"ok": true}),
    ));
    let ok_bytes = codec.encode_message(&ok_res).expect("encode result");
    assert_eq!(codec.decode_message(&ok_bytes).expect("decode"), ok_res);

    let rpc_err = JsonRpcError::with_data(-32001, "failed", json!({"detail": "boom"}));
    assert_eq!(rpc_err.code(), -32001);
    assert_eq!(rpc_err.message(), "failed");
    assert_eq!(rpc_err.data(), Some(&json!({"detail": "boom"})));
    let err_res = JsonRpcMessage::Response(JsonRpcResponse::new_error(
        JsonRpcId::Number(7),
        rpc_err.clone(),
    ));
    let err_bytes = codec
        .encode_message(&err_res)
        .expect("encode error response");
    assert_eq!(codec.decode_message(&err_bytes).expect("decode"), err_res);

    let req = JsonRpcRequest::new(JsonRpcId::Number(3), "ping".into(), Some(json!({"a": 1})));
    assert_eq!(req.params(), Some(&json!({"a": 1})));

    let violations: &[(&[u8], &str)] = &[
        (b"[1, 2, 3]", "Expected JSON object"),
        (b"\"scalar\"", "Expected JSON object"),
        (b"{\"jsonrpc\":\"1.0\",\"method\":\"m\"}", "jsonrpc"),
        (b"{\"jsonrpc\":\"2.0\",\"id\":true,\"method\":\"m\"}", "Invalid 'id'"),
        (b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":123}", "Field 'method' must be a valid string"),
        (b"{\"jsonrpc\":\"2.0\",\"id\":1}", "must contain either 'method', 'result', or 'error'"),
        (
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":\"bad\"}",
            "Invalid 'error' object",
        ),
        (
            b"{\"jsonrpc\":\"2.0\",\"method\":99}",
            "Notification 'method' must be a valid string",
        ),
        (
            b"{\"jsonrpc\":\"2.0\",\"params\":{}}",
            "missing both 'id' and 'method'",
        ),
    ];

    for (raw, snippet) in violations {
        let err = codec
            .decode_message(raw)
            .expect_err("protocol violation must fail");
        match &err {
            AcpError::Framing(FramingError::ProtocolViolation(msg)) => {
                assert!(
                    msg.contains(snippet),
                    "expected `{snippet}` in `{msg}` for {:?}",
                    String::from_utf8_lossy(raw)
                );
            }
            other => panic!("expected ProtocolViolation, got {other:?}"),
        }
        assert!(err.to_string().contains("Framing error:"));
        assert!(err.source().is_some());
    }

    let io_err: AcpError = std::io::Error::other("broken pipe").into();
    assert!(io_err.to_string().contains("IO error:"));
    assert!(io_err.source().is_some());

    let ser_err: AcpError = serde_json::from_str::<serde_json::Value>("{").unwrap_err().into();
    assert!(ser_err.to_string().contains("Serialization error:"));
    assert!(ser_err.source().is_some());

    let sub_err = AcpError::Subprocess("crashed".into());
    assert_eq!(sub_err.to_string(), "Subprocess error: crashed");
    assert!(sub_err.source().is_none());

    let inv_err = FramingError::InvalidPayload("bad".into());
    assert_eq!(inv_err.to_string(), "Invalid payload: bad");
}

#[test]
fn session_types_manager_idempotency_and_errors() {
    let sid = SessionId::new("sess-alpha");
    assert_eq!(sid.as_str(), "sess-alpha");
    assert_eq!(sid.to_string(), "sess-alpha");

    let tid = ThreadId::new("thread-alpha");
    assert_eq!(tid.as_str(), "thread-alpha");
    assert_eq!(tid.to_string(), "thread-alpha");

    let direct = Session::new(sid.clone(), tid.clone());
    assert_eq!(direct.id(), &sid);
    assert_eq!(direct.thread_id(), &tid);

    let err = SessionError::SessionNotFound("missing-1".into());
    assert_eq!(err.to_string(), "Session not found: missing-1");
    let _: &dyn std::error::Error = &err;

    let mut manager = SessionManager::new(false);
    assert!(!manager.process_per_session());
    assert_eq!(manager.active_session_count(), 0);
    assert_eq!(manager.get_session(&tid), None);

    let first = manager.get_or_create_session(&tid).expect("create session");
    let second = manager
        .get_or_create_session(&tid)
        .expect("reuse existing session");
    assert_eq!(first, second);
    assert_eq!(first.thread_id(), &tid);
    assert_eq!(manager.active_session_count(), 1);
    assert_eq!(manager.get_session(&tid), Some(&first));
}

#[tokio::test]
async fn subprocess_runner_send_after_shutdown_and_error_variants() {
    let spawn_err = RunnerError::SpawnFailed("missing".into());
    let io_err = RunnerError::Io("disconnected".into());
    let ser_err = RunnerError::Serialization("bad".into());
    let not_running = RunnerError::NotRunning;
    let chan_err = RunnerError::Channel("closed".into());
    assert!(spawn_err.to_string().contains("Failed to spawn subprocess"));
    assert!(io_err.to_string().contains("I/O error"));
    assert!(ser_err.to_string().contains("Serialization error"));
    assert_eq!(not_running.to_string(), "Process is not running");
    assert!(chan_err.to_string().contains("Channel error"));

    let empty_resp = RunnerResponse::new("10", "ping");
    assert_eq!(empty_resp.id, "10");
    assert_eq!(empty_resp.method, "ping");
    assert!(empty_resp.result.is_none());
    assert!(empty_resp.error.is_none());
    assert!(empty_resp.params.is_none());

    let mut runner = SubprocessRunner::spawn(SubprocessConfig::new("cat", vec![]))
        .await
        .expect("spawn cat");
    assert!(runner.is_running().await);
    runner.shutdown().await.expect("shutdown");
    assert!(!runner.is_running().await);

    for req_id in ["after-1", "after-2"] {
        let err = runner
            .send_request(RunnerRequest::new(req_id, "ping", json!({})))
            .await
            .expect_err("send_request after shutdown must fail");
        assert!(
            matches!(err, RunnerError::Io(ref msg) if msg.contains("not connected")),
            "expected NotConnected I/O error, got {err:?}"
        );
    }

    let mut short_lived = SubprocessRunner::spawn(SubprocessConfig::new("true", vec![]))
        .await
        .expect("spawn true");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!short_lived.is_running().await);
    short_lived.shutdown().await.expect("shutdown exited child");
}

#[tokio::test]
async fn stdio_process_runner_drops_non_v2_frames_and_handles_kill_and_double_terminate() {
    let script = concat!(
        "printf 'not valid json\\n\\n';",
        "printf '{\"jsonrpc\":\"1.0\",\"method\":\"legacy\"}\\n';",
        "printf '{\"jsonrpc\":\"2.0\",\"method\":\"valid_one\"}\\n';",
        "sleep 5"
    );
    let mut runner =
        StdioProcessRunner::spawn(StdioProcessConfig::new("sh", ["-c", script]))
            .await
            .expect("spawn");

    let mut notif_rx = runner.take_notification_rx().expect("first notif rx");
    assert!(runner.take_notification_rx().is_none());
    let _msg_rx = runner.take_message_rx().expect("first msg rx");
    assert!(runner.take_message_rx().is_none());

    let notif = timeout(Duration::from_secs(2), notif_rx.recv())
        .await
        .expect("wait valid notif")
        .expect("notif present");
    assert_eq!(notif.method, "valid_one");

    let exit_status = runner
        .terminate(Duration::from_millis(20))
        .await
        .expect("force kill on timeout");
    assert!(!exit_status.success());
    assert!(!runner.is_running());

    let second = runner
        .terminate(Duration::from_millis(20))
        .await
        .expect_err("double terminate must return ProcessTerminated");
    assert!(matches!(second, StdioRunnerError::ProcessTerminated(_)));
    assert!(second.to_string().contains("terminated"));

    let cat_runner = StdioProcessRunner::spawn(StdioProcessConfig::new("cat", Vec::<String>::new()))
        .await
        .expect("spawn cat");
    let (tx, mut rx) = cat_runner.into_channels();
    tx.send(StdioMessage::Notification(StdioNotification {
        jsonrpc: "1.0".into(),
        method: "drop_outgoing".into(),
        params: None,
    }))
    .await
    .expect("queue 1.0");
    let valid_req = StdioMessage::Request(grein_acp::JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: json!(1),
        method: "echo_ok".into(),
        params: None,
    });
    tx.send(valid_req.clone()).await.expect("queue 2.0");

    let echoed = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("wait echoed")
        .expect("echoed message");
    assert_eq!(echoed, valid_req);
}

#[tokio::test]
async fn stdio_driver_broadcasts_unmatched_errors_and_fails_pending_rpcs_on_shutdown() {
    let script = concat!(
        "IFS= read -r _line;",
        "printf '{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32099,\"message\":\"global overload\"}}\\n';",
        "printf '{\"jsonrpc\":\"2.0\",\"id\":999999,\"error\":{\"message\":\"unmatched stringless\"}}\\n';",
        "sleep 5"
    );
    let runner = StdioProcessRunner::spawn(StdioProcessConfig::new("sh", ["-c", script]))
        .await
        .expect("spawn");
    let driver = Arc::new(AcpStdioDriver::new(runner));
    assert!(driver.is_running());

    let mut persistent_rx = driver.subscribe_session("sess-unmatched");
    let mut turn_rx = driver
        .send_prompt("sess-unmatched", "trigger")
        .await
        .expect("send_prompt");

    let p_ev1 = timeout(Duration::from_secs(2), persistent_rx.recv())
        .await
        .expect("p_ev1")
        .expect("event");
    let p_ev2 = timeout(Duration::from_secs(2), persistent_rx.recv())
        .await
        .expect("p_ev2")
        .expect("event");
    let t_ev1 = timeout(Duration::from_secs(2), turn_rx.recv())
        .await
        .expect("t_ev1")
        .expect("event");
    let t_ev2 = timeout(Duration::from_secs(2), turn_rx.recv())
        .await
        .expect("t_ev2")
        .expect("event");

    assert_eq!(
        p_ev1,
        DriverEvent::Error {
            session_id: None,
            message: "JSON-RPC error -32099: global overload".into(),
        }
    );
    assert_eq!(
        p_ev2,
        DriverEvent::Error {
            session_id: None,
            message: "unmatched stringless".into(),
        }
    );
    assert_eq!(t_ev1, p_ev1);
    assert_eq!(t_ev2, p_ev2);

    let driver_clone = Arc::clone(&driver);
    let pending_init = tokio::spawn(async move { driver_clone.initialize().await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    driver.shutdown().await.expect("first shutdown");
    driver.shutdown().await.expect("double shutdown is idempotent");

    let init_err = timeout(Duration::from_secs(2), pending_init)
        .await
        .expect("join")
        .expect("task")
        .expect_err("pending RPC must fail on shutdown");
    assert!(matches!(init_err, StdioRunnerError::ProcessTerminated(_)));

    let turn_end = timeout(Duration::from_secs(2), turn_rx.recv())
        .await
        .expect("turn_end")
        .expect("DriverEvent::Error on shutdown");
    assert!(matches!(
        turn_end,
        DriverEvent::Error {
            session_id: Some(ref sid),
            ..
        } if sid == "sess-unmatched"
    ));

    let post_shutdown = driver.initialize().await.expect_err("post-shutdown rpc");
    assert!(matches!(post_shutdown, StdioRunnerError::ProcessTerminated(_)));
    let closed_sub = driver.subscribe_session("after-shutdown").recv().await;
    assert!(closed_sub.is_none());

    let io_err: StdioRunnerError = std::io::Error::other("spawn boom").into();
    assert!(io_err.to_string().contains("Failed to spawn subprocess"));
    assert!(io_err.source().is_some());
    assert_eq!(
        StdioRunnerError::Timeout.to_string(),
        "Process termination timed out"
    );
    assert!(StdioRunnerError::Timeout.source().is_none());
    assert!(StdioRunnerError::CommunicationError("x".into())
        .to_string()
        .contains("Communication error:"));
}
