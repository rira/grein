use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use grein_cli::engine::{
    AcpBackendConfig, EngineError, InProcessEngine, PermissionDecisionPayload,
};
use grein_core::blackboard::{BlackboardEntry, BlackboardEntryKind};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::engine_port::{SubagentStatus, TuiEngineEvent};
use serde_json::Value;
use tokio::time::timeout;

fn fake_acp_script_path() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_acp_server.py");
    assert!(
        path.exists(),
        "fake_acp_server.py must exist at {}",
        path.display()
    );
    path
}

fn fake_acp_config(cwd: &Path) -> AcpBackendConfig {
    AcpBackendConfig {
        command: "python3".to_string(),
        args: vec![fake_acp_script_path().to_string_lossy().into_owned()],
        auth_method: Some("oauth-personal".to_string()),
        cwd: cwd.to_path_buf(),
    }
}

fn fake_acp_config_with_log(cwd: &Path, log_path: &Path) -> AcpBackendConfig {
    AcpBackendConfig {
        command: "python3".to_string(),
        args: vec![
            fake_acp_script_path().to_string_lossy().into_owned(),
            "--log-file".to_string(),
            log_path.to_string_lossy().into_owned(),
        ],
        auth_method: Some("oauth-personal".to_string()),
        cwd: cwd.to_path_buf(),
    }
}

#[tokio::test]
async fn test_e2e_hydration_across_restart_streaming_branch_isolation_and_legacy_db_compat() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");

    // Seed legacy blackboard_entries rows before opening engine.
    {
        let legacy_storage = SqliteStorage::open(&db_path).expect("open legacy sqlite");
        legacy_storage
            .insert_blackboard_entry(&BlackboardEntry::new(
                "architecture",
                "two-tier-dag",
                BlackboardEntryKind::Decision,
            ))
            .expect("insert legacy row");
    }

    let cfg = fake_acp_config(temp.path());

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg.clone()))
            .expect("open engine"),
    );

    let main = engine.ensure_main_thread().await.expect("ensure main");
    assert_eq!(main.title, "Main");

    let mut rx = engine.subscribe_tui_events();
    let collect_main = tokio::spawn(async move {
        let mut chunks = Vec::new();
        let mut saw_streaming = false;
        let mut saw_completed = false;
        while let Ok(Some(ev)) = timeout(Duration::from_secs(5), rx.recv()).await {
            match ev {
                TuiEngineEvent::ThreadStatusChanged { ref status, .. } if status == "streaming" => {
                    saw_streaming = true;
                }
                TuiEngineEvent::StreamChunk { content, .. } => {
                    chunks.push(content);
                }
                TuiEngineEvent::TurnCompleted { .. } => {
                    saw_completed = true;
                }
                TuiEngineEvent::ThreadStatusChanged { ref status, .. } if status == "idle" => {
                    break;
                }
                _ => {}
            }
        }
        (saw_streaming, saw_completed, chunks.join(""))
    });

    engine
        .send_turn_for_thread(&main.thread_id, "main-trunk-anchor-secret")
        .await
        .expect("main turn");

    let (saw_streaming, saw_completed, streamed_text) =
        collect_main.await.expect("collect main task");
    assert!(saw_streaming);
    assert!(saw_completed);
    assert_eq!(streamed_text, "Hello from Fake ACP");

    // Fork two sibling branches from Main.
    let branch_a = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("branch-a"))
        .await
        .expect("fork branch-a");
    let branch_b = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("branch-b"))
        .await
        .expect("fork branch-b");

    // Send a sibling-only message in branch_b first.
    engine
        .send_turn_for_thread(&branch_b.thread_id, "sibling-only-banana")
        .await
        .expect("branch_b turn");

    // First turn in branch_a inspects the compiled _meta.antigravity.systemPrompt.append.
    engine
        .send_turn_for_thread(&branch_a.thread_id, "DUMP_SYSTEM_PROMPT")
        .await
        .expect("branch_a dump system prompt");

    let branch_a_transcript = engine
        .get_thread_transcript(&branch_a.thread_id)
        .await
        .expect("branch_a transcript");
    let dumped_prompt = branch_a_transcript
        .last()
        .expect("assistant reply")
        .content
        .clone();
    assert!(
        !dumped_prompt.contains("two-tier-dag"),
        "legacy blackboard_entries must be ignored in systemPrompt.append, got: {dumped_prompt}"
    );
    assert!(
        !dumped_prompt.contains("# Shared Blackboard Working Memory"),
        "systemPrompt.append must not inject Shared Blackboard section, got: {dumped_prompt}"
    );
    assert!(
        dumped_prompt.contains("main-trunk-anchor-secret"),
        "expected ancestor trunk turn in systemPrompt.append, got: {dumped_prompt}"
    );
    assert!(
        !dumped_prompt.contains("sibling-only-banana"),
        "branch_a must be isolated from sibling branch_b, got: {dumped_prompt}"
    );

    engine
        .send_turn_for_thread(&branch_a.thread_id, "ECHO_LAST_PROMPT follow-up")
        .await
        .expect("branch_a echo last prompt");

    let branch_a_after_echo = engine
        .get_thread_transcript(&branch_a.thread_id)
        .await
        .expect("branch_a transcript after echo");
    let echoed_prompt = &branch_a_after_echo
        .last()
        .expect("echoed assistant reply")
        .content;
    assert_eq!(echoed_prompt, "ECHO_LAST_PROMPT follow-up");

    // Close DB and reopen engine -> verify SQLite hydration across restart.
    drop(engine);
    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("reopen engine");

    let hydrated_threads = reopened.list_threads().await.expect("list threads");
    assert_eq!(hydrated_threads.len(), 3);
    let custom_names: Vec<Option<&str>> = hydrated_threads
        .iter()
        .map(|t| t.custom_name.as_deref())
        .collect();
    assert!(custom_names.contains(&Some("branch-a")));
    assert!(custom_names.contains(&Some("branch-b")));

    let persisted_a = reopened
        .get_thread_transcript(&branch_a.thread_id)
        .await
        .expect("reopened branch_a transcript");
    assert!(persisted_a
        .iter()
        .any(|t| t.role == "user" && t.content == "DUMP_SYSTEM_PROMPT"));
    assert!(persisted_a
        .iter()
        .any(|t| t.role == "assistant" && t.content == "ECHO_LAST_PROMPT follow-up"));
}

