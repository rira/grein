use std::sync::Arc;
use std::time::Duration;

use grein_tui::engine_port::TuiEngineEvent;
use serde_json::{json, Value};
use tokio::time::timeout;

use super::super::{AcpBackendConfig, DaemonEngine, InProcessEngine, PermissionDecisionPayload};
use super::write_executable_script;

#[test]
fn test_acp_backend_config_discover_default_env_overrides_and_fallback() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin_dir = temp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("mkdir bin");
    let agy_path = write_executable_script(&bin_dir, "agy", "#!/bin/sh\nexit 0\n");
    let claude_path = write_executable_script(&bin_dir, "claude", "#!/bin/sh\nexit 0\n");
    let search_both = bin_dir.to_string_lossy().into_owned();

    let from_override = AcpBackendConfig::discover_default_from(
        temp.path().to_path_buf(),
        Some("/custom/agent".to_string()),
        Some("--acp --stdio".to_string()),
        Some(String::new()),
        Some(&search_both),
    )
    .expect("override config");
    assert_eq!(from_override.command, "/custom/agent");
    assert_eq!(from_override.args, vec!["--acp", "--stdio"]);
    assert_eq!(from_override.auth_method, None);
    assert_eq!(from_override.cwd, temp.path().to_path_buf());

    let from_override_auth = AcpBackendConfig::discover_default_from(
        temp.path().to_path_buf(),
        Some("/custom/agent".to_string()),
        None,
        Some("api-key".to_string()),
        Some(&search_both),
    )
    .expect("override with auth");
    assert!(from_override_auth.args.is_empty());
    assert_eq!(from_override_auth.auth_method.as_deref(), Some("api-key"));

    let disabled = AcpBackendConfig::discover_default_from(
        temp.path().to_path_buf(),
        Some(String::new()),
        None,
        None,
        Some(&search_both),
    );
    assert_eq!(disabled, None);

    let preferred_agy = AcpBackendConfig::discover_default_from(
        temp.path().to_path_buf(),
        None,
        None,
        None,
        Some(&search_both),
    )
    .expect("prefer agy");
    assert_eq!(preferred_agy.command, agy_path.to_string_lossy());
    assert_eq!(preferred_agy.args, vec!["acp", "--stdio"]);
    assert_eq!(preferred_agy.auth_method.as_deref(), Some("oauth-personal"));

    let only_claude_dir = temp.path().join("only_claude");
    std::fs::create_dir_all(&only_claude_dir).expect("mkdir claude");
    let only_claude =
        write_executable_script(&only_claude_dir, "claude", "#!/bin/sh\nexit 0\n");
    let fallback_claude = AcpBackendConfig::discover_default_from(
        temp.path().to_path_buf(),
        None,
        None,
        None,
        Some(&only_claude_dir.to_string_lossy()),
    )
    .expect("fallback claude");
    assert_eq!(fallback_claude.command, only_claude.to_string_lossy());
    assert_eq!(fallback_claude.args, vec!["--acp"]);
    assert_eq!(fallback_claude.auth_method, None);
    let _ = claude_path;

    let empty_dir = temp.path().join("empty_bin");
    std::fs::create_dir_all(&empty_dir).expect("mkdir empty");
    let none_found = AcpBackendConfig::discover_default_from(
        temp.path().to_path_buf(),
        None,
        None,
        None,
        Some(&empty_dir.to_string_lossy()),
    );
    assert_eq!(none_found, None);
}

#[tokio::test]
async fn test_send_turn_for_thread_without_acp_persists_user_and_errors_cleanly() {
    let engine = InProcessEngine::in_memory().expect("in_memory");
    let main = engine.ensure_main_thread().await.expect("ensure main");
    let mut rx = engine.subscribe_tui_events();

    let err = engine
        .send_turn_for_thread(&main.thread_id, "hello offline")
        .await
        .expect_err("no acp configured must error");
    assert!(!err.to_string().is_empty());

    let transcript = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("transcript");
    assert_eq!(transcript.len(), 1);
    assert_eq!(transcript[0].role, "user");
    assert_eq!(transcript[0].content, "hello offline");

    let mut seen = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        seen.push(ev);
    }
    assert!(seen.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadStatusChanged { thread_id, status }
            if thread_id == &main.thread_id && status == "streaming"
    )));
    assert!(seen.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadStatusChanged { thread_id, status }
            if thread_id == &main.thread_id && status == "error"
    )));
    assert!(seen.iter().any(|e| matches!(e, TuiEngineEvent::Error { .. })));
}

#[tokio::test]
async fn test_live_acp_streaming_resume_permissions_and_cancel() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");
    let capture_path = temp.path().join("acp_capture.jsonl");

    let mock_script = r#"
