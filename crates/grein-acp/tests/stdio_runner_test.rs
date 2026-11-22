use grein_acp::{
    AcpInitializeResult, AcpPermissionOption, AcpSessionInfo, AcpStdioDriver, AcpUsage,
    AgentDriver, DriverEvent, JsonRpcMessage, JsonRpcRequest, StdioProcessConfig,
    StdioProcessRunner,
};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

struct TempCaptureFile {
    path: PathBuf,
}

impl TempCaptureFile {
    fn new(label: &str) -> Self {
        let unique = format!(
            "grein-acp-test-{}-{}-{}.jsonl",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        let _ = fs::remove_file(&path);
        Self { path }
    }

    fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn read_json_lines(&self) -> Vec<serde_json::Value> {
        let content = fs::read_to_string(&self.path).expect("capture file should exist");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("valid captured json"))
            .collect()
    }
}

impl Drop for TempCaptureFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[tokio::test]
async fn test_subprocess_startup_and_handshake() {
    let config = StdioProcessConfig::new("cat", Vec::<String>::new());
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    assert!(
        runner.is_running(),
        "Process must report as actively running"
    );
    assert!(runner.pid().is_some(), "Process must have an allocated PID");

    runner
        .terminate(Duration::from_millis(500))
        .await
        .expect("Process should terminate cleanly");
}

#[tokio::test]
async fn test_message_multiplexing_bidirectional() {
    let config = StdioProcessConfig::new("cat", Vec::<String>::new());
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    let (client, mut incoming) = runner.into_channels();

    let request = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: "echo".into(),
        params: Some(serde_json::json!({"data": "ping"})),
    });

    client
        .send(request.clone())
        .await
        .expect("Message send should succeed");

    let response = timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("Timed out waiting for response")
        .expect("Channel should return echoed message");

    assert_eq!(
        response, request,
        "Cat echo must multiplex identical JSON-RPC frame"
    );
}

#[tokio::test]
async fn test_error_propagation_on_non_zero_exit() {
    let config = StdioProcessConfig::new("false", Vec::<String>::new());
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Spawn should succeed initially before immediate exit");

    let (client, mut incoming) = runner.into_channels();

    let request = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(42),
        method: "test".into(),
        params: None,
    });

    let send_result = client.send(request).await;
    let recv_result = incoming.recv().await;

    let has_error = send_result.is_err()
        || recv_result.is_none()
        || matches!(recv_result, Some(JsonRpcMessage::Error(_)));
    assert!(
        has_error,
        "Unexpected non-zero subprocess termination must propagate error"
    );
}

#[tokio::test]
async fn test_graceful_process_termination() {
    let config = StdioProcessConfig::new("cat", Vec::<String>::new());
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    assert!(runner.is_running());

    let exit_status = runner
        .terminate(Duration::from_millis(500))
        .await
        .expect("Termination must succeed without timing out");

    assert!(
        !runner.is_running(),
        "Process must not be running after termination"
    );
    assert!(
        exit_status.success(),
        "Gracefully terminated process should yield success status"
    );
}

#[tokio::test]
async fn test_notification_streaming() {
    let script = r#"printf '{"jsonrpc":"2.0","method":"stream/chunk","params":{"text":"one"}}\n{"jsonrpc":"2.0","method":"stream/chunk","params":{"text":"two"}}\n'; sleep 2"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()]);
    let mut runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    let mut notifications = runner
        .take_notification_rx()
        .expect("Notification receiver must be available once");

    for expected in ["one", "two"] {
        let notification = timeout(Duration::from_secs(2), notifications.recv())
            .await
            .expect("Timed out waiting for notification")
            .expect("Notification channel should yield a frame");
        assert_eq!(notification.jsonrpc, "2.0");
        assert_eq!(notification.method, "stream/chunk");
        let text = notification
            .params
            .as_ref()
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str());
        assert_eq!(text, Some(expected));
    }
}