#[tokio::test]
async fn test_e2e_permissions_approve_deny_cancel_and_shell_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");
    let cfg = fake_acp_config(temp.path());

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open engine"),
    );
    let main = engine.ensure_main_thread().await.expect("ensure main");

    // 1. Permission Approve
    let mut rx_allow = engine.subscribe_tui_events();
    let engine_allow = Arc::clone(&engine);
    let allow_task = tokio::spawn(async move {
        while let Ok(Some(ev)) = timeout(Duration::from_secs(5), rx_allow.recv()).await {
            if let TuiEngineEvent::PermissionRequested { request_id, .. } = ev {
                engine_allow
                    .resolve_permission(PermissionDecisionPayload {
                        request_id,
                        allow: true,
                    })
                    .await
                    .expect("approve permission");
                break;
            }
        }
    });
    engine
        .send_turn_for_thread(&main.thread_id, "REQUIRE_PERMISSION allow test")
        .await
        .expect("allow turn");
    allow_task.await.expect("allow task");

    // 2. Permission Deny
    let mut rx_deny = engine.subscribe_tui_events();
    let engine_deny = Arc::clone(&engine);
    let deny_task = tokio::spawn(async move {
        while let Ok(Some(ev)) = timeout(Duration::from_secs(5), rx_deny.recv()).await {
            if let TuiEngineEvent::PermissionRequested { request_id, .. } = ev {
                engine_deny
                    .resolve_permission(PermissionDecisionPayload {
                        request_id,
                        allow: false,
                    })
                    .await
                    .expect("deny permission");
                break;
            }
        }
    });
    engine
        .send_turn_for_thread(&main.thread_id, "REQUIRE_PERMISSION deny test")
        .await
        .expect("deny turn");
    deny_task.await.expect("deny task");

    let transcript = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("transcript");
    let assistant_msgs: Vec<&str> = transcript
        .iter()
        .filter(|t| t.role == "assistant")
        .map(|t| t.content.as_str())
        .collect();
    assert!(assistant_msgs.contains(&"PERMISSION:allow"));
    assert!(assistant_msgs.contains(&"PERMISSION:deny"));

    // 3. Cancel active streaming turn
    let engine_cancel = Arc::clone(&engine);
    let main_id = main.thread_id.clone();
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        engine_cancel.cancel_turn(&main_id).await.expect("cancel");
    });
    timeout(
        Duration::from_secs(5),
        engine.send_turn_for_thread(&main.thread_id, "STREAM_SLOW"),
    )
    .await
    .expect("cancel turn timeout")
    .expect("cancel turn completed");
    cancel_task.await.expect("cancel task");

    // 4. Inline shell command execution
    let mut rx_shell = engine.subscribe_tui_events();
    let shell_res = engine
        .run_shell_command(&main.thread_id, "echo hello-shell")
        .await
        .expect("run shell");
    assert_eq!(shell_res.exit_code, 0);
    assert_eq!(shell_res.stdout.trim(), "hello-shell");
    let shell_ev = timeout(Duration::from_secs(2), rx_shell.recv())
        .await
        .expect("shell ev timeout")
        .expect("shell ev");
    assert!(matches!(
        shell_ev,
        TuiEngineEvent::ShellOutput { ref stdout, exit_code, .. }
            if stdout.trim() == "hello-shell" && exit_code == 0
    ));
}

