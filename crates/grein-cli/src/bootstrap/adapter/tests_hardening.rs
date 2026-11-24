use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use crate::engine::{
    AgentTurnPort, AutocompleteContextPort, EngineError, EngineEvent, EngineEventPort,
    PermissionDecisionPayload, SessionSnapshot, ShellCommandResult, ShellUseCasePort,
    TerminalPtyUseCasePort, ThreadSummary, ThreadUseCasePort, TranscriptEntry, TurnDispatchResult,
    WorktreeUseCasePort,
};
use grein_core::model::ThreadKind;
use grein_tui::engine_port::{
    AgentConfigOption, AgentOptionsSnapshot, PtyScreenSnapshot, ThreadMeta, TuiEngineEvent,
    TuiEnginePort, WorktreeEntry,
};
use tokio::sync::{mpsc, Mutex, Notify};

use super::EngineClientPort;

#[derive(Default)]
struct MockEngineState {
    fork_calls: Vec<(Option<String>, Option<String>)>,
    last_transcript: Vec<TranscriptEntry>,
    thread_meta: Vec<ThreadMeta>,
    turn_error: Option<String>,
    hold_turn: Option<(Arc<Notify>, Arc<Notify>)>,
}

struct MockPortEngine {
    rx: Mutex<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>>,
    state: StdMutex<MockEngineState>,
}

impl MockPortEngine {
    fn new(rx: Option<mpsc::UnboundedReceiver<TuiEngineEvent>>) -> Self {
        Self {
            rx: Mutex::new(rx),
            state: StdMutex::new(MockEngineState::default()),
        }
    }
}

#[async_trait::async_trait]
impl ThreadUseCasePort for MockPortEngine {
    async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
        Ok(ThreadSummary {
            thread_id: "t-main".into(),
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
        parent_node_id: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.fork_thread_with_title(parent_node_id, None).await
    }
    async fn fork_thread_with_title(
        &self,
        source: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        self.state
            .lock()
            .unwrap()
            .fork_calls
            .push((source.map(str::to_string), title.map(str::to_string)));
        Ok(ThreadSummary {
            thread_id: "t-fork".into(),
            title: "Branch 2".into(),
            custom_name: title.map(str::to_string),
            parent_node_id: "n-1".into(),
            active_leaf_id: "n-1".into(),
            merged: false,
            status: Default::default(),
            agent_provider: None,
        })
    }
    async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
        Ok(ThreadSummary {
            thread_id: "t-root".into(),
            title: "Root 2".into(),
            custom_name: title.map(str::to_string),
            parent_node_id: "n-r".into(),
            active_leaf_id: "n-r".into(),
            merged: false,
            status: Default::default(),
            agent_provider: None,
        })
    }
    async fn rename_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        Ok(ThreadSummary {
            thread_id: thread_id.into(),
            title: "Main".into(),
            custom_name: (!title.is_empty()).then(|| title.into()),
            parent_node_id: "n-1".into(),
            active_leaf_id: "n-1".into(),
            merged: false,
            status: Default::default(),
            agent_provider: None,
        })
    }
    async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        Ok(vec![self.ensure_main_thread().await?])
    }
    async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        Ok(self.state.lock().unwrap().last_transcript.clone())
    }
    async fn get_thread_transcript(
        &self,
        _thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        Ok(self.state.lock().unwrap().last_transcript.clone())
    }
    async fn merge_thread(&self, thread_id: &str) -> Result<ThreadSummary, EngineError> {
        self.merge_thread_with_summary(thread_id, None, None).await
    }
    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        _target_id: Option<&str>,
        _summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        Ok(ThreadSummary {
            thread_id: source_id.into(),
            title: "Branch 2".into(),
            custom_name: None,
            parent_node_id: "n-1".into(),
            active_leaf_id: "n-1".into(),
            merged: true,
            status: Default::default(),
            agent_provider: None,
        })
    }
    async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
        unreachable!()
    }
    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        Ok(self.state.lock().unwrap().thread_meta.clone())
    }
}