#[tokio::test]
async fn test_large_stderr_output_is_drained_without_deadlock() {
    let script = r#"i=0; while [ $i -lt 4000 ]; do echo "0123456789012345678901234567890123456789012345678901234567890123456789012345678" >&2; i=$((i+1)); done; printf '{"jsonrpc":"2.0","method":"done"}\n'; sleep 2"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()]);
    let mut runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    let mut notifications = runner
        .take_notification_rx()
        .expect("Notification receiver must be available once");

    let notification = timeout(Duration::from_secs(10), notifications.recv())
        .await
        .expect("Child blocked on stderr pipe: drain deadlock")
        .expect("Notification channel should yield the completion frame");
    assert_eq!(notification.method, "done");
    assert!(!runner.recent_stderr().is_empty());
}

#[tokio::test]
async fn test_stdio_process_config_with_current_dir_and_env() {
    let tmp_dir = fs::canonicalize(std::env::temp_dir()).expect("canonical temp dir");
    let script = r#"printf '{"jsonrpc":"2.0","method":"env_check","params":{"pwd":"%s","val":"%s"}}\n' "$(pwd)" "$GREIN_ACP_TEST_ENV"; sleep 1"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_current_dir(&tmp_dir)
        .with_env([("GREIN_ACP_TEST_ENV".to_string(), "worktree-ok".to_string())]);

    let mut runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess with cwd and env should spawn");
    let mut notifications = runner
        .take_notification_rx()
        .expect("notification rx available");

    let notification = timeout(Duration::from_secs(2), notifications.recv())
        .await
        .expect("timed out")
        .expect("notification expected");
    let params = notification.params.expect("params expected");
    assert_eq!(
        params.get("pwd").and_then(|v| v.as_str()),
        Some(tmp_dir.to_string_lossy().as_ref())
    );
    assert_eq!(
        params.get("val").and_then(|v| v.as_str()),
        Some("worktree-ok")
    );
    let _ = runner.terminate(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn test_acp_driver_initialize_and_authenticate() {
    let capture = TempCaptureFile::new("init_auth");
    let script = r#"
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CAPTURE_FILE"
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"antigravity","version":"1.5.14"},"authMethods":[{"id":"oauth-personal","name":"OAuth Personal","description":"Google OAuth"},{"id":"api-key","name":"API Key","description":"Key"}],"agentCapabilities":{"_meta":{"antigravity":{"systemPrompt":{"append":true}}}}}}\n' "$id"
            ;;
        *'"method":"authenticate"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
            ;;
    esac
done
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_env([("CAPTURE_FILE".to_string(), capture.path_string())]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = AcpStdioDriver::new(runner);

    let init: AcpInitializeResult = driver.initialize().await.expect("initialize should succeed");
    assert_eq!(init.protocol_version, 1);
    assert_eq!(init.agent_name.as_deref(), Some("antigravity"));
    assert_eq!(init.agent_version.as_deref(), Some("1.5.14"));
    assert!(init.supports_system_prompt_append);
    assert_eq!(init.auth_method_ids, vec!["oauth-personal", "api-key"]);

    driver
        .authenticate("oauth-personal")
        .await
        .expect("authenticate should succeed");

    driver.shutdown().await.expect("shutdown");

    let captured = capture.read_json_lines();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0]["method"], "initialize");
    assert_eq!(captured[0]["params"]["protocolVersion"], 1);
    assert_eq!(captured[0]["params"]["clientCapabilities"], serde_json::json!({}));
    assert_eq!(captured[0]["params"]["clientInfo"]["name"], "grein");
    assert_eq!(captured[1]["method"], "authenticate");
    assert_eq!(captured[1]["params"]["methodId"], "oauth-personal");
}

