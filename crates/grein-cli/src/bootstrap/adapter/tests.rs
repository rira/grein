use std::sync::Arc;
use std::time::Duration;

use crate::engine::{
    AcpBackendConfig, AgentMetaEntry, AgentTurnPort, EngineError, EngineEvent, EngineEventPort,
    InProcessEngine, PermissionDecisionPayload, SessionSnapshot, ThreadSummary, ThreadUseCasePort,
    TranscriptEntry, TurnDispatchResult,
};
use grein_tui::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentOptionsSnapshot, AgentSlashCommand, ThreadMeta,
    TuiEngineEvent, TuiEnginePort,
};
use tokio::sync::{mpsc, Mutex};

use super::EngineClientPort;

#[tokio::test]
async fn test_engine_client_port_hydrates_main_thread_and_no_duplicate_errors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");

    let engine =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("open");
    let port = EngineClientPort::new(engine);

    let session = port.hydrate_session().await.expect("hydrate");
    assert_eq!(session.threads.len(), 1);
    assert_eq!(session.active_thread_id, session.threads[0].id);
    assert_eq!(session.threads[0].title, "Main");
    assert!(session.threads[0].turns.is_empty());

    let mut rx = port.subscribe_events().await.expect("subscribe");

    let forked = port
        .fork_branch(&session.active_thread_id, Some("auth"))
        .await
        .expect("fork_branch");
    assert_eq!(forked.custom_name.as_deref(), Some("auth"));
    assert_eq!(forked.title, "Branch 2");

    let err = port
        .send_turn(&session.active_thread_id, "hello")
        .await
        .expect_err("offline send_turn must fail");
    assert!(!err.to_string().is_empty());

    port.run_shell_command(&session.active_thread_id, "printf 'hi'")
        .await
        .expect("shell");

    let mut events = Vec::new();
    for _ in 0..20 {
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        if events
            .iter()
            .any(|e| matches!(e, TuiEngineEvent::ShellOutput { .. }))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, TuiEngineEvent::Error { .. })),
        "EngineClientPort must not emit duplicate Error event when send_turn returns Err"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ShellOutput { stdout, exit_code, .. } if stdout == "hi" && *exit_code == 0
    )));

    drop(port);

    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("reopen");
    let port2 = EngineClientPort::new(reopened);
    let session2 = port2.hydrate_session().await.expect("rehydrate");
    assert_eq!(session2.threads.len(), 2);
    assert_eq!(session2.threads[0].turns.len(), 1);
    assert_eq!(session2.threads[0].turns[0].role, "user");
    assert_eq!(session2.threads[0].turns[0].content, "hello");
}