#[async_trait::async_trait]
impl AgentTurnPort for MockPortEngine {
    async fn send_turn(&self, _prompt: &str) -> Result<TurnDispatchResult, EngineError> {
        Ok(TurnDispatchResult {
            node_id: "n-turn".into(),
        })
    }
    async fn send_turn_for_thread(
        &self,
        _thread_id: &str,
        _prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        let gate = self.state.lock().unwrap().hold_turn.clone();
        if let Some((started, release)) = gate {
            started.notify_one();
            release.notified().await;
        }
        if let Some(msg) = self.state.lock().unwrap().turn_error.take() {
            return Err(EngineError::Transport(msg));
        }
        Ok(TurnDispatchResult {
            node_id: "n-turn".into(),
        })
    }
    async fn resolve_permission(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        Ok(EngineEvent::PermissionResolved {
            request_id: decision.request_id,
            allow: decision.allow,
        })
    }
    async fn cancel_turn(&self, _thread_id: &str) -> Result<(), EngineError> {
        Ok(())
    }
    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        _set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        Ok(AgentOptionsSnapshot {
            thread_id: thread_id.into(),
            config_options: vec![AgentConfigOption {
                id: config_id.into(),
                name: "Model".into(),
                category: Some("model".into()),
                current_value: value.into(),
                options: Vec::new(),
            }],
            available_commands: Vec::new(),
        })
    }
}

#[async_trait::async_trait]
impl ShellUseCasePort for MockPortEngine {
    async fn run_shell_command(
        &self,
        _thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        Ok(ShellCommandResult {
            command: command.into(),
            stdout: "out".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }
}

#[async_trait::async_trait]
impl TerminalPtyUseCasePort for MockPortEngine {
    async fn pty_scroll(
        &self,
        _thread_id: &str,
        delta: isize,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        Ok(PtyScreenSnapshot {
            rows: 24,
            cols: 80,
            scrollback_offset: delta.max(0) as usize,
            ..Default::default()
        })
    }
    async fn pty_restart(
        &self,
        _thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<PtyScreenSnapshot, EngineError> {
        Ok(PtyScreenSnapshot {
            rows,
            cols,
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl WorktreeUseCasePort for MockPortEngine {
    async fn ensure_thread_worktree(&self, _thread_id: &str) -> Result<String, EngineError> {
        Ok("/tmp/wt-mock".into())
    }
    async fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        Ok(vec![WorktreeEntry {
            repo_root: "/tmp/repo".into(),
            repo_name: "repo".into(),
            path: "/tmp/wt-mock".into(),
            branch: Some("feat".into()),
            head_short: Some("abc".into()),
            is_primary: false,
            is_detached: false,
            is_grein_managed: true,
            dirty_count: 0,
            attached_thread_ids: vec!["t-fork".into()],
        }])
    }
    async fn switch_thread_worktree(
        &self,
        _thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        Ok(target_path.into())
    }
    async fn remove_worktree(&self, _path: &str, _force: bool) -> Result<(), EngineError> {
        Ok(())
    }
    async fn worktree_diff_preview(&self, _path: &str) -> Result<String, EngineError> {
        Ok("diff --git a b".into())
    }
    async fn prune_worktrees(&self) -> Result<String, EngineError> {
        Ok("pruned 0".into())
    }
}

impl AutocompleteContextPort for MockPortEngine {}

#[async_trait::async_trait]
impl EngineEventPort for MockPortEngine {
    async fn ping(&self) -> Result<(), EngineError> {
        Ok(())
    }
    async fn subscribe_events(
        &self,
    ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
        Ok(self.rx.lock().await.take())
    }
}

#[tokio::test]
async fn test_fork_thread_uuid_vs_title_heuristic_and_non_live_events() {
    let engine = MockPortEngine::new(None);
    engine.state.lock().unwrap().thread_meta = vec![ThreadMeta {
        thread_id: "t-fork".into(),
        parent_thread_id: Some("t-main".into()),
        inherited_turn_count: 1,
        merged: false,
        kind: ThreadKind::Agent,
        cwd: None,
        git_branch: None,
        git_dirty: 0,
        custom_name: None,
        agent_provider: None,
    }];
    let port = EngineClientPort::new(engine);

    let uuid_anchor = "01234567-89ab-cdef-0123-456789abcdef";
    let non_hex_36 = "01234567-89ab-cdef-0123-456789abcdeg";

    assert_eq!(port.fork_thread("t-main", "   ").await.unwrap(), "t-fork");
    assert_eq!(
        port.fork_thread("t-main", uuid_anchor).await.unwrap(),
        "t-fork"
    );
    assert_eq!(
        port.fork_thread("t-main", non_hex_36).await.unwrap(),
        "t-fork"
    );
    assert_eq!(
        port.fork_thread("t-main", "auth-topic").await.unwrap(),
        "t-fork"
    );

    let calls = port.client.state.lock().unwrap().fork_calls.clone();
    assert_eq!(
        calls,
        vec![
            (Some("t-main".into()), None),
            (Some(uuid_anchor.into()), None),
            (Some("t-main".into()), Some(non_hex_36.into())),
            (Some("t-main".into()), Some("auth-topic".into())),
        ]
    );

    let mut rx = port.events_rx.lock().await.take().unwrap();
    let mut evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        evs.push(ev);
    }
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, TuiEngineEvent::ThreadCreated { .. }))
            .count(),
        4
    );
    assert!(evs
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::ThreadMetadataUpdated { .. })));
}