#[tokio::test]
async fn test_e2e_unsolicited_subagent_wakeup_persistence_branch_local_and_delete_thread() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("wakeup.db");
    let cfg = fake_acp_config(temp.path());

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg.clone()))
        .expect("open engine");
    let main = engine.ensure_main_thread().await.expect("ensure main");

    // Seed Main with a trunk turn before spawning background subagent wakeup.
    engine
        .send_turn_for_thread(&main.thread_id, "trunk-seed-question")
        .await
        .expect("seed turn");

    let mut rx = engine.subscribe_tui_events();
    let main_id = main.thread_id.clone();
    let collector = tokio::spawn(async move {
        let mut saw_subagent = false;
        let mut completed_count = 0usize;
        let mut streamed = String::new();
        while let Ok(Some(ev)) = timeout(Duration::from_secs(5), rx.recv()).await {
            match ev {
                TuiEngineEvent::SubagentUpdated {
                    ref thread_id,
                    ref subagent,
                } if thread_id == &main_id => {
                    if subagent.role == "Background Code Checker"
                        && subagent.agent_type == "research"
                        && matches!(
                            subagent.status,
                            SubagentStatus::Running | SubagentStatus::Completed
                        )
                    {
                        saw_subagent = true;
                    }
                }
                TuiEngineEvent::StreamChunk {
                    ref thread_id,
                    ref content,
                } if thread_id == &main_id => {
                    streamed.push_str(content);
                }
                TuiEngineEvent::TurnCompleted { ref thread_id } if thread_id == &main_id => {
                    completed_count += 1;
                    if completed_count >= 2 && streamed.contains("WAKEUP_OK") {
                        break;
                    }
                }
                _ => {}
            }
        }
        (saw_subagent, completed_count, streamed)
    });

    engine
        .send_turn_for_thread(&main.thread_id, "SPAWN_SUBAGENT_WAKEUP")
        .await
        .expect("spawn wakeup turn");

    let (saw_subagent, completed_count, streamed) = timeout(Duration::from_secs(6), collector)
        .await
        .expect("collector timeout")
        .expect("collector join");
    assert!(saw_subagent, "expected SubagentUpdated event from invoke_subagent");
    assert!(completed_count >= 2, "expected solicited + wakeup TurnCompleted, got {completed_count}");
    assert!(streamed.contains("Spawned background subagent."));
    assert!(streamed.contains("WAKEUP_OK: background subagent finished"));

    // Verify SQLite persistence across full engine reopen.
    drop(engine);
    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg.clone()))
            .expect("reopen engine");
    let persisted_main = reopened
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("persisted main transcript");
    assert!(
        persisted_main
            .iter()
            .any(|t| t.role == "assistant" && t.content.contains("WAKEUP_OK: background subagent finished")),
        "unsolicited wakeup output must persist in SQLite across reopen: {persisted_main:?}"
    );

    // Fork a branch and verify branch-local turns + ThreadMeta parent/inherited count.
    let fork = reopened
        .fork_thread_with_title(Some(&main.thread_id), Some("side-fork"))
        .await
        .expect("fork branch");
    let fork_initial = reopened
        .get_thread_transcript(&fork.thread_id)
        .await
        .expect("fork initial transcript");
    assert!(
        fork_initial.is_empty(),
        "forked branch must load only branch-local turns initially, got: {fork_initial:?}"
    );

    let meta = reopened.thread_metadata().await.expect("thread metadata");
    let fork_meta = meta
        .iter()
        .find(|m| m.thread_id == fork.thread_id)
        .expect("fork metadata");
    assert_eq!(fork_meta.parent_thread_id.as_deref(), Some(main.thread_id.as_str()));
    assert!(
        fork_meta.inherited_turn_count >= 4,
        "expected >= 4 inherited trunk turns above fork point, got {}",
        fork_meta.inherited_turn_count
    );

    reopened
        .send_turn_for_thread(&fork.thread_id, "branch-only-question")
        .await
        .expect("fork turn");
    let fork_after = reopened
        .get_thread_transcript(&fork.thread_id)
        .await
        .expect("fork branch-local transcript");
    assert!(fork_after.iter().any(|t| t.content == "branch-only-question"));
    assert!(
        !fork_after.iter().any(|t| t.content == "trunk-seed-question"),
        "ancestor trunk turns must not duplicate inside forked transcript: {fork_after:?}"
    );

    // Delete forked branch (verify ThreadDeleted event), then verify sole-thread guard on Main.
    let mut del_rx = reopened.subscribe_tui_events();
    let next_active = reopened
        .delete_thread(&fork.thread_id)
        .await
        .expect("delete forked branch");
    assert_eq!(next_active, main.thread_id);

    let mut saw_deleted = false;
    while let Ok(Some(ev)) = timeout(Duration::from_millis(500), del_rx.recv()).await {
        if let TuiEngineEvent::ThreadDeleted {
            ref thread_id,
            ref next_active_thread_id,
        } = ev
        {
            if thread_id == &fork.thread_id && next_active_thread_id == &main.thread_id {
                saw_deleted = true;
                break;
            }
        }
    }
    assert!(saw_deleted, "expected ThreadDeleted event");
    assert_eq!(reopened.list_threads().await.expect("threads").len(), 1);
    assert!(
        reopened.delete_thread(&main.thread_id).await.is_err(),
        "deleting sole remaining thread must be refused"
    );

    drop(reopened);
    let reopened_again =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("reopen after delete");
    assert_eq!(reopened_again.list_threads().await.expect("threads").len(), 1);
}

