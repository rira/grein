use grein_core::blackboard::{BlackboardEntry as LegacyBbEntry, BlackboardEntryKind};
use grein_core::model::ThreadId;
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::engine_port::TuiEngineEvent;
use serde_json::Value;

use super::super::types::parse_typed_id;
use super::super::{AcpBackendConfig, DaemonEngine, EngineError, InProcessEngine};
use super::write_executable_script;

#[tokio::test]
async fn test_sqlite_hydration_across_reopen_preserves_threads_and_ignores_legacy_blackboard() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");

    // Pre-populate legacy blackboard_entries rows before opening InProcessEngine.
    {
        let legacy_storage = SqliteStorage::open(&db_path).expect("open legacy sqlite");
        legacy_storage
            .insert_blackboard_entry(&LegacyBbEntry::new(
                "legacy_goal",
                "LEGACY_BLACKBOARD_VAL",
                BlackboardEntryKind::Decision,
            ))
            .expect("insert legacy row");
    }

    let engine =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("open legacy db");
    assert!(engine.list_threads().await.expect("list").is_empty());

    let main = engine.ensure_main_thread().await.expect("ensure main");
    assert_eq!(main.title, "Main");
    assert!(!main.merged);
    assert_eq!(main.status, grein_core::model::ThreadStatus::Idle);
    assert!(engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("transcript")
        .is_empty());

    let mut rx = engine.subscribe_tui_events();

    // Joining a root thread must return EngineError::InvalidParams and leave merged == false.
    let join_root_err = engine
        .merge_thread_with_summary(&main.thread_id, None, Some("should fail"))
        .await
        .expect_err("joining a root thread must fail");
    assert!(
        matches!(join_root_err, EngineError::InvalidParams(_)),
        "expected InvalidParams, got: {join_root_err:?}"
    );
    let main_after_err = engine
        .list_threads()
        .await
        .expect("threads")
        .into_iter()
        .find(|t| t.thread_id == main.thread_id)
        .expect("main exists");
    assert!(!main_after_err.merged);

    let forked = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("feat-auth"))
        .await
        .expect("fork branch");
    assert_eq!(forked.custom_name.as_deref(), Some("feat-auth"));
    assert_eq!(forked.title, "Branch 2");
    let merged = engine.merge_thread(&forked.thread_id).await.expect("merge");
    assert!(merged.merged);

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCreated { thread }
            if thread.id == forked.thread_id
                && thread.custom_name.as_deref() == Some("feat-auth")
                && thread.title == "Branch 2"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, .. }
            if thread_id == &forked.thread_id && target_thread_id == &main.thread_id
    )));

    drop(engine);

    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("reopen db");
    let threads = reopened.list_threads().await.expect("reopened threads");
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].thread_id, main.thread_id);
    assert_eq!(threads[0].title, "Main");
    assert!(!threads[0].merged);
    assert_eq!(threads[1].thread_id, forked.thread_id);
    assert_eq!(threads[1].custom_name.as_deref(), Some("feat-auth"));
    assert_eq!(threads[1].title, "Branch 2");
    assert!(threads[1].merged);

    let main_turns = reopened
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("main turns");
    assert!(main_turns
        .iter()
        .any(|t| t.content.contains("[Decision Landmark") && t.content.contains("feat-auth")));

    let snap = reopened.snapshot().await.expect("reopened snapshot");
    assert_eq!(snap.thread_count, 2);
    assert_eq!(snap.node_count, 1);
}

#[tokio::test]
async fn test_delete_thread_sole_guard_and_closes_acp_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");
    let capture_path = temp.path().join("close_capture.jsonl");

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
            case "$line" in
                *'"sessionId":"sess-1"'*) sid="sess-1" ;;
                *) sid="sess-2" ;;
            esac
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ok"}}}}\n' "$sid"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
        *'"method":"session/close"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
            ;;
    esac
