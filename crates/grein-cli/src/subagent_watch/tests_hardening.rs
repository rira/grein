use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grein_tui::engine_port::{SubagentSnapshot, SubagentStatus, TuiEngineEvent};
use serde_json::json;

use super::parser::{
    detect_child_terminal_state, extract_child_activity, extract_created_subagent,
    extract_message_senders, find_matching_unlinked_key, read_complete_lines_from,
    resolve_truncated_line, shorten_model_name,
};
use super::{SubagentWatcher, TrackedEntry};

fn make_snap(id: &str, role: &str, agent_type: &str) -> SubagentSnapshot {
    SubagentSnapshot {
        id: id.into(),
        role: role.into(),
        agent_type: agent_type.into(),
        model: None,
        activity: Some("running".into()),
        status: SubagentStatus::Running,
        started_at_ms: 1000,
    }
}

fn make_entry(
    thread_id: &str,
    snap: SubagentSnapshot,
    gen_id: u64,
    child_id: Option<&str>,
) -> TrackedEntry {
    TrackedEntry {
        thread_id: thread_id.into(),
        snapshot: snap,
        child_conversation_id: child_id.map(str::to_string),
        generation: gen_id,
    }
}

#[test]
fn resolve_truncated_line_reads_transcript_full_by_step_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let t_path = temp.path().join("transcript.jsonl");
    let full_path = temp.path().join("transcript_full.jsonl");

    // No truncated_fields -> None
    assert!(resolve_truncated_line(&t_path, &json!({"step_index": 1})).is_none());
    assert!(
        resolve_truncated_line(&t_path, &json!({"step_index": 1, "truncated_fields": []}))
            .is_none()
    );
    // Missing transcript_full.jsonl -> None
    assert!(resolve_truncated_line(
        &t_path,
        &json!({"step_index": 2, "truncated_fields": ["content"]})
    )
    .is_none());
    // Missing step_index -> None
    assert!(resolve_truncated_line(&t_path, &json!({"truncated_fields": ["content"]})).is_none());

    fs::write(
        &full_path,
        format!(
            "{{corrupt-line\n{}\n{}\n",
            json!({"step_index": 1, "content": "step one"}),
            json!({"step_index": 7, "content": "Created the following subagent:\n{\"conversationId\":\"c-7\"}"}),
        ),
    )
    .expect("write full");

    let resolved = resolve_truncated_line(
        &t_path,
        &json!({"step_index": 7, "truncated_fields": ["content"], "content": "..."}),
    )
    .expect("must resolve step 7");
    assert_eq!(resolved["step_index"], 7);
    assert!(resolved["content"].as_str().unwrap().contains("c-7"));

    // Non-matching step_index -> None
    assert!(resolve_truncated_line(
        &t_path,
        &json!({"step_index": 99, "truncated_fields": ["content"]})
    )
    .is_none());
}

#[test]
fn read_complete_lines_from_handles_partial_buffering_and_file_shrink() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("tail.jsonl");

    // Missing file returns (prev_offset, [])
    assert_eq!(read_complete_lines_from(&path, 12), (12, Vec::new()));

    // Partial line without newline -> offset stays 0, empty vec
    fs::write(&path, "{\"a\":1}").expect("write partial");
    assert_eq!(read_complete_lines_from(&path, 0), (0, Vec::new()));

    // Complete first line + partial second line
    fs::write(&path, "{\"a\":1}\n{\"b\":2").expect("write mixed");
    let (off1, lines1) = read_complete_lines_from(&path, 0);
    assert_eq!(lines1, vec!["{\"a\":1}"]);
    assert_eq!(off1, "{\"a\":1}\n".len() as u64);

    // Same offset with no new newline -> empty vec
    assert_eq!(read_complete_lines_from(&path, off1), (off1, Vec::new()));

    // Finish second line
    fs::write(&path, "{\"a\":1}\n{\"b\":2}\n").expect("finish line 2");
    let (off2, lines2) = read_complete_lines_from(&path, off1);
    assert_eq!(lines2, vec!["{\"b\":2}"]);
    assert!(off2 > off1);

    // File shrinks below prev_offset -> resets to 0 and reads from start
    fs::write(&path, "{\"c\":3}\n").expect("shrink");
    let (off3, lines3) = read_complete_lines_from(&path, off2);
    assert_eq!(lines3, vec!["{\"c\":3}"]);
    assert_eq!(off3, "{\"c\":3}\n".len() as u64);
}