#[tokio::test]
async fn test_engine_client_port_live_acp_streams_permissions_and_rehydrates() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::time::timeout;

    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");
    let script_path = temp.path().join("mock_tui_acp.sh");
    let script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-tui"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-tui","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Mocked TUI Reply"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
    std::fs::write(&script_path, script).expect("write");
    let mut perms = std::fs::metadata(&script_path)
        .expect("meta")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("chmod");

    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };
    let engine =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open");
    let port = Arc::new(EngineClientPort::new(engine));

    let session = port.hydrate_session().await.expect("hydrate");
    let main_id = session.active_thread_id.clone();
    let mut rx = port.subscribe_events().await.expect("subscribe");

    port.send_turn(&main_id, "ping acp")
        .await
        .expect("send_turn");

    let mut chunks = String::new();
    let mut saw_completed = false;
    while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
        match ev {
            TuiEngineEvent::StreamChunk { content, .. } => chunks.push_str(&content),
            TuiEngineEvent::TurnCompleted { .. } => {
                saw_completed = true;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(chunks, "Mocked TUI Reply");
    assert!(saw_completed);

    port.cancel_turn(&main_id).await.expect("cancel_turn");
    let forked = port
        .fork_branch(&main_id, Some("side-branch"))
        .await
        .expect("fork_branch");
    assert!(
        forked.turns.is_empty(),
        "forked branch must hydrate only branch-local turns, got {:?}",
        forked.turns
    );
    let meta = port.thread_metadata().await.expect("thread_metadata");
    assert_eq!(meta.len(), 2);
    let fork_meta = meta
        .iter()
        .find(|m| m.thread_id == forked.id)
        .expect("fork meta");
    assert_eq!(fork_meta.parent_thread_id.as_deref(), Some(main_id.as_str()));
    assert_eq!(fork_meta.inherited_turn_count, 2);

    port.merge_thread(&forked.id, &main_id)
        .await
        .expect("merge_thread");
    port.resolve_permission("nonexistent-perm", true)
        .await
        .expect("resolve_permission");

    let next_active = port
        .delete_thread(&forked.id)
        .await
        .expect("delete_thread");
    assert_eq!(next_active, main_id);
    assert!(port.delete_thread(&main_id).await.is_err());

    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("reopen");
    let port2 = EngineClientPort::new(reopened);
    let hydrated = port2.hydrate_session().await.expect("rehydrate");
    assert_eq!(hydrated.threads.len(), 1);
    assert_eq!(hydrated.threads[0].turns.len(), 3);
    assert_eq!(hydrated.threads[0].turns[0].content, "ping acp");
    assert_eq!(hydrated.threads[0].turns[1].content, "Mocked TUI Reply");
    assert!(hydrated.threads[0].turns[2]
        .content
        .contains("[Decision Landmark"));
}

#[tokio::test]
async fn test_engine_client_port_forwards_async_background_errors() {
    struct BgErrorEngine {
        rx: Mutex<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>>,
    }

    #[async_trait::async_trait]
    impl ThreadUseCasePort for BgErrorEngine {
        async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
            Ok(ThreadSummary {
                thread_id: "t-1".into(),
                title: "Main".into(),
                custom_name: None,
                parent_node_id: "n-1".into(),
                active_leaf_id: "n-1".into(),
                merged: false,
                status: Default::default(),
                agent_provider: None,
            })
        }
        async fn fork_thread(
            &self,
            _parent_node_id: Option<&str>,
        ) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn fork_thread_with_title(
            &self,
            _source: Option<&str>,
            _title: Option<&str>,
        ) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn create_root_thread(
            &self,
            _title: Option<&str>,
        ) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn rename_thread(
            &self,
            _thread_id: &str,
            _title: &str,
        ) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
            Ok(vec![self.ensure_main_thread().await?])
        }
        async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
            Ok(Vec::new())
        }
        async fn get_thread_transcript(
            &self,
            _thread_id: &str,
        ) -> Result<Vec<TranscriptEntry>, EngineError> {
            Ok(Vec::new())
        }
        async fn merge_thread(&self, _thread_id: &str) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn merge_thread_with_summary(
            &self,
            _source_id: &str,
            _target_id: Option<&str>,
            _summary: Option<&str>,
        ) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
            unreachable!()
        }
    }

    #[async_trait::async_trait]
    impl AgentTurnPort for BgErrorEngine {
        async fn send_turn(&self, _prompt: &str) -> Result<TurnDispatchResult, EngineError> {
            Ok(TurnDispatchResult {
                node_id: "n-2".into(),
            })
        }
        async fn send_turn_for_thread(
            &self,
            _thread_id: &str,
            _prompt: &str,
        ) -> Result<TurnDispatchResult, EngineError> {
            Ok(TurnDispatchResult {
                node_id: "n-2".into(),
            })
        }
        async fn resolve_permission(
            &self,
            _decision: PermissionDecisionPayload,
        ) -> Result<EngineEvent, EngineError> {
            unreachable!()
        }
        async fn cancel_turn(&self, _thread_id: &str) -> Result<(), EngineError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl EngineEventPort for BgErrorEngine {
        async fn ping(&self) -> Result<(), EngineError> {
            Ok(())
        }
        async fn subscribe_events(
            &self,
        ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
            Ok(self.rx.lock().await.take())
        }
    }

    super::impl_default_auxiliary_ports!(BgErrorEngine);

    let (tx, rx) = mpsc::unbounded_channel();
    let port = EngineClientPort::new(BgErrorEngine {
        rx: Mutex::new(Some(rx)),
    });
    let _ = port.hydrate_session().await.expect("hydrate");
    let mut sub_rx = port.subscribe_events().await.expect("subscribe");

    tx.send(TuiEngineEvent::Error {
        message: "async wakeup failed".to_string(),
    })
    .expect("send bg error");

    let mut saw_bg_err = false;
    while let Ok(Some(ev)) =
        tokio::time::timeout(std::time::Duration::from_millis(150), sub_rx.recv()).await
    {
        if matches!(ev, TuiEngineEvent::Error { ref message } if message == "async wakeup failed")
        {
            saw_bg_err = true;
            break;
        }
    }
    assert!(
        saw_bg_err,
        "async background TuiEngineEvent::Error must be forwarded to TUI"
    );
}