#[tokio::test]
#[ignore = "requires live local agy binary and Google OAuth session"]
async fn live_agy_acp_smoke() {
    let agy_bin = std::env::var("AGY_BIN").unwrap_or_else(|_| "agy".to_string());
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("live_smoke.db");
    let cfg = AcpBackendConfig {
        command: agy_bin,
        args: vec!["acp".to_string(), "--stdio".to_string()],
        auth_method: Some("oauth-personal".to_string()),
        cwd: temp.path().to_path_buf(),
    };

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open live engine");
    let main = engine.ensure_main_thread().await.expect("ensure main");

    timeout(
        Duration::from_secs(60),
        engine.send_turn_for_thread(
            &main.thread_id,
            "Remember the exact code word CEDAR_99 for this conversation. Reply with only the single word PONG and do not call any tools.",
        ),
    )
    .await
    .expect("live turn 1 timeout")
    .expect("live turn 1 ok");

    let main_transcript = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("main transcript");
    let reply1 = &main_transcript.last().expect("reply 1").content;
    assert!(
        reply1.to_uppercase().contains("PONG"),
        "expected PONG from live agy, got: {reply1}"
    );

    let branch = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("smoke-branch"))
        .await
        .expect("fork branch");
    timeout(
        Duration::from_secs(60),
        engine.send_turn_for_thread(
            &branch.thread_id,
            "What is the exact code word from the ancestor conversation trunk? Reply with only the value and do not call any tools.",
        ),
    )
    .await
    .expect("live branch turn timeout")
    .expect("live branch turn ok");

    let branch_transcript = engine
        .get_thread_transcript(&branch.thread_id)
        .await
        .expect("branch transcript");
    let reply2 = &branch_transcript.last().expect("reply 2").content;
    assert!(
        reply2.contains("CEDAR_99"),
        "expected CEDAR_99 from live agy branch via pure DAG trunk context, got: {reply2}"
    );
}

