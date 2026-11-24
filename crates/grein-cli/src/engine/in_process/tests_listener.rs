use std::sync::Arc;
use std::time::Duration;

use grein_acp::{
    AcpPermissionOption, AcpStdioDriver, DriverEvent, StdioProcessConfig, StdioProcessRunner,
};
use grein_core::model::{AuthorRole, ConversationNode, NodeId, ThreadId};
use grein_tui::engine_port::TuiEngineEvent;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use super::super::types::parse_typed_id;
use super::listener::spawn_session_listener;
use super::{InProcessEngine, SessionListenerCmd};

async fn dummy_driver() -> Arc<AcpStdioDriver> {
    let cfg = StdioProcessConfig::new("sh", vec!["-c".to_string(), "cat".to_string()]);
    let runner = StdioProcessRunner::spawn(cfg).await.expect("spawn cat");
    Arc::new(AcpStdioDriver::new(runner))
}

#[tokio::test]
async fn test_listener_eof_during_solicited_vs_unsolicited_buffered_text() {
    let engine = InProcessEngine::in_memory().expect("in_memory");
    let main = engine.ensure_main_thread().await.expect("main");
    let tid: ThreadId = parse_typed_id(&main.thread_id).expect("tid");
    let leaf: NodeId = parse_typed_id(&main.active_leaf_id).expect("leaf");
    let driver = dummy_driver().await;
    let mut rx = engine.subscribe_tui_events();

    // Case 1: stream EOF during solicited_in_flight
    let (ev_tx1, ev_rx1) = mpsc::unbounded_channel();
    let (cmd_tx1, cmd_rx1) = mpsc::unbounded_channel();
    let h1 = spawn_session_listener(
        Arc::clone(&engine.runtime),
        tid,
        main.thread_id.clone(),
        "s1".into(),
        Arc::clone(&driver),
        leaf,
        ev_rx1,
        cmd_rx1,
    );
    let (done_tx, done_rx) = oneshot::channel();
    let (ack_tx, ack_rx) = oneshot::channel();
    cmd_tx1
        .send(SessionListenerCmd::StartSolicited {
            user_node_id: leaf,
            done_tx,
            ack_tx,
        })
        .unwrap();
    ack_rx.await.unwrap();
    drop(ev_tx1);
    let res = done_rx.await.unwrap();
    assert!(res.is_err());
    h1.await.unwrap();

    // Case 2: stream EOF with buffered unsolicited text flushes assistant node
    let (ev_tx2, ev_rx2) = mpsc::unbounded_channel();
    let (_cmd_tx2, cmd_rx2) = mpsc::unbounded_channel();
    engine.set_test_quiet_timeout(Duration::from_secs(10));
    let h2 = spawn_session_listener(
        Arc::clone(&engine.runtime),
        tid,
        main.thread_id.clone(),
        "s2".into(),
        Arc::clone(&driver),
        leaf,
        ev_rx2,
        cmd_rx2,
    );
    ev_tx2
        .send(DriverEvent::StreamChunk {
            session_id: "s2".into(),
            text: "unsolicited flushed on EOF".into(),
        })
        .unwrap();
    drop(ev_tx2);
    h2.await.unwrap();

    let transcript = engine.get_thread_transcript(&main.thread_id).await.unwrap();
    assert!(transcript
        .iter()
        .any(|t| t.role == "assistant" && t.content == "unsolicited flushed on EOF"));

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    assert!(events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::Error { message } if message.contains("ACP stream closed"))));
    assert!(events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::TurnCompleted { .. })));

    let _ = driver.shutdown().await;
}