done
"#;
    let wrapper = format!(
        "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
        capture_path.to_string_lossy(),
        mock_script
    );
    let script_path = write_executable_script(temp.path(), "mock_close.sh", &wrapper);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open");
    engine.set_test_subagent_brain(temp.path().join("missing-brain"));
    let main = engine.ensure_main_thread().await.expect("main");

    // Sole-thread guard
    let sole_err = engine
        .delete_thread(&main.thread_id)
        .await
        .expect_err("must reject deleting only thread");
    assert!(sole_err.to_string().contains("only thread"));

    engine
        .send_turn_for_thread(&main.thread_id, "hi")
        .await
        .expect("main turn");
    let fork = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("temp-branch"))
        .await
        .expect("fork");
    engine
        .send_turn_for_thread(&fork.thread_id, "fork turn")
        .await
        .expect("fork turn");

    let mut rx = engine.subscribe_tui_events();
    let next = engine
        .delete_thread(&fork.thread_id)
        .await
        .expect("delete fork");
    assert_eq!(next, main.thread_id);

    let mut evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        evs.push(ev);
    }
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadDeleted { thread_id, next_active_thread_id }
            if thread_id == &fork.thread_id && next_active_thread_id == &main.thread_id
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadMetadataUpdated { meta } if meta.len() == 1
    )));

    let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
        .expect("capture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert!(frames.iter().any(|f| {
        f.get("method").and_then(Value::as_str) == Some("session/close")
            && f["params"]["sessionId"] == "sess-2"
    }));

    // Sole-thread guard remains enforced.
    assert!(engine.delete_thread(&main.thread_id).await.is_err());
}