#[tokio::test]
async fn test_e2e_join_landmark_preservation_and_blank_root_context_invariant() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("join_and_blank_root.db");
    let log_path = temp.path().join("fake_acp_wire.jsonl");

    // Seed legacy blackboard_entries rows before opening engine.
    {
        let legacy_storage = SqliteStorage::open(&db_path).expect("open legacy sqlite");
        legacy_storage
            .insert_blackboard_entry(&BlackboardEntry::new(
                "legacy_key",
                "LEGACY_BB_SECRET",
                BlackboardEntryKind::Decision,
            ))
            .expect("insert legacy row");
    }

    let cfg = fake_acp_config_with_log(temp.path(), &log_path);

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open engine");
    let main = engine.ensure_main_thread().await.expect("ensure main");

    // Seed Main with a secret conversation turn (creates Main's live ACP session sess-1).
    engine
        .send_turn_for_thread(&main.thread_id, "MAIN_SECRET_TURN_ALPHA")
        .await
        .expect("main turn");

    // Joining a root context must return EngineError::InvalidParams and leave merged == false.
    let root_join_err = engine
        .merge_thread_with_summary(&main.thread_id, None, Some("cannot join root"))
        .await
        .expect_err("joining Main root must fail");
    assert!(matches!(root_join_err, EngineError::InvalidParams(_)));
    assert!(
        !engine
            .list_threads()
            .await
            .expect("list")
            .into_iter()
            .find(|t| t.thread_id == main.thread_id)
            .unwrap()
            .merged
    );

    // Fork branch_a and branch_b off Main.
    let branch_a = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("Auth Research"))
        .await
        .expect("fork branch_a");
    let branch_b = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("Sibling Consumer"))
        .await
        .expect("fork branch_b");

    engine
        .send_turn_for_thread(&branch_a.thread_id, "analyze auth tokens")
        .await
        .expect("branch_a turn");
    engine
        .send_turn_for_thread(&branch_b.thread_id, "initial sibling prompt")
        .await
        .expect("branch_b turn 1");

    // Run /join on branch_a -> commits DecisionLandmark onto Main and queues pending landmark for live Main session.
    let joined = engine
        .merge_thread_with_summary(
            &branch_a.thread_id,
            None,
            Some("LANDMARK_ED25519_SELECTED"),
        )
        .await
        .expect("join branch_a");
    assert!(joined.merged);

    // Live sibling receives ZERO [Joined Context], ZERO [Shared Whiteboard Updated], ZERO landmark bleed.
    engine
        .send_turn_for_thread(&branch_b.thread_id, "ECHO_LAST_PROMPT after join")
        .await
        .expect("branch_b turn 2");
    let branch_b_turns = engine
        .get_thread_transcript(&branch_b.thread_id)
        .await
        .expect("branch_b transcript");
    let sibling_echoed = &branch_b_turns.last().unwrap().content;
    assert_eq!(sibling_echoed, "ECHO_LAST_PROMPT after join");

    // Live parent (Main) receives [Joined Context] with DecisionLandmark on its next turn while keeping SAME ACP session_id.
    engine
        .send_turn_for_thread(&main.thread_id, "ECHO_LAST_PROMPT parent after join")
        .await
        .expect("main turn 2");
    let main_turns = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("main transcript");
    let parent_echoed = &main_turns.last().unwrap().content;
    assert!(
        parent_echoed.starts_with("[Joined Context]\n[Decision Landmark"),
        "expected [Joined Context] prefix on live parent, got: {parent_echoed}"
    );
    assert!(parent_echoed.contains("LANDMARK_ED25519_SELECTED"));
    assert!(parent_echoed.ends_with("---\n\nECHO_LAST_PROMPT parent after join"));

    // Subsequent turn on Main has drained pending_landmarks -> no duplicate [Joined Context].
    engine
        .send_turn_for_thread(&main.thread_id, "ECHO_LAST_PROMPT parent turn 3")
        .await
        .expect("main turn 3");
    let main_turns_3 = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("main transcript 3");
    assert_eq!(
        main_turns_3.last().unwrap().content,
        "ECHO_LAST_PROMPT parent turn 3"
    );

    // Verify Main kept the exact same ACP sessionId across /join without any session reset.
    let wire_frames: Vec<Value> = std::fs::read_to_string(&log_path)
        .expect("read fake_acp_wire.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).expect("valid json frame"))
        .collect();
    let main_prompt_sessions: Vec<&str> = wire_frames
        .iter()
        .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/prompt"))
        .filter(|f| {
            let txt = f["params"]["prompt"][0]["text"].as_str().unwrap_or("");
            txt == "MAIN_SECRET_TURN_ALPHA"
                || txt.contains("ECHO_LAST_PROMPT parent after join")
                || txt == "ECHO_LAST_PROMPT parent turn 3"
        })
        .map(|f| f["params"]["sessionId"].as_str().unwrap())
        .collect();
    assert_eq!(main_prompt_sessions.len(), 3);
    assert!(
        main_prompt_sessions
            .windows(2)
            .all(|w| w[0] == w[1]),
        "Main must reuse the exact same ACP sessionId after /join: {main_prompt_sessions:?}"
    );

    // New child forked from Main after /join receives Decision Landmark in systemPrompt.append.
    let branch_c = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("Post-Join Child"))
        .await
        .expect("fork branch_c");
    engine
        .send_turn_for_thread(&branch_c.thread_id, "DUMP_SYSTEM_PROMPT")
        .await
        .expect("branch_c dump");
    let branch_c_turns = engine
        .get_thread_transcript(&branch_c.thread_id)
        .await
        .expect("branch_c transcript");
    let branch_c_sys = &branch_c_turns.last().unwrap().content;
    assert!(branch_c_sys.contains("MAIN_SECRET_TURN_ALPHA"));
    assert!(branch_c_sys.contains("# Decision Landmarks"));
    assert!(branch_c_sys.contains("LANDMARK_ED25519_SELECTED"));
    assert!(!branch_c_sys.contains("# Shared Blackboard Working Memory"));
    assert!(!branch_c_sys.contains("LEGACY_BB_SECRET"));

    // Blank Root Context Invariant: create a brand-new root after Main turns and /join landmark.
    let fresh_root = engine
        .create_root_thread(Some("Blank Root"))
        .await
        .expect("create_root_thread");
    assert_eq!(fresh_root.custom_name.as_deref(), Some("Blank Root"));
    assert!(fresh_root.title.starts_with("Root "));
    assert!(
        engine
            .get_thread_transcript(&fresh_root.thread_id)
            .await
            .expect("fresh_root initial transcript")
            .is_empty(),
        "new root must start with empty transcript"
    );
    let fresh_join_err = engine
        .merge_thread_with_summary(&fresh_root.thread_id, None, None)
        .await
        .expect_err("joining fresh root must fail");
    assert!(matches!(fresh_join_err, EngineError::InvalidParams(_)));

    let meta = engine.thread_metadata().await.expect("metadata");
    let fresh_meta = meta
        .iter()
        .find(|m| m.thread_id == fresh_root.thread_id)
        .expect("fresh_root meta");
    assert_eq!(fresh_meta.parent_thread_id, None);
    assert_eq!(fresh_meta.inherited_turn_count, 0);
    assert!(!fresh_meta.merged);

    // Run DUMP_SYSTEM_PROMPT on fresh_root: ZERO Main turns, Decision Landmarks, or Blackboard may leak.
    engine
        .send_turn_for_thread(&fresh_root.thread_id, "DUMP_SYSTEM_PROMPT")
        .await
        .expect("fresh_root dump");
    let fresh_turns = engine
        .get_thread_transcript(&fresh_root.thread_id)
        .await
        .expect("fresh_root transcript after dump");
    let fresh_sys = &fresh_turns.last().unwrap().content;
    assert!(
        !fresh_sys.contains("MAIN_SECRET_TURN_ALPHA"),
        "Main turns must NOT leak into fresh root systemPrompt.append: {fresh_sys}"
    );
    assert!(
        !fresh_sys.contains("# Ancestor Conversation Trunk"),
        "fresh root must NOT have ancestor trunk section: {fresh_sys}"
    );
    assert!(
        !fresh_sys.contains("# Decision Landmarks"),
        "Main Decision Landmarks must NOT leak into fresh root: {fresh_sys}"
    );
    assert!(
        !fresh_sys.contains("[Decision Landmark"),
        "Main DecisionLandmark nodes must NOT leak into fresh root: {fresh_sys}"
    );
    assert!(
        !fresh_sys.contains("[Joined Context]"),
        "Joined Context must NOT leak into fresh root: {fresh_sys}"
    );
    assert!(
        !fresh_sys.contains("# Shared Blackboard Working Memory"),
        "Shared Blackboard must NEVER appear in fresh root: {fresh_sys}"
    );
    assert!(
        !fresh_sys.contains("LEGACY_BB_SECRET"),
        "Legacy SQLite blackboard_entries rows must NEVER leak into fresh root: {fresh_sys}"
    );
}

