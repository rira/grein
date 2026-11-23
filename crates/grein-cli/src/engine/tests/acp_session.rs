use std::time::Duration;

use grein_acp::{AcpConfigChoice, AcpConfigOption, AcpModeOption, AcpModelOption, AcpSessionInfo};
use grein_tui::engine_port::{SubagentStatus, TuiEngineEvent};
use serde_json::json;
use tokio::time::timeout;

use super::super::types::{extract_tool_call_summary, normalize_session_config_options};
use super::super::{AcpBackendConfig, DaemonEngine, InProcessEngine};
use super::write_executable_script;

#[tokio::test]
async fn test_persistent_acp_unsolicited_wakeups_subagents_and_agent_meta() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");

    let mock_script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-bg","models":{"currentModelId":"gemini-2.5-pro"},"modes":{"currentModeId":"code"}}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            if printf '%s' "$line" | grep -q "launch-sub"; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call","toolCallId":"sa-1","name":"invoke_subagent","title":"Researcher","args":{"Role":"Codebase Researcher","TypeName":"research"},"status":"pending"}}}\n'
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call_update","toolCallId":"sa-1","name":"invoke_subagent","status":"completed","rawOutput":{"output":"spawned"}}}}\n'
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Launched"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
                (
                    sleep 0.08
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Wakeup 1 result"}}}}\n'
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"usage_update","used":50,"size":1000}}}\n'
                ) &
            elif printf '%s' "$line" | grep -q "quiet-wake"; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Ack"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
                (
                    sleep 0.06
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Wakeup quiet"}}}}\n'
                ) &
            else
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call","toolCallId":"sa-fail","name":"invoke_subagent","title":"Failing","args":{"Role":"Fail Worker","TypeName":"research"},"status":"pending"}}}\n'
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-bg","update":{"sessionUpdate":"tool_call_update","toolCallId":"sa-fail","name":"invoke_subagent","status":"failed","rawOutput":{"output":"crashed"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#;
    let script_path = write_executable_script(temp.path(), "mock_bg_acp.sh", mock_script);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open");
    engine.set_test_subagent_brain(temp.path().join("missing-brain"));
    engine.set_test_quiet_timeout(Duration::from_millis(90));
    let main = engine.ensure_main_thread().await.expect("main");
    let mut rx = engine.subscribe_tui_events();

    // Turn 1: launches subagent, completes turn, then unsolicited UsageUpdate wakeup arrives.
    engine
        .send_turn_for_thread(&main.thread_id, "launch-sub")
        .await
        .expect("turn 1");

    // Wait until unsolicited wakeup completes & subagent auto-removes (50ms test removal delay).
    let mut evs = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some(ev)) => {
                let removed = matches!(ev, TuiEngineEvent::SubagentRemoved { .. });
                evs.push(ev);
                if removed {
                    break;
                }
            }
            _ => break,
        }
    }

    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::AgentMetaUpdated { thread_id, model, mode }
            if thread_id == &main.thread_id
                && model.as_deref() == Some("gemini-2.5-pro")
                && mode.as_deref() == Some("code")
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolActivityUpdated { tool_name: Some(name), .. } if name == "invoke_subagent"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolActivityUpdated { tool_name: None, .. }
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::SubagentUpdated { subagent, .. }
            if subagent.id == "sa-1"
                && subagent.role == "Codebase Researcher"
                && subagent.agent_type == "research"
                && subagent.status == SubagentStatus::Running
                && subagent.activity.as_deref() == Some("background active")
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::SubagentUpdated { subagent, .. }
            if subagent.id == "sa-1" && subagent.status == SubagentStatus::Completed
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::SubagentRemoved { subagent_id, .. } if subagent_id == "sa-1"
    )));

    let transcript1 = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("transcript1");
    assert_eq!(
        transcript1.last().map(|t| t.content.as_str()),
        Some("Wakeup 1 result")
    );

    // Turn 2: unsolicited wakeup without usage_update -> finalized by quiet timer.
    engine
        .send_turn_for_thread(&main.thread_id, "quiet-wake")
        .await
        .expect("turn 2");

    let mut saw_quiet_completed = false;
    let deadline2 = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline2 {
        match timeout(Duration::from_millis(250), rx.recv()).await {
            Ok(Some(TuiEngineEvent::TurnCompleted { .. })) => {
                let t = engine
                    .get_thread_transcript(&main.thread_id)
                    .await
                    .expect("t2");
                if t.last().is_some_and(|m| m.content == "Wakeup quiet") {
                    saw_quiet_completed = true;
                    break;
                }
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    assert!(saw_quiet_completed, "quiet timer must finalize unsolicited wakeup");

    // Turn 3: failed invoke_subagent tool_call_update -> Failed & removed.
    engine
        .send_turn_for_thread(&main.thread_id, "fail-sub")
        .await
        .expect("turn 3");
    let mut fail_evs = Vec::new();
    while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
        let removed = matches!(
            ev,
            TuiEngineEvent::SubagentRemoved { ref subagent_id, .. } if subagent_id == "sa-fail"
        );
        fail_evs.push(ev);
        if removed {
            break;
        }
    }
    assert!(fail_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::SubagentUpdated { subagent, .. }
            if subagent.id == "sa-fail" && subagent.status == SubagentStatus::Failed
    )));
}