#[tokio::test]
async fn test_acp_driver_new_session_meta_and_omission() {
    let capture = TempCaptureFile::new("new_session");
    let script = r#"
count=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CAPTURE_FILE"
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    count=$((count+1))
    printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s","models":{"currentModelId":"gemini-2.5-pro"},"modes":{"currentModeId":"code"}}}\n' "$id" "$count"
done
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_env([("CAPTURE_FILE".to_string(), capture.path_string())]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = AcpStdioDriver::new(runner);

    let s1: AcpSessionInfo = driver
        .new_session("/workspace/repo", Some("Follow TDD strictly."))
        .await
        .expect("new_session with system prompt");
    assert_eq!(s1.session_id, "sess-1");
    assert_eq!(s1.current_model_id.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(s1.current_mode_id.as_deref(), Some("code"));

    let s2 = driver
        .new_session("/workspace/repo", None)
        .await
        .expect("new_session without system prompt");
    assert_eq!(s2.session_id, "sess-2");

    let s3 = driver
        .new_session("/workspace/repo", Some(""))
        .await
        .expect("new_session with empty system prompt");
    assert_eq!(s3.session_id, "sess-3");

    driver.shutdown().await.expect("shutdown");

    let captured = capture.read_json_lines();
    assert_eq!(captured.len(), 3);

    assert_eq!(captured[0]["method"], "session/new");
    assert_eq!(captured[0]["params"]["cwd"], "/workspace/repo");
    assert_eq!(captured[0]["params"]["mcpServers"], serde_json::json!([]));
    assert!(captured[0]["params"].get("systemPrompt").is_none());
    assert_eq!(
        captured[0]["params"]["_meta"]["antigravity"]["systemPrompt"]["append"],
        "Follow TDD strictly."
    );

    assert!(captured[1]["params"].get("_meta").is_none());
    assert!(captured[2]["params"].get("_meta").is_none());
}

#[tokio::test]
async fn test_acp_driver_multiplexed_sessions_and_all_updates() {
    let capture = TempCaptureFile::new("multiplex");
    let script = r#"
seen=0
id_a=""
id_b=""
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CAPTURE_FILE"
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"sessionId":"sess-a"'*) id_a="$id"; seen=$((seen+1)) ;;
        *'"sessionId":"sess-b"'*) id_b="$id"; seen=$((seen+1)) ;;
    esac
    if [ "$seen" -eq 2 ]; then
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-a","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking-a"}}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-b","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello-b"}}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-a","update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-a","update":{"sessionUpdate":"tool_call","toolCallId":"tc-1","name":"read_file","title":"Read lib.rs","args":{"Path":"lib.rs"},"status":"running"}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-b","update":{"sessionUpdate":"config_option_update","options":[]}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-a","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-1","name":"read_file","status":"completed","rawOutput":{"output":"fn main() {}"}}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-b","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-2","name":"run_command","status":"completed","content":[{"type":"content","content":{"type":"text","text":"ok-from-content"}}]}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-a","update":{"sessionUpdate":"session_info_update","title":"Branch A Title"}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-a","update":{"sessionUpdate":"usage_update","used":150,"size":100000}}}\n'
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-a","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done-a"}}}}\n'
        printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn","usage":{"inputTokens":10,"outputTokens":20,"cachedReadTokens":3,"cachedWriteTokens":4,"thoughtTokens":5,"totalTokens":42}}}\n' "$id_b"
        printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id_a"
        break
    fi