#[tokio::test]
async fn test_engine_client_port_offline_fallbacks_and_metadata_error_forwarding() {
    let engine = InProcessEngine::in_memory().expect("in_memory");
    let port = EngineClientPort::new(engine);
    let session = port.hydrate_session().await.expect("hydrate");
    let main_id = session.active_thread_id.clone();

    // Notice: subscribe_events NOT called -> has_live_stream == false!
    // Drain initial events via direct receiver? Actually subscribe_events on InProcessEngine
    // would flip has_live_stream to true. So test offline EngineClientPort with a no-stream
    // DaemonEngine wrapper or test create_root/rename/merge before subscribe_events!
    let root2 = port.create_root(Some("Offline Root")).await.expect("create_root");
    assert_eq!(root2.custom_name.as_deref(), Some("Offline Root"));
    assert_eq!(root2.title, "Root 2");
    port.rename_thread(&root2.id, "Renamed Offline Root")
        .await
        .expect("rename");
    port.rename_thread(&root2.id, "")
        .await
        .expect("clear offline rename");
    port.rename_thread(&root2.id, "Renamed Offline Root")
        .await
        .expect("restore offline rename");
    let forked = port
        .fork_branch(&main_id, Some("Offline Branch"))
        .await
        .expect("fork_branch");
    port.merge_thread_with_summary(&forked.id, &main_id, Some("Offline decision"))
        .await
        .expect("merge_thread_with_summary");

    let mut rx = port.events_rx.lock().await.take().expect("events_rx");
    let mut offline_evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        offline_evs.push(ev);
    }
    assert!(offline_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCreated { thread } if thread.id == root2.id
    )));
    assert!(offline_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
            if thread_id == &root2.id && custom_name.as_deref() == Some("Renamed Offline Root")
    )));
    assert!(offline_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
            if thread_id == &root2.id && custom_name.is_none()
    )));
    assert!(offline_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCreated { thread } if thread.id == forked.id
    )));
    assert!(offline_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, summary }
            if thread_id == &forked.id
                && target_thread_id == &main_id
                && summary == "Offline decision"
    )));

    struct FailingMetaEngine;
    #[async_trait::async_trait]
    impl ThreadUseCasePort for FailingMetaEngine {
        async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
            Ok(ThreadSummary {
                thread_id: "t-1".into(),
                title: "Main".into(),
                custom_name: None,
                parent_node_id: "n-1".into(),
                active_leaf_id: "n-1".into(),
                merged: false,
                status: Default::default(),
                agent_provider: None,
            })
        }
        async fn fork_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn fork_thread_with_title(&self, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn create_root_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn rename_thread(&self, _: &str, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
            Ok(vec![self.ensure_main_thread().await?])
        }
        async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
        async fn get_thread_transcript(&self, _: &str) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
        async fn merge_thread(&self, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn merge_thread_with_summary(&self, _: &str, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> { unreachable!() }
        async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
            Err(EngineError::Storage("metadata db failure".to_string()))
        }
    }
    #[async_trait::async_trait]
    impl AgentTurnPort for FailingMetaEngine {
        async fn send_turn(&self, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
        async fn send_turn_for_thread(&self, _: &str, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
        async fn resolve_permission(&self, _: PermissionDecisionPayload) -> Result<EngineEvent, EngineError> { unreachable!() }
        async fn cancel_turn(&self, _: &str) -> Result<(), EngineError> { Ok(()) }
    }
    #[async_trait::async_trait]
    impl EngineEventPort for FailingMetaEngine {
        async fn ping(&self) -> Result<(), EngineError> { Ok(()) }
    }
    super::impl_default_auxiliary_ports!(FailingMetaEngine);

    let err_port = EngineClientPort::new(FailingMetaEngine);
    let mut err_rx = err_port.subscribe_events().await.expect("subscribe");
    let _ = err_port.hydrate_session().await.expect("hydrate succeeds");
    assert!(err_port.thread_metadata().await.is_err());

    let mut err_evs = Vec::new();
    while let Ok(ev) = err_rx.try_recv() {
        err_evs.push(ev);
    }
    assert!(
        err_evs
            .iter()
            .filter(|e| matches!(e, TuiEngineEvent::Error { message } if message.contains("metadata db failure")))
            .count()
            >= 2,
        "thread_metadata Err must emit TuiEngineEvent::Error instead of ignoring: {err_evs:?}"
    );
}

#[tokio::test]
async fn test_engine_client_port_hydrates_cached_options_commands_and_config_rpcs() {
    struct CachedMetaEngine;

    #[async_trait::async_trait]
    impl ThreadUseCasePort for CachedMetaEngine {
        async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
            Ok(ThreadSummary {
                thread_id: "t-cached".into(),
                title: "Main".into(),
                custom_name: None,
                parent_node_id: "n-1".into(),
                active_leaf_id: "n-1".into(),
                merged: false,
                status: Default::default(),
                agent_provider: None,
            })
        }
        async fn fork_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn fork_thread_with_title(&self, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn create_root_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn rename_thread(&self, _: &str, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
            Ok(vec![self.ensure_main_thread().await?])
        }
        async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
        async fn get_thread_transcript(&self, _: &str) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
        async fn merge_thread(&self, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn merge_thread_with_summary(&self, _: &str, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
        async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> { unreachable!() }
    }

    #[async_trait::async_trait]
    impl AgentTurnPort for CachedMetaEngine {
        async fn send_turn(&self, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
        async fn send_turn_for_thread(&self, _: &str, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
        async fn resolve_permission(&self, _: PermissionDecisionPayload) -> Result<EngineEvent, EngineError> { unreachable!() }
        async fn cancel_turn(&self, _: &str) -> Result<(), EngineError> { Ok(()) }
        async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
            Ok(vec![AgentMetaEntry {
                thread_id: "t-cached".into(),
                model: Some("fake-model".into()),
                mode: Some("default".into()),
                config_options: vec![AgentConfigOption {
                    id: "model".into(),
                    name: "Model".into(),
                    category: Some("model".into()),
                    current_value: "fake-model".into(),
                    options: vec![AgentConfigChoice {
                        value: "fake-model".into(),
                        name: "Fake Model".into(),
                        description: None,
                    }],
                }],
                available_commands: vec![AgentSlashCommand {
                    name: "compact".into(),
                    description: "Compact context".into(),
                }],
            }])
        }
        async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
            Ok(AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: self.agent_metadata().await?.remove(0).config_options,
                available_commands: self.agent_metadata().await?.remove(0).available_commands,
            })
        }
        async fn set_agent_config(
            &self,
            thread_id: &str,
            config_id: &str,
            value: &str,
            _set_default: bool,
        ) -> Result<AgentOptionsSnapshot, EngineError> {
            Ok(AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: vec![AgentConfigOption {
                    id: config_id.to_string(),
                    name: "Model".into(),
                    category: Some("model".into()),
                    current_value: value.to_string(),
                    options: Vec::new(),
                }],
                available_commands: Vec::new(),
            })
        }
    }

    #[async_trait::async_trait]
    impl EngineEventPort for CachedMetaEngine {
        async fn ping(&self) -> Result<(), EngineError> { Ok(()) }
    }
    super::impl_default_auxiliary_ports!(CachedMetaEngine);

    let port = EngineClientPort::new(CachedMetaEngine);
    let session = port.hydrate_session().await.expect("hydrate");
    assert_eq!(session.active_thread_id, "t-cached");
    let mut rx = port.subscribe_events().await.expect("subscribe");

    let opts = port.agent_options("t-cached").await.expect("agent_options");
    assert_eq!(opts.config_options.len(), 1);
    assert_eq!(opts.available_commands.len(), 1);

    let updated = port
        .set_agent_config("t-cached", "model", "fake-model-fast", true)
        .await
        .expect("set_agent_config");
    assert_eq!(updated.config_options[0].current_value, "fake-model-fast");

    let mut evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        evs.push(ev);
    }
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::AgentMetaUpdated { thread_id, model, .. }
            if thread_id == "t-cached" && model.as_deref() == Some("fake-model")
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::AgentConfigUpdated { thread_id, config_options }
            if thread_id == "t-cached" && config_options.len() == 1
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::AgentCommandsUpdated { thread_id, commands }
            if thread_id == "t-cached" && commands.len() == 1 && commands[0].name == "compact"
    )));
}