sess_counter=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    case "$line" in
        *'"method":"initialize"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock-agy","version":"1.0"},"authMethods":[{"id":"oauth-personal","name":"OAuth","description":"OAuth"}],"agentCapabilities":{"_meta":{"antigravity":{"systemPrompt":{"append":true}}}}}}\n' "$id"
            ;;
        *'"method":"authenticate"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            sess_counter=$((sess_counter+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s"}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/prompt"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            case "$line" in
                *'"sessionId":"sess-1"'*) sid="sess-1" ;;
                *'"sessionId":"sess-2"'*) sid="sess-2" ;;
                *) sid="sess-3" ;;
            esac
            if printf '%s' "$line" | grep -q "trigger-cancel"; then
                while IFS= read -r next_line; do
                    printf '%s\n' "$next_line" >> "$GREIN_CAPTURE"
                    if printf '%s' "$next_line" | grep -q '"method":"session/cancel"'; then
                        printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"cancelled"}}\n' "$id"
                        break
                    fi
                done
            elif printf '%s' "$line" | grep -q "trigger-perm"; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"tool_call","toolCallId":"tc-1","name":"run_command","title":"Run cargo","args":{"cmd":"cargo test"},"status":"pending"}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{"sessionId":"%s","toolCall":{"toolCallId":"tc-1","name":"run_command","title":"Run cargo","args":{"cmd":"cargo test"}},"options":[{"optionId":"allow_once","name":"Allow","kind":"allow_once"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]}}\n' "$sid"
                IFS= read -r perm_reply
                printf '%s\n' "$perm_reply" >> "$GREIN_CAPTURE"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-1","name":"run_command","status":"completed","rawOutput":{"output":"ok-tool"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello "}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"world!"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            else
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"session_info_update","title":"Auto Named Branch"}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Reply ok"}}}}\n' "$sid"
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#;
    let wrapper_script = format!(
        "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
        capture_path.to_string_lossy(),
        mock_script
    );
    let script_path = write_executable_script(temp.path(), "mock_acp.sh", &wrapper_script);

    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: Some("oauth-personal".to_string()),
        cwd: temp.path().to_path_buf(),
    };

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg.clone()))
            .expect("open"),
    );
    let main = engine.ensure_main_thread().await.expect("main");

    let mut rx = engine.subscribe_tui_events();
    let engine_perm = Arc::clone(&engine);
    let perm_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(ev) = rx.recv().await {
            if let TuiEngineEvent::PermissionRequested { ref request_id, .. } = ev {
                engine_perm
                    .resolve_permission(PermissionDecisionPayload {
                        request_id: request_id.clone(),
                        allow: true,
                    })
                    .await
                    .expect("resolve permission");
            }
            let is_done = matches!(
                ev,
                TuiEngineEvent::ThreadStatusChanged { ref status, .. } if status == "idle"
            );
            collected.push(ev);
            if is_done {
                break;
            }
        }
        collected
    });

    let turn1 = timeout(
        Duration::from_secs(5),
        engine.send_turn_for_thread(&main.thread_id, "trigger-perm first prompt"),
    )
    .await
    .expect("turn1 timeout")
    .expect("turn1 ok");
    assert!(!turn1.node_id.is_empty());

    let evs1 = timeout(Duration::from_secs(3), perm_task)
        .await
        .expect("events timeout")
        .expect("join");
    assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::ThoughtChunk { content, .. } if content == "thinking")));
    assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::ToolInvoked { tool_name, .. } if tool_name == "run_command")));
    assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::ToolResult { output, .. } if output == "ok-tool")));
    assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::StreamChunk { content, .. } if content == "Hello ")));
    assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::StreamChunk { content, .. } if content == "world!")));
    assert!(evs1.iter().any(|e| matches!(e, TuiEngineEvent::TurnCompleted { .. })));

    // Turn 2 reuses sess-1 cleanly without whiteboard delta prefix.
    timeout(
        Duration::from_secs(5),
        engine.send_turn_for_thread(&main.thread_id, "second prompt"),
    )
    .await
    .expect("turn2 timeout")
    .expect("turn2 ok");

    // Turn 3 reuses sess-1 cleanly.
    timeout(
        Duration::from_secs(5),
        engine.send_turn_for_thread(&main.thread_id, "third prompt"),
    )
    .await
    .expect("turn3 timeout")
    .expect("turn3 ok");

    // Fork generic branch -> gets auto-renamed by session_info_update & sees ancestor trunk.
    let branch_auto = engine
        .fork_thread_with_title(Some(&main.thread_id), None)
        .await
        .expect("fork auto");
    assert!(branch_auto.title.starts_with("Branch"));
    timeout(
        Duration::from_secs(5),
        engine.send_turn_for_thread(&branch_auto.thread_id, "branch auto prompt"),
    )
    .await
    .expect("branch auto timeout")
    .expect("branch auto ok");
    let threads_after_auto = engine.list_threads().await.expect("list");
    let updated_auto = threads_after_auto
        .iter()
        .find(|t| t.thread_id == branch_auto.thread_id)
        .expect("auto branch exists");
    assert_eq!(updated_auto.title, "Auto Named Branch");

    // Fork explicit named branch -> custom_name is preserved while ACP updates auto-title.
    let branch_named = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("explicit-title"))
        .await
        .expect("fork named");
    assert_eq!(branch_named.custom_name.as_deref(), Some("explicit-title"));
    timeout(
        Duration::from_secs(5),
        engine.send_turn_for_thread(&branch_named.thread_id, "branch named prompt"),
    )
    .await
    .expect("branch named timeout")
    .expect("branch named ok");
    let threads_after_named = engine.list_threads().await.expect("list");
    let preserved = threads_after_named
        .iter()
        .find(|t| t.thread_id == branch_named.thread_id)
        .expect("named branch exists");
    assert_eq!(preserved.custom_name.as_deref(), Some("explicit-title"));
    assert_eq!(preserved.title, "Auto Named Branch");

    // Cancel test on Main
    let engine_cancel = Arc::clone(&engine);
    let main_id = main.thread_id.clone();
    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        engine_cancel.cancel_turn(&main_id).await.expect("cancel ok");
    });
    timeout(
        Duration::from_secs(5),
        engine.send_turn_for_thread(&main.thread_id, "trigger-cancel now"),
    )
    .await
    .expect("cancel turn timeout")
    .expect("cancelled turn completes");
    cancel_handle.await.expect("cancel join");

    // Drop engine, reopen DB, verify resumed session passes Branch History in systemPrompt.append.
    drop(engine);
    let capture_resume = temp.path().join("acp_resume_capture.jsonl");
    let wrapper_resume = format!(
        "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
        capture_resume.to_string_lossy(),
        mock_script
    );
    let script_resume =
        write_executable_script(temp.path(), "mock_acp_resume.sh", &wrapper_resume);
    let cfg_resume = AcpBackendConfig {
        command: script_resume.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };
    let reopened = InProcessEngine::open_with_config(
        &db_path,
        temp.path().to_path_buf(),
        Some(cfg_resume),
    )
    .expect("reopen");
    timeout(
        Duration::from_secs(5),
        reopened.send_turn_for_thread(&main.thread_id, "resumed prompt"),
    )
    .await
    .expect("resume turn timeout")
    .expect("resume turn ok");

    // Verify captured ACP wire traffic
    let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
        .expect("read capture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("valid json"))
        .collect();

    assert_eq!(frames[0]["method"], "initialize");
    assert_eq!(frames[1]["method"], "authenticate");
    assert_eq!(frames[1]["params"]["methodId"], "oauth-personal");
    assert_eq!(frames[2]["method"], "session/new");
    let sys_append_1 = frames[2]["params"]["_meta"]["antigravity"]["systemPrompt"]["append"]
        .as_str()
        .unwrap_or("");
    assert!(!sys_append_1.contains("# Shared Blackboard Working Memory"));
    assert!(!sys_append_1.contains("trigger-perm first prompt"));

    let perm_reply = frames
        .iter()
        .find(|f| f.get("id") == Some(&json!("perm-1")))
        .expect("perm reply");
    assert_eq!(perm_reply["result"]["outcome"]["optionId"], "allow_once");

    let prompts: Vec<&str> = frames
        .iter()
        .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/prompt"))
        .map(|f| {
            f["params"]["prompt"][0]["text"]
                .as_str()
                .expect("prompt text")
        })
        .collect();
    assert!(!prompts[0].contains("[Shared Whiteboard Updated]"));
    assert_eq!(prompts[1], "second prompt");
    assert_eq!(prompts[2], "third prompt");

    assert!(frames
        .iter()
        .any(|f| f.get("method").and_then(Value::as_str) == Some("session/cancel")));

    let resume_frames: Vec<Value> = std::fs::read_to_string(&capture_resume)
        .expect("read resume capture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("valid json"))
        .collect();
    let resume_new = resume_frames
        .iter()
        .find(|f| f.get("method").and_then(Value::as_str) == Some("session/new"))
        .expect("resume session/new");
    let resume_append = resume_new["params"]["_meta"]["antigravity"]["systemPrompt"]["append"]
        .as_str()
        .expect("resume systemPrompt append");
    assert!(resume_append.contains("# Branch History"));
    assert!(resume_append.contains("Hello world!"));
}

#[tokio::test]
async fn test_acp_deny_permission_and_crash_recovery() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");
    let capture_path = temp.path().join("acp_deny_capture.jsonl");
    let flag_path = temp.path().join("crash_once.flag");
    std::fs::write(&flag_path, "1").expect("flag");

    let mock_script = r#"
if [ -f "$GREIN_CRASH_FLAG" ]; then
    rm -f "$GREIN_CRASH_FLAG"
    exit 1
fi
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    case "$line" in
        *'"method":"initialize"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-rec"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
            if printf '%s' "$line" | grep -q "deny-me"; then
                printf '{"jsonrpc":"2.0","id":"perm-deny","method":"session/request_permission","params":{"sessionId":"sess-rec","toolCall":{"toolCallId":"tc-d","name":"write_file","title":"Write"},"options":[{"optionId":"allow_once","name":"Allow","kind":"allow_once"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]}}\n'
                IFS= read -r perm_reply
                printf '%s\n' "$perm_reply" >> "$GREIN_CAPTURE"
            fi
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-rec","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
    let wrapper = format!(
        "#!/bin/sh\nexport GREIN_CAPTURE={:?}\nexport GREIN_CRASH_FLAG={:?}\n{}\n",
        capture_path.to_string_lossy(),
        flag_path.to_string_lossy(),
        mock_script
    );
    let script_path = write_executable_script(temp.path(), "mock_recover.sh", &wrapper);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open"),
    );
    let main = engine.ensure_main_thread().await.expect("main");

    // First call crashes on initialize -> error returned and driver reset cleanly.
    let crash_err = engine
        .send_turn_for_thread(&main.thread_id, "first fails")
        .await
        .expect_err("crash must fail");
    assert!(crash_err.to_string().contains("initialize"));

    // Second call recovers automatically, denies permission, and finishes cleanly.
    let mut rx = engine.subscribe_tui_events();
    let engine_deny = Arc::clone(&engine);
    let deny_task = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let TuiEngineEvent::PermissionRequested { request_id, .. } = ev {
                engine_deny
                    .resolve_permission(PermissionDecisionPayload {
                        request_id,
                        allow: false,
                    })
                    .await
                    .expect("deny");
                break;
            }
        }
    });
    engine
        .send_turn_for_thread(&main.thread_id, "deny-me")
        .await
        .expect("recovers cleanly");
    deny_task.await.expect("deny task");

    engine
        .send_turn_for_thread(&main.thread_id, "follow-up after recovery")
        .await
        .expect("follow-up");

    let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
        .expect("capture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let deny_reply = frames
        .iter()
        .find(|f| f.get("id") == Some(&json!("perm-deny")))
        .expect("deny reply");
    assert_eq!(deny_reply["result"]["outcome"]["optionId"], "reject_once");
    let last_prompt = frames
        .iter()
        .rfind(|f| f.get("method").and_then(Value::as_str) == Some("session/prompt"))
        .expect("last prompt");
    let text = last_prompt["params"]["prompt"][0]["text"]
        .as_str()
        .expect("text");
    assert!(!text.contains("[Shared Whiteboard Cleared]"));
    assert_eq!(text, "follow-up after recovery");
}