#[test]
fn test_extract_tool_call_summary_variants() {
    assert_eq!(
        extract_tool_call_summary("run_command", "Run   tests\nnow", &json!({"cmd": "cargo test"})),
        Some("Run tests now".to_string())
    );
    assert_eq!(
        extract_tool_call_summary(
            "run_command",
            "run_command",
            &json!({"CommandLine": "cargo test --locked"})
        ),
        Some("cargo test --locked".to_string())
    );
    assert_eq!(
        extract_tool_call_summary("read_file", "", &json!({"TargetFile": "src/main.rs"})),
        Some("src/main.rs".to_string())
    );
    assert_eq!(
        extract_tool_call_summary("search", "", &json!({"Query": "fn route"})),
        Some("fn route".to_string())
    );
    assert_eq!(extract_tool_call_summary("noop", "", &json!({})), None);

    let long_cmd = "x".repeat(120);
    let truncated = extract_tool_call_summary("run_command", "", &json!({"command": long_cmd}))
        .expect("truncated");
    assert_eq!(truncated.chars().count(), 80);
    assert!(truncated.ends_with("..."));
}

#[test]
fn test_normalize_session_config_options_precedence_and_fallback() {
    // 1. Prefer config_options when non-empty with model/mode
    let full = AcpSessionInfo {
        session_id: "s1".into(),
        current_model_id: Some("legacy-model".into()),
        current_mode_id: Some("legacy-mode".into()),
        available_models: vec![AcpModelOption {
            model_id: "legacy-model".into(),
            name: "Legacy".into(),
            description: None,
        }],
        available_modes: vec![AcpModeOption {
            id: "legacy-mode".into(),
            name: "Legacy Mode".into(),
            description: None,
        }],
        config_options: vec![
            AcpConfigOption {
                id: "model".into(),
                name: "Model".into(),
                category: Some("model".into()),
                current_value: "modern-model".into(),
                options: vec![AcpConfigChoice {
                    value: "modern-model".into(),
                    name: "Modern".into(),
                    description: None,
                }],
            },
            AcpConfigOption {
                id: "mode".into(),
                name: "Mode".into(),
                category: Some("mode".into()),
                current_value: "modern-mode".into(),
                options: vec![AcpConfigChoice {
                    value: "modern-mode".into(),
                    name: "Modern Mode".into(),
                    description: None,
                }],
            },
        ],
    };
    let norm_full = normalize_session_config_options(&full);
    assert_eq!(norm_full.len(), 2);
    assert_eq!(norm_full[0].current_value, "modern-model");
    assert_eq!(norm_full[1].current_value, "modern-mode");

    // 2. Fallback synthesis from available_models / available_modes when config_options empty
    let legacy = AcpSessionInfo {
        session_id: "s2".into(),
        current_model_id: Some("m2".into()),
        current_mode_id: Some("code".into()),
        available_models: vec![
            AcpModelOption {
                model_id: "m1".into(),
                name: "M1".into(),
                description: None,
            },
            AcpModelOption {
                model_id: "m2".into(),
                name: "M2".into(),
                description: Some("Fast".into()),
            },
        ],
        available_modes: vec![AcpModeOption {
            id: "code".into(),
            name: "Code".into(),
            description: None,
        }],
        config_options: Vec::new(),
    };
    let norm_legacy = normalize_session_config_options(&legacy);
    assert_eq!(norm_legacy.len(), 2);
    assert_eq!(norm_legacy[0].id, "model");
    assert_eq!(norm_legacy[0].current_value, "m2");
    assert_eq!(norm_legacy[0].options.len(), 2);
    assert_eq!(norm_legacy[1].id, "mode");
    assert_eq!(norm_legacy[1].current_value, "code");

    // 3. Hybrid: non-empty config_options without model/mode still synthesizes missing model/mode
    let hybrid = AcpSessionInfo {
        session_id: "s3".into(),
        current_model_id: Some("m1".into()),
        current_mode_id: None,
        available_models: vec![AcpModelOption {
            model_id: "m1".into(),
            name: "M1".into(),
            description: None,
        }],
        available_modes: Vec::new(),
        config_options: vec![AcpConfigOption {
            id: "thought_level".into(),
            name: "Thinking".into(),
            category: Some("thought_level".into()),
            current_value: "high".into(),
            options: Vec::new(),
        }],
    };
    let norm_hybrid = normalize_session_config_options(&hybrid);
    assert_eq!(norm_hybrid.len(), 2);
    assert!(norm_hybrid.iter().any(|o| o.id == "thought_level"));
    assert!(norm_hybrid.iter().any(|o| o.id == "model" && o.current_value == "m1"));
}