#[tokio::test]
async fn test_engine_client_port_terminal_and_worktree_methods() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).expect("mkdir repo");
    let run_git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo_root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git failed: {:?}", args);
    };
    run_git(&["init"]);
    run_git(&["config", "user.name", "Grein Test"]);
    run_git(&["config", "user.email", "test@grein.dev"]);
    std::fs::write(repo_root.join("file.txt"), "hello\n").expect("write");
    run_git(&["add", "."]);
    run_git(&["commit", "-m", "init"]);

    let db_path = temp.path().join("session.db");
    let engine = InProcessEngine::open_with_config(&db_path, repo_root.clone(), None)
        .expect("open");
    let port = EngineClientPort::new(engine);

    let session = port.hydrate_session().await.expect("hydrate");
    let main_id = session.active_thread_id.clone();

    let branch = port
        .fork_branch(&main_id, Some("wt-port"))
        .await
        .expect("fork");
    let wt_path = port
        .ensure_thread_worktree(&branch.id)
        .await
        .expect("ensure_thread_worktree");
    assert!(std::path::Path::new(&wt_path).join(".git").exists());

    let term = port
        .create_terminal(Some(&branch.id), None, None, 24, 100)
        .await
        .expect("create_terminal");
    assert_eq!(term.title, "Terminal");

    port.pty_resize(&term.id, 30, 240)
        .await
        .expect("pty_resize");
    port.pty_write(&term.id, b"printf '\\nPORT_%s\\n' \"$(pwd)\"\n".to_vec())
        .await
        .expect("pty_write");

    let expected = format!("PORT_{wt_path}");
    let mut saw_output = false;
    let mut last_screen = String::new();
    for _ in 0..150 {
        let snap = port
            .pty_snapshot(&term.id, 30, 240)
            .await
            .expect("pty_snapshot");
        assert_eq!(snap.rows, 30);
        assert_eq!(snap.cols, 240);
        last_screen = snap.plain_text();
        if last_screen.lines().any(|l| l.trim() == expected) {
            saw_output = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(
        saw_output,
        "EngineClientPort must drive PTY in worktree cwd; last screen:\n{last_screen}"
    );
}

#[tokio::test]
async fn test_engine_client_port_create_terminal_non_live_emits_metadata_before_created() {
    let temp = tempfile::tempdir().expect("tempdir");
    let work_dir = temp.path().join("term_port_dir");
    std::fs::create_dir_all(&work_dir).expect("mkdir");
    let canonical_dir = std::fs::canonicalize(&work_dir).expect("canonical");
    let db_path = temp.path().join("port_order.db");

    let engine =
        InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None)
            .expect("open");
    let port = EngineClientPort::new(engine);

    let session = port.hydrate_session().await.expect("hydrate");
    let mut rx = port.events_rx.lock().await.take().expect("events_rx");
    while rx.try_recv().is_ok() {}

    let term = port
        .create_terminal(
            Some(&session.active_thread_id),
            Some("Offline Term"),
            None,
            24,
            80,
        )
        .await
        .expect("create_terminal");

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }

    let meta_idx = events
        .iter()
        .position(|ev| {
            matches!(
                ev,
                TuiEngineEvent::ThreadMetadataUpdated { meta }
                    if meta.iter().any(|m| {
                        m.thread_id == term.id
                            && m.kind == grein_core::model::ThreadKind::Terminal
                            && m.cwd.as_deref() == Some(canonical_dir.to_string_lossy().as_ref())
                    })
            )
        })
        .expect("expected ThreadMetadataUpdated containing new terminal meta");
    let created_idx = events
        .iter()
        .position(|ev| {
            matches!(
                ev,
                TuiEngineEvent::ThreadCreated { thread } if thread.id == term.id
            )
        })
        .expect("expected ThreadCreated for terminal");

    assert!(
        meta_idx < created_idx,
        "non-live EngineClientPort ThreadMetadataUpdated (idx {meta_idx}) must precede ThreadCreated (idx {created_idx}): {events:?}"
    );
}