done
sleep 1
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_env([("CAPTURE_FILE".to_string(), capture.path_string())]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = Arc::new(AcpStdioDriver::new(runner));

    let trait_driver: Arc<dyn AgentDriver> = driver.clone();

    let (rx_a, rx_b) = tokio::try_join!(
        trait_driver.send_prompt("sess-a", "Prompt A"),
        trait_driver.send_prompt("sess-b", "Prompt B")
    )
    .expect("both send_prompt calls should succeed");

    let collect_all = |mut rx: tokio::sync::mpsc::UnboundedReceiver<DriverEvent>| async move {
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    };

    let (events_a, events_b) = timeout(Duration::from_secs(4), async {
        tokio::join!(collect_all(rx_a), collect_all(rx_b))
    })
    .await
    .expect("multiplexed streams should finish promptly");

    assert_eq!(
        events_a,
        vec![
            DriverEvent::ThoughtChunk {
                session_id: "sess-a".into(),
                text: "thinking-a".into(),
            },
            DriverEvent::ToolCall {
                session_id: "sess-a".into(),
                tool_call_id: "tc-1".into(),
                name: "read_file".into(),
                title: "Read lib.rs".into(),
                args: serde_json::json!({"Path": "lib.rs"}),
                status: "running".into(),
            },
            DriverEvent::ToolCallUpdate {
                session_id: "sess-a".into(),
                tool_call_id: "tc-1".into(),
                name: "read_file".into(),
                status: "completed".into(),
                output: "fn main() {}".into(),
            },
            DriverEvent::SessionInfoUpdate {
                session_id: "sess-a".into(),
                title: "Branch A Title".into(),
            },
            DriverEvent::UsageUpdate {
                session_id: "sess-a".into(),
                used: 150,
                size: 100000,
            },
            DriverEvent::StreamChunk {
                session_id: "sess-a".into(),
                text: "done-a".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: "sess-a".into(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    assert_eq!(
        events_b,
        vec![
            DriverEvent::StreamChunk {
                session_id: "sess-b".into(),
                text: "hello-b".into(),
            },
            DriverEvent::ToolCallUpdate {
                session_id: "sess-b".into(),
                tool_call_id: "tc-2".into(),
                name: "run_command".into(),
                status: "completed".into(),
                output: "ok-from-content".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: "sess-b".into(),
                stop_reason: "end_turn".into(),
                usage: Some(AcpUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cached_read_tokens: 3,
                    cached_write_tokens: 4,
                    thought_tokens: 5,
                    total_tokens: 42,
                }),
            },
        ]
    );

    trait_driver.shutdown().await.expect("shutdown");

    let captured = capture.read_json_lines();
    assert_eq!(captured.len(), 2);
    for req in &captured {
        assert_eq!(req["method"], "session/prompt");
        assert!(req["params"]["prompt"].is_array());
        assert_eq!(req["params"]["prompt"][0]["type"], "text");
    }
}

#[tokio::test]
async fn test_acp_driver_permission_request_preserves_string_id() {
    let capture = TempCaptureFile::new("perm");
    let script = r#"
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CAPTURE_FILE"
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","id":"1","method":"session/request_permission","params":{"sessionId":"sess-perm","toolCall":{"toolCallId":"tc-p1","name":"run_command","title":"Run tests","args":{"CommandLine":"cargo test"}},"options":[{"optionId":"allow_once","name":"Allow Once","kind":"allow_once"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]}}\n'
            IFS= read -r reply1
            printf '%s\n' "$reply1" >> "$CAPTURE_FILE"
            printf '{"jsonrpc":"2.0","id":"perm-2","method":"session/request_permission","params":{"sessionId":"sess-perm","toolCall":{"toolCallId":"tc-p2","name":"write_file","title":"Write file","args":{"Path":"a.rs"}},"options":[{"optionId":"allow_always","name":"Always Allow","kind":"allow_always"}]}}\n'
            IFS= read -r reply2
            printf '%s\n' "$reply2" >> "$CAPTURE_FILE"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            break
            ;;
    esac
done
sleep 1
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_env([("CAPTURE_FILE".to_string(), capture.path_string())]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = AcpStdioDriver::new(runner);

    let mut events = driver
        .send_prompt("sess-perm", "Run tests and edit")
        .await
        .expect("send_prompt");

    let ev1 = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timed out ev1")
        .expect("ev1");
    match ev1 {
        DriverEvent::PermissionRequested {
            request_id,
            session_id,
            tool_call_id,
            tool_name,
            title,
            args,
            options,
        } => {
            assert_eq!(request_id, serde_json::json!("1"));
            assert_eq!(session_id, "sess-perm");
            assert_eq!(tool_call_id, "tc-p1");
            assert_eq!(tool_name, "run_command");
            assert_eq!(title, "Run tests");
            assert_eq!(args, serde_json::json!({"CommandLine": "cargo test"}));
            assert_eq!(
                options,
                vec![
                    AcpPermissionOption {
                        option_id: "allow_once".into(),
                        name: "Allow Once".into(),
                        kind: "allow_once".into(),
                    },
                    AcpPermissionOption {
                        option_id: "reject_once".into(),
                        name: "Reject".into(),
                        kind: "reject_once".into(),
                    },
                ]
            );
            driver
                .respond_permission(request_id, Some("allow_once"))
                .await
                .expect("respond_permission selected");
        }
        other => panic!("expected PermissionRequested, got {other:?}"),
    }

    let ev2 = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timed out ev2")
        .expect("ev2");
    match ev2 {
        DriverEvent::PermissionRequested { request_id, .. } => {
            assert_eq!(request_id, serde_json::json!("perm-2"));
            driver
                .respond_permission(request_id, None)
                .await
                .expect("respond_permission cancelled");
        }
        other => panic!("expected second PermissionRequested, got {other:?}"),
    }

    let ev3 = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timed out ev3")
        .expect("ev3");
    assert_eq!(
        ev3,
        DriverEvent::PromptCompleted {
            session_id: "sess-perm".into(),
            stop_reason: "end_turn".into(),
            usage: None,
        }
    );

    driver.shutdown().await.expect("shutdown");

    let captured = capture.read_json_lines();
    assert_eq!(captured.len(), 3);
    assert_eq!(
        captured[1],
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "result": {
                "outcome": {
                    "outcome": "selected",
                    "optionId": "allow_once"
                }
            }
        })
    );
    assert_eq!(
        captured[2],
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "perm-2",
            "result": {
                "outcome": {
                    "outcome": "cancelled"
                }
            }
        })
    );
}