#[tokio::test]
async fn test_live_acp_config_updates_and_commands_via_session_listener() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("live_updates.db");

    let mock_script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-u","models":{"currentModelId":"m1","availableModels":[{"modelId":"m1","name":"M1"},{"modelId":"m2","name":"M2"}]},"modes":{"currentModeId":"default","availableModes":[{"id":"default","name":"Default"},{"id":"plan","name":"Plan"}]}}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compact context"}]}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"current_mode_update","currentModeId":"plan"}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"thought_level","name":"Thinking","category":"thought_level","currentValue":"high","options":[{"value":"high","name":"High"}]}]}}}\n'
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-u","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"updated"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
    let script_path = write_executable_script(temp.path(), "mock_updates.sh", mock_script);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    let mut rx = engine.subscribe_tui_events();

    engine
        .send_turn_for_thread(&main.thread_id, "trigger updates")
        .await
        .expect("turn");

    let mut evs = Vec::new();
    while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
        let done = matches!(ev, TuiEngineEvent::ThreadStatusChanged { ref status, .. } if status == "idle");
        evs.push(ev);
        if done {
            break;
        }
    }

    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::AgentCommandsUpdated { thread_id, commands }
            if thread_id == &main.thread_id
                && commands.len() == 1
                && commands[0].name == "compact"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::AgentMetaUpdated { thread_id, mode: Some(mode), .. }
            if thread_id == &main.thread_id && mode == "plan"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::AgentConfigUpdated { thread_id, config_options }
            if thread_id == &main.thread_id
                && config_options.iter().any(|o| o.id == "thought_level" && o.current_value == "high")
                && config_options.iter().any(|o| o.id == "model" && o.current_value == "m1")
                && config_options.iter().any(|o| o.id == "mode" && o.current_value == "plan")
    )));

    let metas = engine.agent_metadata().await.expect("agent_metadata");
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].mode.as_deref(), Some("plan"));
    assert_eq!(metas[0].available_commands.len(), 1);
    assert!(metas[0].config_options.iter().any(|o| o.id == "thought_level"));
}

#[tokio::test]
async fn test_unconditional_acp_session_info_update_updates_titles_and_preserves_custom_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session_info_unconditional.db");

    let mock_script = r#"#!/bin/sh