#[tokio::test]
async fn test_live_stream_error_dedup_buffer_flush_ttl_and_double_subscribe() {
    let (tx, rx) = mpsc::unbounded_channel();
    let engine = MockPortEngine::new(Some(rx));
    let port = Arc::new(EngineClientPort::new(engine));

    let mut sub_rx = port.subscribe_events().await.expect("first subscribe");
    let second_err = port
        .subscribe_events()
        .await
        .expect_err("second subscribe must fail");
    assert!(second_err.to_string().contains("already subscribed"));

    // 1. Buffer unrelated error during in_flight turn -> flushes when remaining == 0
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    port.client.state.lock().unwrap().hold_turn = Some((Arc::clone(&started), Arc::clone(&release)));

    let p1 = Arc::clone(&port);
    let h1 = tokio::spawn(async move { p1.send_turn("t-main", "hi").await });
    started.notified().await;

    tx.send(TuiEngineEvent::Error {
        message: "unrelated bg warning".into(),
    })
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        sub_rx.try_recv().is_err(),
        "error during in_flight turn must stay buffered"
    );

    release.notify_one();
    h1.await.unwrap().expect("turn succeeds");

    let flushed = tokio::time::timeout(Duration::from_millis(150), sub_rx.recv())
        .await
        .expect("timeout")
        .expect("flushed error");
    assert!(matches!(
        flushed,
        TuiEngineEvent::Error { message } if message == "unrelated bg warning"
    ));

    // 2. Sync error matches buffered event -> consumed cleanly with zero duplicate
    let started2 = Arc::new(Notify::new());
    let release2 = Arc::new(Notify::new());
    {
        let mut st = port.client.state.lock().unwrap();
        st.hold_turn = Some((Arc::clone(&started2), Arc::clone(&release2)));
        st.turn_error = Some("buffered-sync".into());
    }
    let p2 = Arc::clone(&port);
    let h2 = tokio::spawn(async move { p2.send_turn("t-main", "fail").await });
    started2.notified().await;
    tx.send(TuiEngineEvent::Error {
        message: "buffered-sync".into(),
    })
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    release2.notify_one();
    assert!(h2.await.unwrap().is_err());
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        sub_rx.try_recv().is_err(),
        "consumed sync error must not emit duplicate"
    );

    // 3. Suppressed sync error suppressed within 5s, but >5s TTL expired entry is evicted & forwarded
    {
        let mut st = port.client.state.lock().unwrap();
        st.hold_turn = None;
        st.turn_error = Some("fresh-sync".into());
    }
    assert!(port.send_turn("t-main", "fail fast").await.is_err());
    tx.send(TuiEngineEvent::Error {
        message: "fresh-sync".into(),
    })
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        sub_rx.try_recv().is_err(),
        "matching error within 5s TTL must be suppressed"
    );

    // Seed expired >5s entry directly into suppressed_sync_errors -> must be evicted and forwarded
    {
        let mut sup = port.suppressed_sync_errors.lock().unwrap();
        sup.push((
            "transport error: expired-sync".into(),
            Instant::now() - Duration::from_secs(6),
        ));
    }
    tx.send(TuiEngineEvent::Error {
        message: "expired-sync".into(),
    })
    .unwrap();
    let forwarded = tokio::time::timeout(Duration::from_millis(150), sub_rx.recv())
        .await
        .expect("timeout")
        .expect("forwarded after TTL");
    assert!(matches!(
        forwarded,
        TuiEngineEvent::Error { message } if message == "expired-sync"
    ));
}