#[tokio::test]
async fn test_acp_driver_cancel_notification_has_no_id_and_close_session() {
    let capture = TempCaptureFile::new("cancel_close");
    let script = r#"
prompt_id=""
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CAPTURE_FILE"
    case "$line" in
        *'"method":"session/prompt"'*)
            rest=${line#*\"id\":}
            id=${rest%%,*}
            prompt_id=${id%%\}*}
            ;;
        *'"method":"session/cancel"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"cancelled"}}\n' "$prompt_id"
            ;;
        *'"method":"session/close"'*)
            rest=${line#*\"id\":}
            id=${rest%%,*}
            id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
            break
            ;;
    esac
done
sleep 1
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_env([("CAPTURE_FILE".to_string(), capture.path_string())]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = AcpStdioDriver::new(runner);

    let mut events = driver
        .send_prompt("sess-cancel", "do work")
        .await
        .expect("send_prompt");

    driver
        .cancel_session("sess-cancel")
        .await
        .expect("cancel_session");

    let ev = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timed out")
        .expect("PromptCompleted expected");
    assert_eq!(
        ev,
        DriverEvent::PromptCompleted {
            session_id: "sess-cancel".into(),
            stop_reason: "cancelled".into(),
            usage: None,
        }
    );

    driver
        .close_session("sess-cancel")
        .await
        .expect("close_session should succeed");

    driver.shutdown().await.expect("shutdown");

    let captured = capture.read_json_lines();
    assert_eq!(captured.len(), 3);
    let cancel_msg = &captured[1];
    assert_eq!(cancel_msg["method"], "session/cancel");
    assert!(
        cancel_msg.get("id").is_none(),
        "session/cancel must be a strict notification with NO id field: {cancel_msg}"
    );
    assert_eq!(cancel_msg["params"]["sessionId"], "sess-cancel");

    let close_msg = &captured[2];
    assert_eq!(close_msg["method"], "session/close");
    assert!(close_msg.get("id").is_some());
    assert_eq!(close_msg["params"]["sessionId"], "sess-cancel");
}

#[tokio::test]
async fn test_acp_driver_crash_mid_rpc_and_mid_turn_propagates_errors() {
    // Case A: crash mid-RPC (initialize)
    let script_rpc = r#"
IFS= read -r _line
echo "fatal startup stderr" >&2
exit 1
"#;
    let config_rpc = StdioProcessConfig::new("sh", vec!["-c".to_string(), script_rpc.to_string()]);
    let runner_rpc = StdioProcessRunner::spawn(config_rpc)
        .await
        .expect("spawn rpc");
    let driver_rpc = AcpStdioDriver::new(runner_rpc);

    let init_res = timeout(Duration::from_secs(2), driver_rpc.initialize())
        .await
        .expect("initialize must not hang on crash");
    assert!(init_res.is_err(), "crash mid-RPC must return Err");
    assert!(
        driver_rpc
            .recent_stderr()
            .iter()
            .any(|line| line.contains("fatal startup stderr")),
        "recent_stderr should capture crash diagnostics"
    );

    // Case B: crash mid-turn (send_prompt)
    let script_turn = r#"
IFS= read -r _line
echo "mid-turn crash stderr" >&2
printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-crash","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"partial"}}}}\n'
exit 1
"#;
    let config_turn =
        StdioProcessConfig::new("sh", vec!["-c".to_string(), script_turn.to_string()]);
    let runner_turn = StdioProcessRunner::spawn(config_turn)
        .await
        .expect("spawn turn");
    let driver_turn = AcpStdioDriver::new(runner_turn);

    let mut events = driver_turn
        .send_prompt("sess-crash", "go")
        .await
        .expect("send_prompt");

    let first = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("first event")
        .expect("stream chunk expected");
    assert_eq!(
        first,
        DriverEvent::StreamChunk {
            session_id: "sess-crash".into(),
            text: "partial".into(),
        }
    );

    let second = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("error event must not hang on crash")
        .expect("DriverEvent::Error expected on EOF");
    match second {
        DriverEvent::Error {
            session_id,
            message,
        } => {
            assert_eq!(session_id.as_deref(), Some("sess-crash"));
            assert!(!message.is_empty());
        }
        other => panic!("expected DriverEvent::Error, got {other:?}"),
    }

    let end = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("stream should close after error");
    assert!(end.is_none());
    assert!(
        driver_turn
            .recent_stderr()
            .iter()
            .any(|line| line.contains("mid-turn crash stderr"))
    );
}