#[tokio::test]
async fn test_engine_client_port_pty_scroll_and_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let work_dir = temp.path().join("port_pty_ws");
    std::fs::create_dir_all(&work_dir).expect("mkdir");
    let canonical_dir = std::fs::canonicalize(&work_dir).expect("canonical");
    let db_path = temp.path().join("port_pty.db");

    let engine =
        InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None).expect("open");
    let port = EngineClientPort::new(engine);

    let session = port.hydrate_session().await.expect("hydrate");
    let mut rx = port.subscribe_events().await.expect("subscribe");
    let term = port
        .create_terminal(Some(&session.active_thread_id), Some("Port Term"), None, 24, 80)
        .await
        .expect("create_terminal");

    port.pty_write(
        &term.id,
        b"printf '\\n'; i=1; while [ \"$i\" -le 50 ]; do printf \"PORT_L_%02d\\n\" \"$i\"; i=$((i+1)); done; printf \"PORT_%s\" \"READY\"; read -r grein_pause; exit 7\n".to_vec(),
    )
    .await
    .expect("pty_write");

    let mut saw_50 = false;
    for _ in 0..150 {
        let snap = port.pty_snapshot(&term.id, 24, 80).await.expect("snapshot");
        let text = snap.plain_text();
        if text.lines().any(|l| l.trim() == "PORT_L_50")
            && text.lines().any(|l| l.trim() == "PORT_READY")
        {
            saw_50 = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(saw_50, "expected PORT_L_50 in EngineClientPort snapshot");

    let scrolled = port.pty_scroll(&term.id, 10).await.expect("pty_scroll");
    assert_eq!(scrolled.scrollback_offset, 10);
    let restored = port.pty_scroll(&term.id, -10).await.expect("pty_scroll -10");
    assert_eq!(restored.scrollback_offset, 0);

    port.pty_write(&term.id, b"\n".to_vec())
        .await
        .expect("exit 7");
    let mut saw_exit = false;
    for _ in 0..150 {
        let snap = port.pty_snapshot(&term.id, 24, 80).await.expect("snapshot exited");
        if snap.exited && snap.exit_code == Some(7) {
            saw_exit = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(saw_exit, "expected exit code 7 before restart");

    while rx.try_recv().is_ok() {}
    let restarted = port.pty_restart(&term.id, 26, 96).await.expect("pty_restart");
    assert!(!restarted.exited);
    assert_eq!(restarted.exit_code, None);
    assert_eq!((restarted.rows, restarted.cols), (26, 96));

    let mut saw_restart_event = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(TuiEngineEvent::PtyScreenUpdated { thread_id, snapshot })) =
            tokio::time::timeout(Duration::from_millis(150), rx.recv()).await
        {
            if thread_id == term.id
                && !snapshot.exited
                && snapshot.exit_code.is_none()
                && snapshot.rows == 26
                && snapshot.cols == 96
            {
                saw_restart_event = true;
                break;
            }
        }
    }
    assert!(
        saw_restart_event,
        "EngineClientPort::pty_restart must forward PtyScreenUpdated"
    );
}