#[tokio::test]
async fn test_concurrent_send_turn_for_thread_serializes_without_listener_close() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("concurrent_turns.db");
    let capture_path = temp.path().join("concurrent_capture.jsonl");

    let mock_script = r#"
sess_counter=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            sess_counter=$((sess_counter+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s"}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/prompt"'*)
            if printf '%s' "$line" | grep -q "turn-one"; then
                sleep 0.12
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"reply-one"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            else
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"reply-two"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#;
    let wrapper = format!(
        "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
        capture_path.to_string_lossy(),
        mock_script
    );
    let script_path = write_executable_script(temp.path(), "mock_serial.sh", &wrapper);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open"),
    );
    let main = engine.ensure_main_thread().await.expect("main");

    let e1 = Arc::clone(&engine);
    let t1 = main.thread_id.clone();
    let e2 = Arc::clone(&engine);
    let t2 = main.thread_id.clone();

    let (res1, res2) = tokio::join!(
        async move { e1.send_turn_for_thread(&t1, "turn-one").await },
        async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            e2.send_turn_for_thread(&t2, "turn-two").await
        }
    );

    res1.expect("first turn must succeed");
    res2.expect("second overlapping turn must serialize and succeed");

    let transcript = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("transcript");
    let entries: Vec<(&str, &str)> = transcript
        .iter()
        .map(|t| (t.role.as_str(), t.content.as_str()))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("user", "turn-one"),
            ("assistant", "reply-one"),
            ("user", "turn-two"),
            ("assistant", "reply-two"),
        ]
    );

    let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
        .expect("capture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let session_new_count = frames
        .iter()
        .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/new"))
        .count();
    assert_eq!(session_new_count, 1, "must reuse single ACP session");
}
