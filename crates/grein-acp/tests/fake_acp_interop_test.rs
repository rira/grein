use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use grein_acp::{
    AcpPermissionOption, AcpStdioDriver, AcpUsage, AgentDriver, DriverEvent, StdioProcessConfig,
    StdioProcessRunner,
};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn fake_acp_script_path() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../grein-cli/tests/fake_acp_server.py");
    assert!(
        path.exists(),
        "fake_acp_server.py must exist at {}",
        path.display()
    );
    path
}

struct TempLogFile {
    path: PathBuf,
}

impl TempLogFile {
    fn new(label: &str) -> Self {
        let unique = format!(
            "grein-fake-acp-{}-{}-{}.jsonl",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
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
        let content = fs::read_to_string(&self.path).expect("log file should be created");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("valid JSON line in log"))
            .collect()
    }
}

impl Drop for TempLogFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

async fn collect_turn_events(
    rx: &mut mpsc::UnboundedReceiver<DriverEvent>,
) -> Vec<DriverEvent> {
    let mut events = Vec::new();
    while let Some(ev) = timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("timed out waiting for driver event")
    {
        let is_done = matches!(
            ev,
            DriverEvent::PromptCompleted { .. } | DriverEvent::Error { .. }
        );
        events.push(ev);
        if is_done {
            break;
        }
    }
    events
}

async fn spawn_fake_driver(
    extra_args: Vec<String>,
    env: Vec<(String, String)>,
) -> AcpStdioDriver {
    let script = fake_acp_script_path();
    let mut args = vec![script.to_string_lossy().into_owned()];
    args.extend(extra_args);
    let config = StdioProcessConfig::new("python3", args).with_env(env);
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("fake_acp_server.py must spawn cleanly via python3");
    AcpStdioDriver::new(runner)
}

