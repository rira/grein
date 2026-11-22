use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use grein_cli::engine::{
    AcpBackendConfig, DaemonEngine, InProcessEngine, PermissionDecisionPayload,
};
use grein_tui::engine_port::TuiEngineEvent;
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

#[tokio::test]
async fn test_e2e_hydration_across_restart_streaming_branch_isolation_and_whiteboard_delta() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");
    let cfg = fake_acp_config(temp.path());

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg.clone()))
            .expect("open engine"),
    );

    let main = engine.ensure_main_thread().await.expect("ensure main");
    assert_eq!(main.title, "Main");

    engine
        .blackboard_upsert("architecture", "two-tier-dag")
        .await
        .expect("upsert whiteboard");

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
        dumped_prompt.contains("two-tier-dag"),
        "expected whiteboard fact in systemPrompt.append, got: {dumped_prompt}"
    );
    assert!(
        dumped_prompt.contains("main-trunk-anchor-secret"),
        "expected ancestor trunk turn in systemPrompt.append, got: {dumped_prompt}"
    );
    assert!(
        !dumped_prompt.contains("sibling-only-banana"),
        "branch_a must be isolated from sibling branch_b, got: {dumped_prompt}"
    );

    // Edit whiteboard mid-session -> branch_a's next prompt receives [Shared Whiteboard Updated].
    engine
        .blackboard_upsert("api_port", "9090")
        .await
        .expect("upsert api_port");

    engine
        .send_turn_for_thread(&branch_a.thread_id, "ECHO_LAST_PROMPT follow-up")
        .await
        .expect("branch_a echo last prompt");

    let branch_a_after_wb = engine
        .get_thread_transcript(&branch_a.thread_id)
        .await
        .expect("branch_a transcript after wb");
    let echoed_prompt = &branch_a_after_wb
        .last()
        .expect("echoed assistant reply")
        .content;
    assert!(echoed_prompt.contains("[Shared Whiteboard Updated]"));
    assert!(echoed_prompt.contains("9090"));
    assert!(echoed_prompt.ends_with("ECHO_LAST_PROMPT follow-up"));

    // Close DB and reopen engine -> verify SQLite hydration across restart.
    drop(engine);
    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("reopen engine");

    let hydrated_threads = reopened.list_threads().await.expect("list threads");
    assert_eq!(hydrated_threads.len(), 3);
    let titles: Vec<&str> = hydrated_threads.iter().map(|t| t.title.as_str()).collect();
    assert!(titles.contains(&"branch-a"));
    assert!(titles.contains(&"branch-b"));

    let hydrated_bb = reopened.blackboard_list().await.expect("list blackboard");
    assert_eq!(hydrated_bb.len(), 2);

    let persisted_a = reopened
        .get_thread_transcript(&branch_a.thread_id)
        .await
        .expect("reopened branch_a transcript");
    assert!(persisted_a
        .iter()
        .any(|t| t.role == "user" && t.content == "DUMP_SYSTEM_PROMPT"));
    assert!(persisted_a
        .iter()
        .any(|t| t.role == "assistant" && t.content.contains("9090")));
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
#[ignore = "requires live local agy binary and Google OAuth session"]
async fn live_agy_acp_smoke() {
    let agy_bin = std::env::var("AGY_BIN").unwrap_or_else(|_| "/home/richard/.local/bin/agy".to_string());
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
    engine
        .blackboard_upsert("GREIN_SMOKE_SENTINEL", "CEDAR_99")
        .await
        .expect("upsert sentinel");

    timeout(
        Duration::from_secs(60),
        engine.send_turn_for_thread(
            &main.thread_id,
            "Reply with only the single word PONG and do not call any tools.",
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
            "What is the exact value of GREIN_SMOKE_SENTINEL from the Shared Whiteboard? Reply with only the value and do not call any tools.",
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
        "expected CEDAR_99 from live agy branch via Shared Whiteboard injection, got: {reply2}"
    );
}
