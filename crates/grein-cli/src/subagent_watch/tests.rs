use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grein_tui::engine_port::{SubagentSnapshot, SubagentStatus, TuiEngineEvent};
use serde_json::{json, Value};

use super::{shorten_model_name, SubagentWatcher};

fn write_jsonl(path: &Path, lines: &[Value]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    let mut body = String::new();
    for l in lines {
        body.push_str(&serde_json::to_string(l).expect("json"));
        body.push('\n');
    }
    fs::write(path, body).expect("write jsonl");
}

fn append_jsonl(path: &Path, line: &Value) {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open append");
    writeln!(f, "{}", serde_json::to_string(line).expect("json")).expect("append line");
}

#[test]
fn missing_brain_root_no_ops_gracefully() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("does-not-exist");
    let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
    let events_cb = Arc::clone(&events);
    let watcher = SubagentWatcher::with_timings(
        missing,
        Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
        Duration::from_millis(50),
        Duration::from_millis(20),
    );

    watcher.register_subagent(
        "t-1",
        SubagentSnapshot {
            id: "tc-1".to_string(),
            role: "Codebase Researcher".to_string(),
            agent_type: "research".to_string(),
            model: None,
            activity: Some("running".to_string()),
            status: SubagentStatus::Running,
            started_at_ms: 1000,
        },
    );
    watcher.poll_once();
    assert!(!watcher.is_confirmed_running("t-1", "tc-1"));
    assert!(events.lock().unwrap().is_empty());
    watcher.shutdown();
}

#[test]
fn model_name_shortening_handles_common_variants() {
    assert_eq!(shorten_model_name("gemini-2.5-pro"), "gemini-2.5-pro");
    assert_eq!(
        shorten_model_name("models/gemini-2.5-pro-preview-06-05"),
        "gemini-2.5-pro"
    );
    assert_eq!(
        shorten_model_name("claude-3-7-sonnet-20250219"),
        "claude-3-7-sonnet"
    );
    assert_eq!(
        shorten_model_name("claude-sonnet-4-5@20250929"),
        "claude-sonnet-4-5"
    );
}

#[tokio::test]
async fn extracts_model_live_activity_and_parent_sender_completion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let brain = temp.path().join("brain");
    let parent_id = "parent-conv-1";
    let child_id = "child-conv-99";
    let parent_log = brain
        .join(parent_id)
        .join(".system_generated/logs/transcript.jsonl");
    let child_log = brain
        .join(child_id)
        .join(".system_generated/logs/transcript.jsonl");

    write_jsonl(
        &parent_log,
        &[
            json!({
                "step_index": 1,
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "tool_calls": [{
                    "name": "invoke_subagent",
                    "args": {"Role": "Database Auditor", "TypeName": "research"}
                }]
            }),
            json!({
                "step_index": 2,
                "type": "GENERIC",
                "status": "DONE",
                "content": format!("Created the following subagent:\n{{\"conversationId\":\"{child_id}\"}}")
            }),
        ],
    );

    write_jsonl(
        &child_log,
        &[json!({
            "step_index": 1,
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "model": "models/gemini-2.5-pro-preview-05-06",
            "thinking": "Inspecting schema files",
            "tool_calls": [{
                "name": "read_file",
                "args": {"Path": "/repo/crates/grein-core/src/storage/sqlite.rs"}
            }]
        })],
    );

    let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
    let events_cb = Arc::clone(&events);
    let watcher = SubagentWatcher::with_timings(
        brain.clone(),
        Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
        Duration::from_millis(50),
        Duration::from_millis(40),
    );

    watcher.register_subagent(
        "t-main",
        SubagentSnapshot {
            id: "tc-sub-1".to_string(),
            role: "Database Auditor".to_string(),
            agent_type: "research".to_string(),
            model: None,
            activity: Some("running".to_string()),
            status: SubagentStatus::Running,
            started_at_ms: 1000,
        },
    );

    watcher.poll_once();
    assert!(watcher.is_confirmed_running("t-main", "tc-sub-1"));

    {
        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentUpdated { thread_id, subagent }
                if thread_id == "t-main"
                    && subagent.id == "tc-sub-1"
                    && subagent.model.as_deref() == Some("gemini-2.5-pro")
                    && subagent.activity.as_deref() == Some("read_file sqlite.rs")
                    && subagent.status == SubagentStatus::Running
        )));
    }

    // Parent receives wakeup [Message] sender=<child_id> -> marks Completed and schedules removal.
    append_jsonl(
        &parent_log,
        &json!({
            "step_index": 3,
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": format!("[Message] timestamp=2026-11-22T10:30:00Z sender={child_id} priority=HIGH content=Audit complete")
        }),
    );

    watcher.poll_once();
    assert!(!watcher.is_confirmed_running("t-main", "tc-sub-1"));
    {
        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentUpdated { subagent, .. }
                if subagent.id == "tc-sub-1" && subagent.status == SubagentStatus::Completed
        )));
    }

    tokio::time::sleep(Duration::from_millis(120)).await;
    {
        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentRemoved { thread_id, subagent_id }
                if thread_id == "t-main" && subagent_id == "tc-sub-1"
        )));
    }
    watcher.shutdown();
}

#[tokio::test]
async fn detects_child_terminal_completion_and_stops_polling_when_empty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let brain = temp.path().join("brain");
    let child_id = "child-direct-42";
    let child_log = brain
        .join(child_id)
        .join(".system_generated/logs/transcript.jsonl");

    write_jsonl(
        &child_log,
        &[json!({
            "step_index": 1,
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "model": "claude-sonnet-4-5@20250929",
            "thinking": "Analyzing logs"
        })],
    );

    let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
    let events_cb = Arc::clone(&events);
    let watcher = SubagentWatcher::with_timings(
        brain.clone(),
        Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
        Duration::from_millis(30),
        Duration::from_millis(30),
    );

    watcher.register_subagent(
        "t-1",
        SubagentSnapshot {
            id: "tc-2".to_string(),
            role: "Log Analyzer".to_string(),
            agent_type: "research".to_string(),
            model: None,
            activity: Some("running".to_string()),
            status: SubagentStatus::Running,
            started_at_ms: 1000,
        },
    );
    watcher.ingest_tool_output(
        "t-1",
        "tc-2",
        &format!("Created the following subagent:\n{{\"conversationId\":\"{child_id}\"}}"),
    );

    tokio::time::sleep(Duration::from_millis(90)).await;
    assert!(watcher.is_confirmed_running("t-1", "tc-2"));

    append_jsonl(
        &child_log,
        &json!({
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "model": "claude-sonnet-4-5@20250929",
            "response": "All logs look healthy.",
            "tool_calls": []
        }),
    );

    tokio::time::sleep(Duration::from_millis(160)).await;
    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::SubagentUpdated { subagent, .. }
            if subagent.id == "tc-2"
                && subagent.model.as_deref() == Some("claude-sonnet-4-5")
                && subagent.status == SubagentStatus::Completed
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::SubagentRemoved { subagent_id, .. } if subagent_id == "tc-2"
    )));
    // Polling task should automatically stop once no subagents are running.
    assert!(!watcher.state.lock().unwrap().polling_active);
}