#[tokio::test]
async fn test_manual_join_delivers_landmark_to_live_parent_isolates_siblings_and_rejects_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("join.db");
    let capture_path = temp.path().join("join_capture.jsonl");

    // Seed legacy rows in SQLite blackboard_entries to prove they open cleanly and are ignored.
    {
        let legacy_storage = SqliteStorage::open(&db_path).expect("open legacy sqlite");
        legacy_storage
            .insert_blackboard_entry(&LegacyBbEntry::new(
                "legacy_secret",
                "SHOULD_BE_IGNORED",
                BlackboardEntryKind::Decision,
            ))
            .expect("seed legacy row");
    }

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
            case "$line" in
                *'"sessionId":"sess-1"'*) sid="sess-1" ;;
                *'"sessionId":"sess-2"'*) sid="sess-2" ;;
                *'"sessionId":"sess-3"'*) sid="sess-3" ;;
                *) sid="sess-4" ;;
            esac
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Branch decision: use SQLite WAL mode."}}}}\n' "$sid"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
    let wrapper = format!(
        "#!/bin/sh\nexport GREIN_CAPTURE={:?}\n{}\n",
        capture_path.to_string_lossy(),
        mock_script
    );
    let script_path = write_executable_script(temp.path(), "mock_join.sh", &wrapper);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    engine
        .send_turn_for_thread(&main.thread_id, "kickoff")
        .await
        .expect("main turn");

    let main_tid: ThreadId = parse_typed_id(&main.thread_id).unwrap();
    let main_session_id_before = engine
        .runtime
        .storage
        .get_thread(main_tid)
        .unwrap()
        .unwrap()
        .acp_session_id
        .expect("main acp_session_id");
    assert_eq!(main_session_id_before, "sess-1");

    // Joining a root thread returns EngineError::InvalidParams and leaves merged == false.
    let join_root_err = engine
        .merge_thread_with_summary(&main.thread_id, None, Some("bad root join"))
        .await
        .expect_err("joining root must fail");
    assert!(matches!(join_root_err, EngineError::InvalidParams(_)));
    assert!(!engine
        .runtime
        .storage
        .get_thread(main_tid)
        .unwrap()
        .unwrap()
        .merged);

    let branch_a = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("Storage Choice"))
        .await
        .expect("fork a");
    let branch_b = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("Sibling"))
        .await
        .expect("fork b");

    engine
        .send_turn_for_thread(&branch_a.thread_id, "choose storage")
        .await
        .expect("branch_a turn");
    engine
        .send_turn_for_thread(&branch_b.thread_id, "sibling initial")
        .await
        .expect("branch_b turn 1");

    let mut rx = engine.subscribe_tui_events();
    let merged = engine
        .merge_thread_with_summary(&branch_a.thread_id, None, None)
        .await
        .expect("merge_thread_with_summary");
    assert!(merged.merged);

    let mut evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        evs.push(ev);
    }
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, summary }
            if thread_id == &branch_a.thread_id
                && target_thread_id == &main.thread_id
                && summary.contains("SQLite WAL mode")
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadMetadataUpdated { meta }
            if meta.iter().any(|m| m.thread_id == branch_a.thread_id && m.merged)
    )));

    let main_transcript = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("main transcript");
    assert!(main_transcript.iter().any(|t| {
        t.content.contains("[Decision Landmark") && t.content.contains("SQLite WAL mode")
    }));

    // Live sibling branch receives ZERO [Joined Context] and ZERO [Shared Whiteboard Updated].
    engine
        .send_turn_for_thread(&branch_b.thread_id, "sibling after join")
        .await
        .expect("branch_b turn 2");

    // Live parent (Main) receives [Joined Context] on its next turn while keeping SAME ACP session_id.
    engine
        .send_turn_for_thread(&main.thread_id, "main after join")
        .await
        .expect("main turn 2");
    let main_session_id_after = engine
        .runtime
        .storage
        .get_thread(main_tid)
        .unwrap()
        .unwrap()
        .acp_session_id
        .expect("main acp_session_id after");
    assert_eq!(main_session_id_after, main_session_id_before);

    // Turn 3 on Main does NOT repeat [Joined Context].
    engine
        .send_turn_for_thread(&main.thread_id, "main follow-up 3")
        .await
        .expect("main turn 3");

    // Newly created blank root receives ZERO [Joined Context] and ZERO legacy blackboard.
    let blank_root = engine
        .create_root_thread(Some("Blank Root"))
        .await
        .expect("create blank root");
    let blank_join_err = engine
        .merge_thread_with_summary(&blank_root.thread_id, None, None)
        .await
        .expect_err("joining blank root must fail");
    assert!(matches!(blank_join_err, EngineError::InvalidParams(_)));
    let blank_tid: ThreadId = parse_typed_id(&blank_root.thread_id).unwrap();
    assert!(!engine
        .runtime
        .storage
        .get_thread(blank_tid)
        .unwrap()
        .unwrap()
        .merged);

    engine
        .send_turn_for_thread(&blank_root.thread_id, "blank root prompt")
        .await
        .expect("blank root turn");

    let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
        .expect("capture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();

    // Exactly 4 session/new calls: main, branch_a, branch_b, blank_root (no reset of main!).
    let session_news: Vec<&Value> = frames
        .iter()
        .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/new"))
        .collect();
    assert_eq!(session_news.len(), 4);
    for sn in &session_news {
        let append = sn["params"]["_meta"]["antigravity"]["systemPrompt"]["append"]
            .as_str()
            .unwrap_or("");
        assert!(!append.contains("# Shared Blackboard Working Memory"));
        assert!(!append.contains("SHOULD_BE_IGNORED"));
        assert!(!append.contains("[Joined Context]"));
    }

    let prompts: Vec<(&str, &str)> = frames
        .iter()
        .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/prompt"))
        .map(|f| {
            (
                f["params"]["sessionId"].as_str().unwrap(),
                f["params"]["prompt"][0]["text"].as_str().unwrap(),
            )
        })
        .collect();

    // Sibling sess-3 got verbatim prompt, zero [Joined Context] / [Shared Whiteboard Updated].
    let sibling_prompts: Vec<&str> = prompts
        .iter()
        .filter(|(sid, _)| *sid == "sess-3")
        .map(|(_, text)| *text)
        .collect();
    assert_eq!(sibling_prompts, vec!["sibling initial", "sibling after join"]);

    // Main sess-1 got [Joined Context] with DecisionLandmark on turn 2, and clean prompt on turn 3.
    let main_prompts: Vec<&str> = prompts
        .iter()
        .filter(|(sid, _)| *sid == "sess-1")
        .map(|(_, text)| *text)
        .collect();
    assert_eq!(main_prompts.len(), 3);
    assert_eq!(main_prompts[0], "kickoff");
    assert!(main_prompts[1].starts_with("[Joined Context]\n[Decision Landmark"));
    assert!(main_prompts[1].contains("SQLite WAL mode"));
    assert!(main_prompts[1].ends_with("---\n\nmain after join"));
    assert_eq!(main_prompts[2], "main follow-up 3");

    // Blank root sess-4 got clean prompt.
    let blank_prompts: Vec<&str> = prompts
        .iter()
        .filter(|(sid, _)| *sid == "sess-4")
        .map(|(_, text)| *text)
        .collect();
    assert_eq!(blank_prompts, vec!["blank root prompt"]);
}