fn fake_acp_config_with_flags(
    cwd: &Path,
    log_path: Option<&Path>,
    extra_flags: &[&str],
) -> AcpBackendConfig {
    let mut args = vec![fake_acp_script_path().to_string_lossy().into_owned()];
    if let Some(log) = log_path {
        args.push("--log-file".to_string());
        args.push(log.to_string_lossy().into_owned());
    }
    for flag in extra_flags {
        args.push((*flag).to_string());
    }
    AcpBackendConfig {
        command: "python3".to_string(),
        args,
        auth_method: Some("oauth-personal".to_string()),
        cwd: cwd.to_path_buf(),
    }
}

#[tokio::test]
async fn test_e2e_agent_options_before_turn1_config_set_and_default_propagation() {
    use grein_cli::engine::{dispatch_json_rpc, UnixSocketEngineClient};
    use grein_cli::run_daemon_with_socket_and_db;
    use tokio::net::UnixStream;

    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("cfg.db");
    let log_path = temp.path().join("cfg_wire.jsonl");
    let cfg = fake_acp_config_with_flags(temp.path(), Some(&log_path), &["--emit-commands"]);

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open engine");
    let main = engine.ensure_main_thread().await.expect("ensure main");
    let mut rx = engine.subscribe_tui_events();

    // Calling agent_options before Turn 1 lazily initializes the ACP session and returns config options.
    let opts = engine
        .agent_options(&main.thread_id)
        .await
        .expect("agent_options before Turn 1");
    assert_eq!(opts.thread_id, main.thread_id);
    let model_opt = opts
        .config_options
        .iter()
        .find(|o| o.id == "model")
        .expect("model config option");
    assert_eq!(model_opt.current_value, "fake-model");
    assert_eq!(model_opt.options.len(), 2);
    assert!(opts.config_options.iter().any(|o| o.id == "mode"));
    assert!(opts.config_options.iter().any(|o| o.id == "thought_level"));

    // Wait briefly for --emit-commands available_commands_update from the new session.
    let mut saw_cmds = false;
    while let Ok(Some(ev)) = timeout(Duration::from_millis(400), rx.recv()).await {
        if let TuiEngineEvent::AgentCommandsUpdated {
            ref thread_id,
            ref commands,
        } = ev
        {
            if thread_id == &main.thread_id && commands.iter().any(|c| c.name == "compact") {
                saw_cmds = true;
                break;
            }
        }
    }
    assert!(saw_cmds, "expected AgentCommandsUpdated from ACP session");
    let opts_cached = engine
        .agent_options(&main.thread_id)
        .await
        .expect("cached agent_options");
    assert!(opts_cached
        .available_commands
        .iter()
        .any(|c| c.name == "compact"));

    // Invalid option value returns Err cleanly without corrupting defaults.
    let bad_err = engine
        .set_agent_config(&main.thread_id, "thought_level", "bogus", true)
        .await
        .expect_err("invalid value must error");
    assert!(bad_err.to_string().contains("invalid option"));

    // Set per-thread only (set_default = false) -> does NOT propagate to newly forked branch.
    let updated_local = engine
        .set_agent_config(&main.thread_id, "thought_level", "low", false)
        .await
        .expect("set per-thread thought_level");
    assert_eq!(
        updated_local
            .config_options
            .iter()
            .find(|o| o.id == "thought_level")
            .unwrap()
            .current_value,
        "low"
    );

    let mut saw_cfg_ev = false;
    while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
        if let TuiEngineEvent::AgentConfigUpdated {
            ref thread_id,
            ref config_options,
        } = ev
        {
            if thread_id == &main.thread_id
                && config_options
                    .iter()
                    .any(|o| o.id == "thought_level" && o.current_value == "low")
            {
                saw_cfg_ev = true;
                break;
            }
        }
    }
    assert!(saw_cfg_ev, "expected AgentConfigUpdated after set_agent_config");

    let branch_local = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("per-thread-isolation"))
        .await
        .expect("fork branch_local");
    engine
        .send_turn_for_thread(&branch_local.thread_id, "DUMP_CONFIG")
        .await
        .expect("branch_local DUMP_CONFIG");
    let branch_local_reply = engine
        .get_thread_transcript(&branch_local.thread_id)
        .await
        .unwrap()
        .pop()
        .unwrap()
        .content;
    assert_eq!(
        branch_local_reply,
        "CONFIG:model=fake-model,mode=default,thought_level=medium"
    );

    // Set default (set_default = true) -> updates Main AND propagates to newly forked branches!
    engine
        .set_agent_config(&main.thread_id, "model", "fake-model-fast", true)
        .await
        .expect("set default model");
    engine
        .set_agent_config(&main.thread_id, "thought_level", "high", true)
        .await
        .expect("set default thought_level");

    engine
        .send_turn_for_thread(&main.thread_id, "DUMP_CONFIG")
        .await
        .expect("main DUMP_CONFIG");
    let main_cfg_reply = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .unwrap()
        .pop()
        .unwrap()
        .content;
    assert_eq!(
        main_cfg_reply,
        "CONFIG:model=fake-model-fast,mode=default,thought_level=high"
    );

    let branch_default = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("inherits-defaults"))
        .await
        .expect("fork branch_default");
    let branch_opts = engine
        .agent_options(&branch_default.thread_id)
        .await
        .expect("agent_options on forked branch");
    assert_eq!(
        branch_opts
            .config_options
            .iter()
            .find(|o| o.id == "model")
            .unwrap()
            .current_value,
        "fake-model-fast"
    );
    assert_eq!(
        branch_opts
            .config_options
            .iter()
            .find(|o| o.id == "thought_level")
            .unwrap()
            .current_value,
        "high"
    );
    engine
        .send_turn_for_thread(&branch_default.thread_id, "DUMP_CONFIG")
        .await
        .expect("branch_default DUMP_CONFIG");
    let branch_default_reply = engine
        .get_thread_transcript(&branch_default.thread_id)
        .await
        .unwrap()
        .pop()
        .unwrap()
        .content;
    assert_eq!(
        branch_default_reply,
        "CONFIG:model=fake-model-fast,mode=default,thought_level=high"
    );

    // JSON-RPC dispatch test for session.agent_options & session.set_agent_config
    let rpc_opts_raw = dispatch_json_rpc(
        &engine,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "session.agent_options",
            "params": {"thread_id": branch_default.thread_id}
        })
        .to_string(),
    )
    .await;
    let rpc_opts: Value = serde_json::from_str(&rpc_opts_raw).expect("rpc json");
    assert_eq!(rpc_opts["result"]["thread_id"], branch_default.thread_id);

    let rpc_set_raw = dispatch_json_rpc(
        &engine,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "session.set_agent_config",
            "params": {
                "thread_id": branch_default.thread_id,
                "config_id": "mode",
                "value": "architect",
                "set_default": false
            }
        })
        .to_string(),
    )
    .await;
    let rpc_set: Value = serde_json::from_str(&rpc_set_raw).expect("rpc set json");
    assert!(rpc_set["error"].is_null());

    // Verify UnixSocketEngineClient round-trip for session.agent_options & session.set_agent_config
    let sock_path = temp.path().join("cfg_daemon.sock");
    let sock_db = temp.path().join("cfg_daemon.db");
    let sock_clone = sock_path.clone();
    let daemon_task = tokio::spawn(async move {
        let _ = run_daemon_with_socket_and_db(sock_clone, sock_db).await;
    });
    for _ in 0..40 {
        if sock_path.exists() && UnixStream::connect(&sock_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let sock_client = UnixSocketEngineClient::connect(&sock_path)
        .await
        .expect("connect sock");
    let sock_main = sock_client.ensure_main_thread().await.expect("sock main");
    let sock_opts = sock_client
        .agent_options(&sock_main.thread_id)
        .await
        .expect("sock agent_options");
    assert_eq!(sock_opts.thread_id, sock_main.thread_id);
    daemon_task.abort();

    // Legacy ACP server (--legacy-config: models + modes only, no configOptions, no set_config_option)
    let legacy_db = temp.path().join("legacy_cfg.db");
    let legacy_cfg = fake_acp_config_with_flags(temp.path(), None, &["--legacy-config"]);
    let legacy_engine =
        InProcessEngine::open_with_config(&legacy_db, temp.path().to_path_buf(), Some(legacy_cfg))
            .expect("open legacy");
    let legacy_main = legacy_engine.ensure_main_thread().await.expect("legacy main");
    let legacy_opts = legacy_engine
        .agent_options(&legacy_main.thread_id)
        .await
        .expect("legacy agent_options");
    assert!(legacy_opts.config_options.iter().any(|o| o.id == "model" && o.current_value == "fake-model"));
    assert!(legacy_opts.config_options.iter().any(|o| o.id == "mode" && o.current_value == "default"));

    let legacy_model_res = legacy_engine
        .set_agent_config(&legacy_main.thread_id, "model", "fake-model-fast", true)
        .await
        .expect("legacy fallback set_session_model");
    assert_eq!(
        legacy_model_res
            .config_options
            .iter()
            .find(|o| o.id == "model")
            .unwrap()
            .current_value,
        "fake-model-fast"
    );
    let legacy_mode_res = legacy_engine
        .set_agent_config(&legacy_main.thread_id, "mode", "architect", false)
        .await
        .expect("legacy fallback set_session_mode");
    assert_eq!(
        legacy_mode_res
            .config_options
            .iter()
            .find(|o| o.id == "mode")
            .unwrap()
            .current_value,
        "architect"
    );
    legacy_engine
        .send_turn_for_thread(&legacy_main.thread_id, "DUMP_CONFIG")
        .await
        .expect("legacy DUMP_CONFIG");
    let legacy_reply = legacy_engine
        .get_thread_transcript(&legacy_main.thread_id)
        .await
        .unwrap()
        .pop()
        .unwrap()
        .content;
    assert_eq!(
        legacy_reply,
        "CONFIG:model=fake-model-fast,mode=architect,thought_level=medium"
    );
}

#[tokio::test]
async fn test_e2e_concurrent_turn_serialization_and_enriched_tool_events() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("serial_tools.db");
    let cfg = fake_acp_config(temp.path());

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open engine"),
    );
    let main = engine.ensure_main_thread().await.expect("ensure main");

    // 1. Concurrent overlapping turns on the same thread serialize cleanly and both succeed.
    let e1 = Arc::clone(&engine);
    let t1 = main.thread_id.clone();
    let e2 = Arc::clone(&engine);
    let t2 = main.thread_id.clone();
    let (r1, r2) = tokio::join!(
        async move { e1.send_turn_for_thread(&t1, "STREAM_SLOW").await },
        async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            e2.send_turn_for_thread(&t2, "ECHO_LAST_PROMPT after slow").await
        }
    );
    r1.expect("first overlapping turn must succeed");
    r2.expect("second overlapping turn must succeed");

    let transcript = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("transcript");
    let contents: Vec<(&str, &str)> = transcript
        .iter()
        .map(|t| (t.role.as_str(), t.content.as_str()))
        .collect();
    assert_eq!(
        contents,
        vec![
            ("user", "STREAM_SLOW"),
            ("assistant", "PART1 PART2"),
            ("user", "ECHO_LAST_PROMPT after slow"),
            ("assistant", "ECHO_LAST_PROMPT after slow"),
        ]
    );

    // 2. Enriched ToolInvoked (tool_call_id, summary) and ToolResult (tool_call_id, is_error).
    let mut rx = engine.subscribe_tui_events();
    engine
        .send_turn_for_thread(&main.thread_id, "TOOL_SUMMARY_AND_ERROR")
        .await
        .expect("tool summary turn");

    let mut evs = Vec::new();
    while let Ok(Some(ev)) = timeout(Duration::from_millis(300), rx.recv()).await {
        let done = matches!(
            ev,
            TuiEngineEvent::ThreadStatusChanged { ref status, .. } if status == "idle"
        );
        evs.push(ev);
        if done {
            break;
        }
    }

    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolInvoked {
            tool_name,
            tool_call_id: Some(tc_id),
            summary: Some(summary),
            ..
        } if tool_name == "run_command"
            && tc_id == "tc-cmd-1"
            && summary == "cargo test --locked"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolResult {
            tool_name,
            tool_call_id: Some(tc_id),
            is_error: true,
            output,
            ..
        } if tool_name == "run_command" && tc_id == "tc-cmd-1" && output == "exit code 101"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolInvoked {
            tool_name,
            tool_call_id: Some(tc_id),
            summary: Some(summary),
            ..
        } if tool_name == "read_file"
            && tc_id == "tc-read-2"
            && summary == "Read Cargo manifest"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolResult {
            tool_name,
            tool_call_id: Some(tc_id),
            is_error: false,
            output,
            ..
        } if tool_name == "read_file" && tc_id == "tc-read-2" && output == "[workspace]"
    )));
}