#[tokio::test]
async fn test_fake_acp_initialize_auth_system_prompt_echo_and_default_turn() {
    let log = TempLogFile::new("basic");
    let driver = spawn_fake_driver(
        vec!["--log-file".to_string(), log.path_string()],
        Vec::new(),
    )
    .await;

    let init = driver
        .initialize()
        .await
        .expect("initialize should succeed");
    assert_eq!(init.protocol_version, 1);
    assert_eq!(init.agent_name.as_deref(), Some("Fake ACP Server"));
    assert_eq!(init.agent_version.as_deref(), Some("0.1.0"));
    assert!(init.supports_system_prompt_append);
    assert_eq!(init.auth_method_ids, vec!["oauth-personal"]);

    driver
        .authenticate("oauth-personal")
        .await
        .expect("authenticate should succeed");

    let sess = driver
        .new_session(Path::new("/tmp"), Some("SECRET_WB=42"))
        .await
        .expect("new_session should succeed");
    assert_eq!(sess.session_id, "sess-1");
    assert_eq!(sess.current_model_id.as_deref(), Some("fake-model"));
    assert_eq!(sess.current_mode_id.as_deref(), Some("default"));

    // Verify captured _meta.antigravity.systemPrompt.append
    let mut rx_dump = driver
        .send_prompt(&sess.session_id, "DUMP_SYSTEM_PROMPT")
        .await
        .expect("DUMP_SYSTEM_PROMPT should start");
    let dump_events = collect_turn_events(&mut rx_dump).await;
    assert_eq!(
        dump_events,
        vec![
            DriverEvent::StreamChunk {
                session_id: "sess-1".into(),
                text: "SECRET_WB=42".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: "sess-1".into(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    // Verify full prompt echo including [Shared Whiteboard Updated] delta prefix
    let prefixed_prompt =
        "[Shared Whiteboard Updated]\n- api_port: 8080\n\n---\n\nECHO_LAST_PROMPT";
    let mut rx_echo = driver
        .send_prompt(&sess.session_id, prefixed_prompt)
        .await
        .expect("ECHO_LAST_PROMPT should start");
    let echo_events = collect_turn_events(&mut rx_echo).await;
    assert_eq!(
        echo_events,
        vec![
            DriverEvent::StreamChunk {
                session_id: "sess-1".into(),
                text: prefixed_prompt.into(),
            },
            DriverEvent::PromptCompleted {
                session_id: "sess-1".into(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    // Verify default rich turn events (first default turn emits session_info_update)
    let mut rx_default = driver
        .send_prompt(&sess.session_id, "Hello fake server")
        .await
        .expect("default prompt should start");
    let default_events = collect_turn_events(&mut rx_default).await;
    assert_eq!(
        default_events,
        vec![
            DriverEvent::SessionInfoUpdate {
                session_id: "sess-1".into(),
                title: "Fake ACP Session".into(),
            },
            DriverEvent::ThoughtChunk {
                session_id: "sess-1".into(),
                text: "Thinking through request".into(),
            },
            DriverEvent::StreamChunk {
                session_id: "sess-1".into(),
                text: "Hello ".into(),
            },
            DriverEvent::StreamChunk {
                session_id: "sess-1".into(),
                text: "from Fake ACP".into(),
            },
            DriverEvent::UsageUpdate {
                session_id: "sess-1".into(),
                used: 135,
                size: 1000000,
            },
            DriverEvent::PromptCompleted {
                session_id: "sess-1".into(),
                stop_reason: "end_turn".into(),
                usage: Some(AcpUsage {
                    input_tokens: 100,
                    output_tokens: 25,
                    cached_read_tokens: 10,
                    cached_write_tokens: 0,
                    thought_tokens: 5,
                    total_tokens: 135,
                }),
            },
        ]
    );

    // Second default turn on same session must NOT repeat session_info_update
    let mut rx_second_default = driver
        .send_prompt(&sess.session_id, "Second default turn")
        .await
        .expect("second default prompt should start");
    let second_events = collect_turn_events(&mut rx_second_default).await;
    assert!(
        !second_events
            .iter()
            .any(|ev| matches!(ev, DriverEvent::SessionInfoUpdate { .. })),
        "session_info_update should only fire on first default turn: {second_events:?}"
    );
    assert!(second_events.iter().any(|ev| matches!(
        ev,
        DriverEvent::PromptCompleted {
            stop_reason,
            ..
        } if stop_reason == "end_turn"
    )));

    driver
        .close_session(&sess.session_id)
        .await
        .expect("close_session should succeed");
    driver.shutdown().await.expect("shutdown should succeed");

    let logged = log.read_json_lines();
    let methods: Vec<&str> = logged
        .iter()
        .filter_map(|v| v.get("method").and_then(|m| m.as_str()))
        .collect();
    assert_eq!(
        methods,
        vec![
            "initialize",
            "authenticate",
            "session/new",
            "session/prompt",
            "session/prompt",
            "session/prompt",
            "session/prompt",
            "session/close",
        ]
    );
}

#[tokio::test]
async fn test_fake_acp_concurrent_multiplexed_sessions_and_slow_cancel() {
    let driver = Arc::new(spawn_fake_driver(Vec::new(), Vec::new()).await);
    let agent: Arc<dyn AgentDriver> = driver.clone();

    agent.initialize().await.expect("init");
    agent.authenticate("oauth-personal").await.expect("auth");

    let s1 = agent
        .new_session(Path::new("/tmp"), Some("FIRST_SECRET=alpha"))
        .await
        .expect("session 1");
    let s2 = agent
        .new_session(Path::new("/tmp"), Some("SECOND_SECRET=beta"))
        .await
        .expect("session 2");
    assert_ne!(s1.session_id, s2.session_id);

    // Start slow stream on s1 and prompt on s2 concurrently; s2 must complete while s1 is mid-stream.
    let (mut rx1, mut rx2) = tokio::try_join!(
        agent.send_prompt(&s1.session_id, "STREAM_SLOW"),
        agent.send_prompt(&s2.session_id, "DUMP_SYSTEM_PROMPT")
    )
    .expect("concurrent prompts should start");

    let s2_events = collect_turn_events(&mut rx2).await;
    assert_eq!(
        s2_events,
        vec![
            DriverEvent::StreamChunk {
                session_id: s2.session_id.clone(),
                text: "SECOND_SECRET=beta".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: s2.session_id.clone(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    let first_chunk = timeout(Duration::from_secs(2), rx1.recv())
        .await
        .expect("timed out waiting for PART1")
        .expect("PART1 chunk");
    assert_eq!(
        first_chunk,
        DriverEvent::StreamChunk {
            session_id: s1.session_id.clone(),
            text: "PART1 ".into(),
        }
    );

    agent
        .cancel_session(&s1.session_id)
        .await
        .expect("cancel_session should succeed");

    let s1_rest = collect_turn_events(&mut rx1).await;
    assert_eq!(
        s1_rest,
        vec![DriverEvent::PromptCompleted {
            session_id: s1.session_id.clone(),
            stop_reason: "cancelled".into(),
            usage: None,
        }]
    );

    // Verify s1 can still serve a subsequent prompt after cancellation.
    let mut rx1_after = agent
        .send_prompt(&s1.session_id, "DUMP_SYSTEM_PROMPT")
        .await
        .expect("prompt after cancel");
    let s1_after = collect_turn_events(&mut rx1_after).await;
    assert_eq!(
        s1_after,
        vec![
            DriverEvent::StreamChunk {
                session_id: s1.session_id.clone(),
                text: "FIRST_SECRET=alpha".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: s1.session_id.clone(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    agent.close_session(&s1.session_id).await.expect("close s1");
    agent.close_session(&s2.session_id).await.expect("close s2");
    agent.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_fake_acp_stream_slow_completes_when_uncancelled() {
    let driver = spawn_fake_driver(
        Vec::new(),
        vec![("FAKE_ACP_SLOW_MS".to_string(), "120".to_string())],
    )
    .await;

    driver.initialize().await.expect("init");
    driver.authenticate("oauth-personal").await.expect("auth");
    let sess = driver
        .new_session(Path::new("/tmp"), None)
        .await
        .expect("new_session");

    let mut rx = driver
        .send_prompt(&sess.session_id, "STREAM_SLOW")
        .await
        .expect("send_prompt");
    let events = collect_turn_events(&mut rx).await;
    assert_eq!(
        events,
        vec![
            DriverEvent::StreamChunk {
                session_id: sess.session_id.clone(),
                text: "PART1 ".into(),
            },
            DriverEvent::StreamChunk {
                session_id: sess.session_id.clone(),
                text: "PART2".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: sess.session_id.clone(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    driver.close_session(&sess.session_id).await.expect("close");
    driver.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_fake_acp_permission_request_allow_deny_and_cancel() {
    let log = TempLogFile::new("perm_env");
    let driver = spawn_fake_driver(
        Vec::new(),
        vec![("FAKE_ACP_LOG".to_string(), log.path_string())],
    )
    .await;

    driver.initialize().await.expect("init");
    driver.authenticate("oauth-personal").await.expect("auth");
    let sess = driver
        .new_session(Path::new("/tmp"), None)
        .await
        .expect("new_session");

    // 1. Approve permission ("allow")
    let mut rx_allow = driver
        .send_prompt(&sess.session_id, "REQUIRE_PERMISSION")
        .await
        .expect("send_prompt allow");
    let perm_ev = timeout(Duration::from_secs(2), rx_allow.recv())
        .await
        .expect("perm timeout")
        .expect("perm event");
    match perm_ev {
        DriverEvent::PermissionRequested {
            request_id,
            session_id,
            tool_call_id,
            tool_name,
            title,
            args,
            options,
        } => {
            assert!(
                request_id.is_string(),
                "request_id must be a string, got {request_id:?}"
            );
            assert_eq!(request_id, json!("perm-1"));
            assert_eq!(session_id, sess.session_id);
            assert_eq!(tool_call_id, "call-1");
            assert_eq!(tool_name, "run_command");
            assert_eq!(title, "echo ok");
            assert_eq!(args, json!({}));
            assert_eq!(
                options,
                vec![
                    AcpPermissionOption {
                        option_id: "allow".into(),
                        name: "Allow".into(),
                        kind: "allow_once".into(),
                    },
                    AcpPermissionOption {
                        option_id: "deny".into(),
                        name: "Deny".into(),
                        kind: "reject_once".into(),
                    },
                ]
            );
            driver
                .respond_permission(request_id, Some("allow"))
                .await
                .expect("respond allow");
        }
        other => panic!("expected PermissionRequested, got {other:?}"),
    }
    let allow_rest = collect_turn_events(&mut rx_allow).await;
    assert_eq!(
        allow_rest,
        vec![
            DriverEvent::ToolCall {
                session_id: sess.session_id.clone(),
                tool_call_id: "call-1".into(),
                name: "run_command".into(),
                title: "echo ok".into(),
                args: json!({}),
                status: "pending".into(),
            },
            DriverEvent::ToolCallUpdate {
                session_id: sess.session_id.clone(),
                tool_call_id: "call-1".into(),
                name: "run_command".into(),
                status: "completed".into(),
                output: "ok".into(),
            },
            DriverEvent::StreamChunk {
                session_id: sess.session_id.clone(),
                text: "PERMISSION:allow".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: sess.session_id.clone(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    // 2. Deny permission ("deny")
    let mut rx_deny = driver
        .send_prompt(&sess.session_id, "REQUIRE_PERMISSION")
        .await
        .expect("send_prompt deny");
    let perm_ev2 = timeout(Duration::from_secs(2), rx_deny.recv())
        .await
        .expect("perm2 timeout")
        .expect("perm2 event");
    match perm_ev2 {
        DriverEvent::PermissionRequested { request_id, .. } => {
            assert_eq!(request_id, json!("perm-2"));
            driver
                .respond_permission(request_id, Some("deny"))
                .await
                .expect("respond deny");
        }
        other => panic!("expected PermissionRequested, got {other:?}"),
    }
    let deny_rest = collect_turn_events(&mut rx_deny).await;
    assert_eq!(
        deny_rest,
        vec![
            DriverEvent::ToolCall {
                session_id: sess.session_id.clone(),
                tool_call_id: "call-1".into(),
                name: "run_command".into(),
                title: "echo ok".into(),
                args: json!({}),
                status: "pending".into(),
            },
            DriverEvent::ToolCallUpdate {
                session_id: sess.session_id.clone(),
                tool_call_id: "call-1".into(),
                name: "run_command".into(),
                status: "failed".into(),
                output: "deny".into(),
            },
            DriverEvent::StreamChunk {
                session_id: sess.session_id.clone(),
                text: "PERMISSION:deny".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: sess.session_id.clone(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    // 3. Cancel permission (None)
    let mut rx_cancel = driver
        .send_prompt(&sess.session_id, "REQUIRE_PERMISSION")
        .await
        .expect("send_prompt cancel perm");
    let perm_ev3 = timeout(Duration::from_secs(2), rx_cancel.recv())
        .await
        .expect("perm3 timeout")
        .expect("perm3 event");
    match perm_ev3 {
        DriverEvent::PermissionRequested { request_id, .. } => {
            assert_eq!(request_id, json!("perm-3"));
            driver
                .respond_permission(request_id, None)
                .await
                .expect("respond cancelled");
        }
        other => panic!("expected PermissionRequested, got {other:?}"),
    }
    let cancel_rest = collect_turn_events(&mut rx_cancel).await;
    assert!(cancel_rest.iter().any(|ev| {
        matches!(
            ev,
            DriverEvent::StreamChunk { text, .. } if text == "PERMISSION:cancelled"
        )
    }));

    driver.close_session(&sess.session_id).await.expect("close");
    driver.shutdown().await.expect("shutdown");

    let logged = log.read_json_lines();
    assert!(
        logged.iter().any(|v| v.get("id") == Some(&json!("perm-1"))),
        "FAKE_ACP_LOG env var should log permission response frame"
    );
}

#[tokio::test]
async fn test_fake_acp_unsolicited_subagent_wakeup_over_persistent_stream() {
    let driver = spawn_fake_driver(Vec::new(), Vec::new()).await;

    driver.initialize().await.expect("init");
    driver.authenticate("oauth-personal").await.expect("auth");
    let sess = driver
        .new_session(Path::new("/tmp"), None)
        .await
        .expect("new_session");

    let mut rx = driver.subscribe_session(&sess.session_id);
    driver
        .submit_prompt(&sess.session_id, "SPAWN_SUBAGENT_WAKEUP")
        .await
        .expect("submit_prompt SPAWN_SUBAGENT_WAKEUP");

    let initial_events = collect_turn_events(&mut rx).await;
    assert_eq!(
        initial_events,
        vec![
            DriverEvent::ToolCall {
                session_id: sess.session_id.clone(),
                tool_call_id: "tc-subagent-1".into(),
                name: "invoke_subagent".into(),
                title: "Launching background subagent".into(),
                args: json!({
                    "Role": "Background Code Checker",
                    "TypeName": "research",
                    "Prompt": "check codebase",
                }),
                status: "in_progress".into(),
            },
            DriverEvent::ToolCallUpdate {
                session_id: sess.session_id.clone(),
                tool_call_id: "tc-subagent-1".into(),
                name: String::new(),
                status: "completed".into(),
                output: String::new(),
            },
            DriverEvent::StreamChunk {
                session_id: sess.session_id.clone(),
                text: "Spawned background subagent.".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: sess.session_id.clone(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    let wakeup_chunk = timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("timed out waiting for unsolicited wakeup chunk")
        .expect("persistent receiver should stay open for unsolicited wakeup");
    match &wakeup_chunk {
        DriverEvent::StreamChunk { session_id, text } => {
            assert_eq!(session_id, &sess.session_id);
            assert!(
                text.contains("WAKEUP_OK"),
                "expected WAKEUP_OK in unsolicited chunk, got: {text:?}"
            );
        }
        other => panic!("expected unsolicited StreamChunk, got {other:?}"),
    }

    let wakeup_usage = timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("timed out waiting for trailing usage_update")
        .expect("persistent receiver should receive trailing usage_update");
    assert_eq!(
        wakeup_usage,
        DriverEvent::UsageUpdate {
            session_id: sess.session_id.clone(),
            used: 180,
            size: 1000000,
        }
    );

    driver
        .submit_prompt(&sess.session_id, "ECHO_LAST_PROMPT after wakeup")
        .await
        .expect("follow-up submit_prompt should succeed while wakeup fired");
    let follow_up_events = collect_turn_events(&mut rx).await;
    assert_eq!(
        follow_up_events,
        vec![
            DriverEvent::StreamChunk {
                session_id: sess.session_id.clone(),
                text: "ECHO_LAST_PROMPT after wakeup".into(),
            },
            DriverEvent::PromptCompleted {
                session_id: sess.session_id.clone(),
                stop_reason: "end_turn".into(),
                usage: None,
            },
        ]
    );

    driver
        .close_session(&sess.session_id)
        .await
        .expect("close_session should succeed");
    let closed = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("closed persistent receiver should not hang");
    assert!(closed.is_none());
    driver.shutdown().await.expect("shutdown should succeed");
}