sess_counter=0
while IFS= read -r line; do
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
            sid=${line#*\"sessionId\":\"}
            sid=${sid%%\"*}
            info_title=""
            case "$line" in
                *"main turn 1"*) info_title="Auth System Overview" ;;
                *"main turn 2"*) info_title="Refined Auth Architecture" ;;
                *"branch auto turn"*) info_title="Token Rotation Plan" ;;
                *"custom branch turn"*) info_title="OAuth PKCE Flow" ;;
                *"root turn"*) info_title="Root Auto Title" ;;
            esac
            if [ -n "$info_title" ]; then
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"session_info_update","title":"%s"}}}\n' "$sid" "$info_title"
            fi
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ok"}}}}\n' "$sid"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
    let script_path = write_executable_script(temp.path(), "mock_info_update.sh", mock_script);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    let branch_auto = engine
        .fork_thread_with_title(Some(&main.thread_id), None)
        .await
        .expect("fork auto");
    let branch_custom = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("Auth"))
        .await
        .expect("fork custom");
    let root2 = engine.create_root_thread(None).await.expect("create root2");

    let mut rx = engine.subscribe_tui_events();

    engine
        .send_turn_for_thread(&main.thread_id, "main turn 1")
        .await
        .expect("main turn 1");
    engine
        .send_turn_for_thread(&main.thread_id, "main turn 2")
        .await
        .expect("main turn 2");
    engine
        .send_turn_for_thread(&branch_auto.thread_id, "branch auto turn")
        .await
        .expect("branch auto turn");
    engine
        .send_turn_for_thread(&branch_custom.thread_id, "custom branch turn")
        .await
        .expect("custom branch turn");
    engine
        .send_turn_for_thread(&root2.thread_id, "root turn")
        .await
        .expect("root turn");

    let mut events = Vec::new();
    while let Ok(Some(ev)) = timeout(Duration::from_millis(120), rx.recv()).await {
        events.push(ev);
    }

    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadTitleUpdated { thread_id, title }
            if thread_id == &main.thread_id && title == "Auth System Overview"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadTitleUpdated { thread_id, title }
            if thread_id == &main.thread_id && title == "Refined Auth Architecture"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadTitleUpdated { thread_id, title }
            if thread_id == &branch_auto.thread_id && title == "Token Rotation Plan"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadTitleUpdated { thread_id, title }
            if thread_id == &branch_custom.thread_id && title == "OAuth PKCE Flow"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadTitleUpdated { thread_id, title }
            if thread_id == &root2.thread_id && title == "Root Auto Title"
    )));
    assert!(!events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::ThreadCustomNameUpdated { .. })));

    let threads = engine.list_threads().await.expect("list");
    let m_summary = threads.iter().find(|t| t.thread_id == main.thread_id).unwrap();
    assert_eq!(m_summary.title, "Refined Auth Architecture");
    assert_eq!(m_summary.custom_name, None);

    let ba_summary = threads
        .iter()
        .find(|t| t.thread_id == branch_auto.thread_id)
        .unwrap();
    assert_eq!(ba_summary.title, "Token Rotation Plan");
    assert_eq!(ba_summary.custom_name, None);

    let bc_summary = threads
        .iter()
        .find(|t| t.thread_id == branch_custom.thread_id)
        .unwrap();
    assert_eq!(bc_summary.title, "OAuth PKCE Flow");
    assert_eq!(bc_summary.custom_name.as_deref(), Some("Auth"));

    let r2_summary = threads.iter().find(|t| t.thread_id == root2.thread_id).unwrap();
    assert_eq!(r2_summary.title, "Root Auto Title");
    assert_eq!(r2_summary.custom_name, None);

    // Reopen DB and verify persisted titles & custom_name.
    drop(engine);
    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("reopen");
    let persisted = reopened.list_threads().await.expect("persisted");
    let bc_persisted = persisted
        .iter()
        .find(|t| t.thread_id == branch_custom.thread_id)
        .unwrap();
    assert_eq!(bc_persisted.title, "OAuth PKCE Flow");
    assert_eq!(bc_persisted.custom_name.as_deref(), Some("Auth"));
}