#[tokio::test]
async fn test_acp_driver_persistent_stream_receives_unsolicited_post_turn_updates() {
    let capture = TempCaptureFile::new("persistent_unsolicited");
    let script = r#"
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CAPTURE_FILE"
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-persist","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking-1"}}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-persist","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"turn1-hello"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-persist","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"unsolicited-msg"}}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-persist","update":{"sessionUpdate":"tool_call","toolCallId":"tc-bg","name":"run_command","title":"Background job","args":{"cmd":"echo hi"},"status":"running"}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-persist","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-bg","name":"run_command","status":"completed","rawOutput":{"output":"bg-ok"}}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-persist","update":{"sessionUpdate":"usage_update","used":320,"size":100000}}}\n'
            printf '{"jsonrpc":"2.0","id":"bg-perm-1","method":"session/request_permission","params":{"sessionId":"sess-persist","toolCall":{"toolCallId":"tc-bg-p","name":"write_file","title":"Write log","args":{"Path":"bg.log"}},"options":[{"optionId":"allow_once","name":"Allow Once","kind":"allow_once"}]}}\n'
            ;;
        *'"id":"bg-perm-1"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-persist","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"after-perm"}}}}\n'
            ;;
        *'"method":"session/close"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
            break
            ;;
    esac
done
sleep 1
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_env([("CAPTURE_FILE".to_string(), capture.path_string())]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = AcpStdioDriver::new(runner);

    let mut persistent_rx = driver.subscribe_session("sess-persist");

    driver
        .submit_prompt("sess-persist", "turn 1")
        .await
        .expect("submit_prompt should succeed");

    let mut received = Vec::new();
    for _ in 0..8 {
        let ev = timeout(Duration::from_secs(2), persistent_rx.recv())
            .await
            .expect("timed out waiting for persistent event")
            .expect("persistent receiver should stay open");
        if let DriverEvent::PermissionRequested { request_id, .. } = &ev {
            driver
                .respond_permission(request_id.clone(), Some("allow_once"))
                .await
                .expect("respond to unsolicited permission");
        }
        received.push(ev);
    }

    assert_eq!(
        received,
        vec![
            DriverEvent::ThoughtChunk {
                session_id: "sess-persist".into(),
                text: "thinking-1".into(),
            },
            DriverEvent::StreamChunk {
                session_id: "sess-persist".into(),
                text: "turn1-hello".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: "sess-persist".into(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
            DriverEvent::StreamChunk {
                session_id: "sess-persist".into(),
                text: "unsolicited-msg".into(),
            },
            DriverEvent::ToolCall {
                session_id: "sess-persist".into(),
                tool_call_id: "tc-bg".into(),
                name: "run_command".into(),
                title: "Background job".into(),
                args: serde_json::json!({"cmd": "echo hi"}),
                status: "running".into(),
            },
            DriverEvent::ToolCallUpdate {
                session_id: "sess-persist".into(),
                tool_call_id: "tc-bg".into(),
                name: "run_command".into(),
                status: "completed".into(),
                output: "bg-ok".into(),
            },
            DriverEvent::UsageUpdate {
                session_id: "sess-persist".into(),
                used: 320,
                size: 100000,
            },
            DriverEvent::PermissionRequested {
                request_id: serde_json::json!("bg-perm-1"),
                session_id: "sess-persist".into(),
                tool_call_id: "tc-bg-p".into(),
                tool_name: "write_file".into(),
                title: "Write log".into(),
                args: serde_json::json!({"Path": "bg.log"}),
                options: vec![AcpPermissionOption {
                    option_id: "allow_once".into(),
                    name: "Allow Once".into(),
                    kind: "allow_once".into(),
                }],
            },
        ]
    );

    let after_perm = timeout(Duration::from_secs(2), persistent_rx.recv())
        .await
        .expect("timed out waiting for after-perm chunk")
        .expect("persistent receiver should receive after permission reply");
    assert_eq!(
        after_perm,
        DriverEvent::StreamChunk {
            session_id: "sess-persist".into(),
            text: "after-perm".into(),
        }
    );

    driver
        .close_session("sess-persist")
        .await
        .expect("close_session should succeed");
    let closed = timeout(Duration::from_secs(2), persistent_rx.recv())
        .await
        .expect("closed persistent receiver should not hang");
    assert!(closed.is_none());

    driver.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_acp_driver_close_session_and_dual_dispatch_keep_send_prompt_compatible() {
    let capture = TempCaptureFile::new("close_and_compat");
    let script = r#"
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CAPTURE_FILE"
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"session/prompt"'*)
            case "$line" in
                *'"sessionId":"sess-p"'*)
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-p","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"persist-chunk"}}}}\n'
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
                    ;;
                *'"sessionId":"sess-both"'*)
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-both","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"shared-chunk"}}}}\n'
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
                    ;;
                *'"sessionId":"sess-legacy"'*)
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-legacy","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"legacy-chunk"}}}}\n'
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
                    ;;
            esac
            ;;
        *'"method":"session/close"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
            ;;
    esac