#[test]
fn extract_created_subagent_handles_escaped_quotes_nested_braces_and_unbalanced() {
    let nested_escaped = r#"Created the following subagent:
{"conversationId":"conv-esc","meta":{"note":"brace } and \"quoted {fake}\"","deep":{"x":1}},"role":"Auditor","typeName":"research"} trailing"#;
    let parsed = extract_created_subagent(nested_escaped).expect("should parse nested/escaped");
    assert_eq!(parsed.conversation_id, "conv-esc");
    assert_eq!(parsed.role.as_deref(), Some("Auditor"));
    assert_eq!(parsed.agent_type.as_deref(), Some("research"));

    // Alternative key aliases
    let alt = r#"Created the following subagent: {"conversation_id":"conv-alt","Role":"R","agentType":"T"}"#;
    let parsed_alt = extract_created_subagent(alt).expect("alt aliases");
    assert_eq!(parsed_alt.conversation_id, "conv-alt");
    assert_eq!(parsed_alt.role.as_deref(), Some("R"));
    assert_eq!(parsed_alt.agent_type.as_deref(), Some("T"));

    // Unbalanced JSON -> None
    assert!(extract_created_subagent(
        r#"Created the following subagent: {"conversationId":"unbalanced","nested":{"x":1}"#
    )
    .is_none());
    // Empty conversationId -> None
    assert!(
        extract_created_subagent(r#"Created the following subagent: {"conversationId":"  "}"#)
            .is_none()
    );
}

#[test]
fn find_matching_unlinked_key_fallback_tiers() {
    let mut entries = HashMap::new();
    entries.insert(
        "t::linked".into(),
        make_entry(
            "t",
            make_snap("linked", "Researcher", "research"),
            1,
            Some("c-already"),
        ),
    );
    entries.insert(
        "t::a".into(),
        make_entry("t", make_snap("a", "Planner", "plan"), 2, None),
    );
    entries.insert(
        "t::b".into(),
        make_entry("t", make_snap("b", "Reviewer", "review"), 3, None),
    );
    entries.insert(
        "t::c".into(),
        make_entry("t", make_snap("c", "Tester", "review"), 4, None),
    );

    // Tier 1: exact role + type (case-insensitive)
    assert_eq!(
        find_matching_unlinked_key(&entries, Some("reviewer"), Some("REVIEW")).as_deref(),
        Some("t::b")
    );
    // Tier 2: role-only when type doesn't match
    assert_eq!(
        find_matching_unlinked_key(&entries, Some("tester"), Some("unknown")).as_deref(),
        Some("t::c")
    );
    // Tier 3: unique agent_type match ("plan" appears once)
    assert_eq!(
        find_matching_unlinked_key(&entries, Some("mismatch"), Some("plan")).as_deref(),
        Some("t::a")
    );
    // Tier 4: ambiguous agent_type ("review" appears twice) -> falls back to oldest running unlinked generation (gen 2 = "t::a")
    assert_eq!(
        find_matching_unlinked_key(&entries, Some("mismatch"), Some("review")).as_deref(),
        Some("t::a")
    );
    // Tier 4: both None -> oldest generation ("t::a")
    assert_eq!(
        find_matching_unlinked_key(&entries, None, None).as_deref(),
        Some("t::a")
    );
}

#[test]
fn terminal_state_tool_activity_and_model_name_variants() {
    // Error / Failed terminal states
    for ty in ["ERROR", "FAILED", "TASK_FAILED"] {
        let (st, act) =
            detect_child_terminal_state(&json!({"type": ty, "error": "boom"})).expect(ty);
        assert_eq!(st, SubagentStatus::Failed);
        assert_eq!(act, "boom");
    }
    let (st_status, act_status) = detect_child_terminal_state(&json!({
        "type": "STEP",
        "status": "FAILED",
        "content": "   "
    }))
    .expect("status failed");
    assert_eq!(st_status, SubagentStatus::Failed);
    assert_eq!(act_status, "failed");

    // Completed terminal states
    for ty in [
        "COMPLETED",
        "FINAL_RESPONSE",
        "TURN_COMPLETE",
        "TASK_COMPLETE",
    ] {
        let (st, act) = detect_child_terminal_state(&json!({"type": ty})).expect(ty);
        assert_eq!(st, SubagentStatus::Completed);
        assert_eq!(act, "done");
    }

    // PLANNER_RESPONSE with tool_calls is NOT terminal; without tool_calls + text IS terminal
    assert!(detect_child_terminal_state(&json!({
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "tool_calls": [{"name": "read_file"}],
        "response": "intermediate"
    }))
    .is_none());
    assert!(detect_child_terminal_state(&json!({
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "response": "   "
    }))
    .is_none());

    // Tool activity: CommandLine & query truncation with ellipsis + fallback branches
    let long_cmd = "cargo test --workspace --all-targets --locked --extra-long-flag";
    let cmd_act = extract_child_activity(&json!({
        "tool_calls": [{"name": "run_command", "args": {"CommandLine": long_cmd}}]
    }))
    .expect("cmd activity");
    assert!(cmd_act.starts_with("run_command "));
    assert!(cmd_act.ends_with('…'));

    let q_act = extract_child_activity(&json!({
        "tool_calls": [{"name": "grep", "args": {"query": "find_very_long_identifier_across_crate"}}]
    }))
    .expect("query activity");
    assert!(q_act.starts_with("grep "));
    assert!(q_act.ends_with('…'));

    assert_eq!(
        extract_child_activity(&json!({"tool_name": "custom_tool"})).as_deref(),
        Some("custom_tool")
    );
    assert_eq!(
        extract_child_activity(&json!({"thinking": "first\nsecond"})).as_deref(),
        Some("thinking: first second")
    );

    // shorten_model_name -exp-
    assert_eq!(
        shorten_model_name("models/gemini-2.0-pro-exp-02-05"),
        "gemini-2.0-pro"
    );

    // extract_message_senders
    assert_eq!(
        extract_message_senders("noise\n[Message] sender=abc-123_4 content=hi\n[Message] no_sender"),
        vec!["abc-123_4".to_string()]
    );
}

#[tokio::test]
async fn watcher_unlinked_activity_mark_terminal_clear_and_generation_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let brain = temp.path().join("brain");
    fs::create_dir_all(&brain).unwrap();

    let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
    let events_cb = Arc::clone(&events);
    let watcher = SubagentWatcher::with_timings(
        brain,
        Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
        Duration::from_millis(200),
        Duration::from_millis(60),
    );

    // 1. update_activity_if_unlinked & mark_terminal idempotency + generation mismatch on schedule_removal
    watcher.register_subagent("t-1", make_snap("sa-1", "Worker", "research"));
    watcher.update_activity_if_unlinked("t-1", "missing", "ignored");
    watcher.update_activity_if_unlinked("t-1", "sa-1", "running"); // unchanged -> no-op
    watcher.update_activity_if_unlinked("t-1", "sa-1", "step 2");

    // Mark terminal (gen 1) -> schedules removal in 60ms
    watcher.mark_terminal("t-1", "sa-1", SubagentStatus::Failed, None);
    // Idempotent second mark_terminal with same status -> ignored
    watcher.mark_terminal("t-1", "sa-1", SubagentStatus::Failed, Some("dup".into()));
    // Re-register same id immediately (gen 2, Running) before gen 1 timer fires!
    watcher.register_subagent("t-1", make_snap("sa-1", "Worker", "research"));

    tokio::time::sleep(Duration::from_millis(110)).await;
    {
        let evs = events.lock().unwrap();
        let updated_count = evs
            .iter()
            .filter(|e| matches!(e, TuiEngineEvent::SubagentUpdated { subagent, .. } if subagent.id == "sa-1"))
            .count();
        // 1 for "step 2", 1 for first mark_terminal (second mark_terminal was idempotent!)
        assert_eq!(updated_count, 2, "expected exact 2 updates, got {evs:?}");
        assert!(
            !evs.iter()
                .any(|e| matches!(e, TuiEngineEvent::SubagentRemoved { subagent_id, .. } if subagent_id == "sa-1")),
            "generation mismatch must NOT remove re-registered gen 2 subagent"
        );
    }

    // 2. clear_thread removes only target thread's subagents and emits SubagentRemoved
    watcher.register_subagent("t-other", make_snap("sa-other", "Other", "research"));
    watcher.clear_thread("t-1");
    assert!(!watcher.is_confirmed_running("t-1", "sa-1"));
    {
        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentRemoved { thread_id, subagent_id }
                if thread_id == "t-1" && subagent_id == "sa-1"
        )));
        assert!(!evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentRemoved { thread_id, .. } if thread_id == "t-other"
        )));
    }
    watcher.clear_thread("t-other");
    watcher.shutdown();

    // 3. Truncated parent transcript resolved via transcript_full.jsonl links child & detects ERROR terminal state
    let brain_trunc = temp.path().join("brain_trunc");
    let parent_log = brain_trunc.join("parent-1/.system_generated/logs/transcript.jsonl");
    let full_log = brain_trunc.join("parent-1/.system_generated/logs/transcript_full.jsonl");
    let child_log = brain_trunc.join("child-trunc-1/.system_generated/logs/transcript.jsonl");
    fs::create_dir_all(parent_log.parent().unwrap()).unwrap();
    fs::create_dir_all(child_log.parent().unwrap()).unwrap();

    fs::write(
        &parent_log,
        format!(
            "{}\n",
            json!({"step_index": 1, "truncated_fields": ["content"], "content": "truncated"})
        ),
    )
    .unwrap();
    fs::write(
        &full_log,
        format!(
            "{}\n",
            json!({
                "step_index": 1,
                "content": "Created the following subagent:\n{\"conversationId\":\"child-trunc-1\",\"role\":\"TruncWorker\",\"typeName\":\"research\"}"
            })
        ),
    )
    .unwrap();
    fs::write(
        &child_log,
        format!(
            "{}\n",
            json!({"step_index": 1, "type": "ERROR", "error": "oom crash"})
        ),
    )
    .unwrap();

    let events2 = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
    let events2_cb = Arc::clone(&events2);
    let watcher2 = SubagentWatcher::with_timings(
        brain_trunc,
        Arc::new(move |ev| events2_cb.lock().unwrap().push(ev)),
        Duration::from_millis(200),
        Duration::from_millis(60),
    );
    watcher2.register_subagent("t-trunc", make_snap("sa-trunc", "TruncWorker", "research"));
    watcher2.poll_once();
    let mut found_trunc = false;
    for _ in 0..25 {
        if events2.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                TuiEngineEvent::SubagentUpdated { thread_id, subagent }
                    if thread_id == "t-trunc"
                        && subagent.id == "sa-trunc"
                        && subagent.status == SubagentStatus::Failed
                        && subagent.activity.as_deref() == Some("oom crash")
            )
        }) {
            found_trunc = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        found_trunc,
        "expected truncated parent transcript to link child & report oom crash, got {:?}",
        *events2.lock().unwrap()
    );

    watcher2.shutdown();
}

#[test]
fn default_brain_root_and_outside_runtime_mark_terminal() {
    let _root = SubagentWatcher::default_brain_root();
    let temp = tempfile::tempdir().expect("tempdir");
    let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
    let events_cb = Arc::clone(&events);
    let watcher = SubagentWatcher::new(
        Path::new(temp.path()).to_path_buf(),
        Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
    );
    watcher.register_subagent("t-sync", make_snap("sa-sync", "Sync", "research"));
    watcher.mark_terminal("t-sync", "sa-sync", SubagentStatus::Completed, None);
    let evs = events.lock().unwrap();
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::SubagentUpdated { subagent, .. }
            if subagent.id == "sa-sync" && subagent.activity.as_deref() == Some("done")
    )));
    watcher.shutdown();
}