#[tokio::test]
async fn test_listener_driver_error_prepare_solicited_permission_and_tool_fallbacks() {
    let engine = InProcessEngine::in_memory().expect("in_memory");
    let main = engine.ensure_main_thread().await.expect("main");
    let tid: ThreadId = parse_typed_id(&main.thread_id).expect("tid");
    let leaf: NodeId = parse_typed_id(&main.active_leaf_id).expect("leaf");
    let driver = dummy_driver().await;
    engine.set_test_quiet_timeout(Duration::from_millis(60));
    let mut rx = engine.subscribe_tui_events();

    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let h = spawn_session_listener(
        Arc::clone(&engine.runtime),
        tid,
        main.thread_id.clone(),
        "s-mix".into(),
        Arc::clone(&driver),
        leaf,
        ev_rx,
        cmd_rx,
    );

    // 1. Unsolicited stream chunk followed by PermissionRequested clears quiet_deadline
    ev_tx
        .send(DriverEvent::StreamChunk {
            session_id: "s-mix".into(),
            text: "waiting on perm".into(),
        })
        .unwrap();
    ev_tx
        .send(DriverEvent::PermissionRequested {
            session_id: "s-mix".into(),
            request_id: json!(99),
            tool_call_id: "tc-perm".into(),
            tool_name: "run_cmd".into(),
            title: "Run".into(),
            args: json!({}),
            options: vec![AcpPermissionOption {
                option_id: "allow".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            }],
        })
        .unwrap();
    // Wait longer than quiet_timeout (60ms) -> PermissionRequested cleared quiet_deadline so it didn't finalize yet
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(engine
        .get_thread_transcript(&main.thread_id)
        .await
        .unwrap()
        .is_empty());

    // 2. PrepareSolicited flushes buffered unsolicited text before next turn
    let (prep_tx, prep_rx) = oneshot::channel();
    cmd_tx
        .send(SessionListenerCmd::PrepareSolicited { reply_tx: prep_tx })
        .unwrap();
    let flushed_leaf = prep_rx.await.unwrap();
    assert_ne!(flushed_leaf, leaf);
    assert_eq!(
        engine
            .get_thread_transcript(&main.thread_id)
            .await
            .unwrap()
            .last()
            .map(|t| t.content.as_str()),
        Some("waiting on perm")
    );

    // 3. Unsolicited DriverEvent::Error clears accumulated text & marks error
    ev_tx
        .send(DriverEvent::StreamChunk {
            session_id: "s-mix".into(),
            text: "discarded on error".into(),
        })
        .unwrap();
    ev_tx
        .send(DriverEvent::Error {
            session_id: Some("s-mix".into()),
            message: "unsolicited err".into(),
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;

    // 4. ToolCall & ToolCallUpdate fallbacks (empty name + empty title -> "tool", empty name -> active_tool_names, failed + empty output -> status)
    let user_node = ConversationNode::new(NodeId::new(), Some(flushed_leaf), AuthorRole::User, "u".into());
    engine.runtime.storage.insert_node(&user_node).unwrap();
    engine.runtime.storage.update_thread_leaf(tid, user_node.id).unwrap();

    let (done_tx, done_rx) = oneshot::channel();
    let (ack_tx, ack_rx) = oneshot::channel();
    cmd_tx
        .send(SessionListenerCmd::StartSolicited {
            user_node_id: user_node.id,
            done_tx,
            ack_tx,
        })
        .unwrap();
    ack_rx.await.unwrap();

    // ToolCall with empty name + empty title -> "tool", then ToolCallUpdate with empty name & failed + empty output -> "failed"
    ev_tx
        .send(DriverEvent::ToolCall {
            session_id: "s-mix".into(),
            tool_call_id: "tc-a".into(),
            name: "".into(),
            title: "".into(),
            args: json!({}),
            status: "pending".into(),
        })
        .unwrap();
    ev_tx
        .send(DriverEvent::ToolCallUpdate {
            session_id: "s-mix".into(),
            tool_call_id: "tc-a".into(),
            name: "".into(),
            status: "failed".into(),
            output: "".into(),
        })
        .unwrap();
    // ToolCallUpdate for unknown tool_call_id with empty name -> "tool"
    ev_tx
        .send(DriverEvent::ToolCallUpdate {
            session_id: "s-mix".into(),
            tool_call_id: "tc-unknown".into(),
            name: "".into(),
            status: "completed".into(),
            output: "".into(),
        })
        .unwrap();
    // Solicited DriverEvent::Error -> sets solicited_error_emitted and fails done_rx
    ev_tx
        .send(DriverEvent::Error {
            session_id: Some("s-mix".into()),
            message: "solicited boom".into(),
        })
        .unwrap();
    let err_res = done_rx.await.unwrap();
    assert!(matches!(err_res, Err(super::super::types::EngineError::Transport(m)) if m == "solicited boom"));

    let (abort_tx, abort_rx) = oneshot::channel();
    cmd_tx
        .send(SessionListenerCmd::AbortSolicited { ack_tx: abort_tx })
        .unwrap();
    assert!(abort_rx.await.unwrap());

    drop(cmd_tx);
    h.await.unwrap();

    let mut events = Vec::new();
    while let Ok(Some(ev)) = timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(ev);
    }
    assert!(events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::PermissionRequested { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::ToolInvoked { tool_name, .. } if tool_name == "tool")));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolResult { tool_name, output, is_error: true, .. }
            if tool_name == "tool" && output == "failed"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ToolResult { tool_name, output, is_error: false, .. }
            if tool_name == "tool" && output.is_empty()
    )));
    assert!(events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::Error { message } if message == "unsolicited err")));
    assert!(events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::Error { message } if message == "solicited boom")));

    let _ = driver.shutdown().await;
}