#[tokio::test]
async fn test_non_live_fallbacks_and_merge_truncation_and_target_resolution() {
    let engine = MockPortEngine::new(None);
    engine.state.lock().unwrap().thread_meta = vec![ThreadMeta {
        thread_id: "t-fork".into(),
        parent_thread_id: Some("t-parent-from-meta".into()),
        inherited_turn_count: 1,
        merged: false,
        kind: ThreadKind::Agent,
        cwd: None,
        git_branch: None,
        git_dirty: 0,
        custom_name: None,
        agent_provider: None,
    }];
    let port = EngineClientPort::new(engine);

    // Non-live send_turn: assistant last entry emits StreamChunk; user last entry skips StreamChunk
    port.client.state.lock().unwrap().last_transcript = vec![TranscriptEntry {
        node_id: "n-a".into(),
        role: "assistant".into(),
        content: "offline assistant reply".into(),
    }];
    port.send_turn("t-main", "p1").await.unwrap();

    port.client.state.lock().unwrap().last_transcript = vec![TranscriptEntry {
        node_id: "n-u".into(),
        role: "user".into(),
        content: "user only".into(),
    }];
    port.send_turn("t-main", "p2").await.unwrap();

    // Non-live PTY, shell, config, and worktree methods
    let scroll_snap = port.pty_scroll("t-term", 5).await.unwrap();
    assert_eq!(scroll_snap.scrollback_offset, 5);
    let restart_snap = port.pty_restart("t-term", 30, 100).await.unwrap();
    assert_eq!((restart_snap.rows, restart_snap.cols), (30, 100));
    port.run_shell_command("t-main", "echo hi").await.unwrap();
    port.set_agent_config("t-main", "model", "m2", false)
        .await
        .unwrap();

    assert_eq!(
        port.ensure_thread_worktree("t-fork").await.unwrap(),
        "/tmp/wt-mock"
    );
    assert_eq!(port.list_worktrees().await.unwrap().len(), 1);
    assert_eq!(
        port.switch_thread_worktree("t-fork", "/tmp/repo")
            .await
            .unwrap(),
        "/tmp/repo"
    );
    assert_eq!(
        port.worktree_diff_preview("/tmp/wt-mock").await.unwrap(),
        "diff --git a b"
    );
    port.remove_worktree("/tmp/wt-mock", true).await.unwrap();
    assert_eq!(port.prune_worktrees().await.unwrap(), "pruned 0");

    // merge_thread_with_summary: >400-char summary truncation & empty-target fallback
    let long_summary = "x".repeat(450);
    port.merge_thread_with_summary("t-fork", "   ", Some(&long_summary))
        .await
        .unwrap();

    let mut rx = port.events_rx.lock().await.take().unwrap();
    let mut evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        evs.push(ev);
    }

    let chunks: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            TuiEngineEvent::StreamChunk { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(chunks, vec!["offline assistant reply"]);
    assert!(evs
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::PtyScreenUpdated { snapshot, .. } if snapshot.scrollback_offset == 5)));
    assert!(evs
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::PtyScreenUpdated { snapshot, .. } if snapshot.rows == 30)));
    assert!(evs
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::ShellOutput { command, .. } if command == "echo hi")));
    assert!(evs
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::AgentConfigUpdated { .. })));

    let merged_ev = evs
        .iter()
        .find_map(|e| match e {
            TuiEngineEvent::ThreadMerged {
                thread_id,
                target_thread_id,
                summary,
            } => Some((thread_id.as_str(), target_thread_id.as_str(), summary.as_str())),
            _ => None,
        })
        .expect("ThreadMerged emitted");
    assert_eq!(merged_ev.0, "t-fork");
    assert_eq!(merged_ev.1, "t-parent-from-meta");
    assert_eq!(merged_ev.2.chars().count(), 400);
    assert!(merged_ev.2.ends_with("..."));
}