done
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()])
        .with_env([("CAPTURE_FILE".to_string(), capture.path_string())]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = AcpStdioDriver::new(runner);

    let mut rx_p = driver.subscribe_session("sess-p");
    driver
        .submit_prompt("sess-p", "hello")
        .await
        .expect("submit_prompt");

    assert_eq!(
        timeout(Duration::from_secs(2), rx_p.recv()).await.unwrap(),
        Some(DriverEvent::StreamChunk {
            session_id: "sess-p".into(),
            text: "persist-chunk".into(),
        })
    );
    assert_eq!(
        timeout(Duration::from_secs(2), rx_p.recv()).await.unwrap(),
        Some(DriverEvent::PromptCompleted {
            session_id: "sess-p".into(),
            stop_reason: "end_turn".into(),
            usage: None,
        })
    );

    driver
        .close_session("sess-p")
        .await
        .expect("close_session");
    assert_eq!(
        timeout(Duration::from_secs(2), rx_p.recv()).await.unwrap(),
        None
    );

    let mut rx_both_persistent = driver.subscribe_session("sess-both");
    let mut rx_both_turn = driver
        .send_prompt("sess-both", "dual")
        .await
        .expect("send_prompt dual");
    assert_eq!(
        timeout(Duration::from_secs(2), rx_both_turn.recv())
            .await
            .unwrap(),
        Some(DriverEvent::StreamChunk {
            session_id: "sess-both".into(),
            text: "shared-chunk".into(),
        })
    );
    assert_eq!(
        timeout(Duration::from_secs(2), rx_both_turn.recv())
            .await
            .unwrap(),
        Some(DriverEvent::PromptCompleted {
            session_id: "sess-both".into(),
            stop_reason: "end_turn".into(),
            usage: None,
        })
    );
    assert_eq!(
        timeout(Duration::from_secs(2), rx_both_turn.recv())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        timeout(Duration::from_secs(2), rx_both_persistent.recv())
            .await
            .unwrap(),
        Some(DriverEvent::StreamChunk {
            session_id: "sess-both".into(),
            text: "shared-chunk".into(),
        })
    );

    let mut rx_legacy = driver
        .send_prompt("sess-legacy", "legacy")
        .await
        .expect("send_prompt legacy");
    assert_eq!(
        timeout(Duration::from_secs(2), rx_legacy.recv())
            .await
            .unwrap(),
        Some(DriverEvent::StreamChunk {
            session_id: "sess-legacy".into(),
            text: "legacy-chunk".into(),
        })
    );
    assert_eq!(
        timeout(Duration::from_secs(2), rx_legacy.recv())
            .await
            .unwrap(),
        Some(DriverEvent::PromptCompleted {
            session_id: "sess-legacy".into(),
            stop_reason: "end_turn".into(),
            usage: None,
        })
    );
    assert_eq!(
        timeout(Duration::from_secs(2), rx_legacy.recv())
            .await
            .unwrap(),
        None
    );

    driver.shutdown().await.expect("shutdown");
    assert_eq!(
        timeout(Duration::from_secs(2), rx_both_persistent.recv())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn test_acp_driver_submit_prompt_error_resubscribe_and_eof_cleanup() {
    let script = r#"
turn=0
while IFS= read -r line; do
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"session/prompt"'*)
            turn=$((turn+1))
            if [ "$turn" -eq 1 ]; then
                printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"rate limited"}}\n' "$id"
            elif [ "$turn" -eq 2 ]; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-err","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"recovered"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            else
                exit 1
            fi
            ;;
    esac
done
"#;
    let config = StdioProcessConfig::new("sh", vec!["-c".to_string(), script.to_string()]);
    let runner = StdioProcessRunner::spawn(config).await.expect("spawn");
    let driver = AcpStdioDriver::new(runner);

    let mut old_rx = driver.subscribe_session("sess-err");
    let mut new_rx = driver.subscribe_session("sess-err");
    assert_eq!(
        timeout(Duration::from_secs(2), old_rx.recv()).await.unwrap(),
        None
    );

    let err = driver
        .submit_prompt("sess-err", "fail")
        .await
        .expect_err("JSON-RPC error must return Err");
    assert!(err.to_string().contains("rate limited"), "unexpected error: {err}");

    driver
        .submit_prompt("sess-err", "recover")
        .await
        .expect("persistent subscription must survive JSON-RPC error");

    assert_eq!(
        timeout(Duration::from_secs(2), new_rx.recv()).await.unwrap(),
        Some(DriverEvent::StreamChunk {
            session_id: "sess-err".into(),
            text: "recovered".into(),
        })
    );
    assert_eq!(
        timeout(Duration::from_secs(2), new_rx.recv()).await.unwrap(),
        Some(DriverEvent::PromptCompleted {
            session_id: "sess-err".into(),
            stop_reason: "end_turn".into(),
            usage: None,
        })
    );

    let crash_err = driver
        .submit_prompt("sess-err", "crash")
        .await
        .expect_err("process EOF must return Err");
    assert!(!crash_err.to_string().is_empty());

    assert_eq!(
        timeout(Duration::from_secs(2), new_rx.recv()).await.unwrap(),
        None
    );
}
