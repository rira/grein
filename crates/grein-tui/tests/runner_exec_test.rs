use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use grein_core::model::{AuthorRole, ThreadId};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::app::App;
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, HydratedTurn, TuiEngineEvent, TuiEnginePort,
};
use tokio::sync::mpsc;

/// Plain test double: records dispatched prompts, replays a scripted event stream
/// and hydrates from a fixed session. No shell execution, no mockall.
struct RecordingEngine {
    session: HydratedSession,
    sent: Mutex<Vec<(String, String)>>,
    scripted_events: Mutex<Option<Vec<TuiEngineEvent>>>,
}

impl RecordingEngine {
    fn new(session: HydratedSession, events: Vec<TuiEngineEvent>) -> Self {
        Self {
            session,
            sent: Mutex::new(Vec::new()),
            scripted_events: Mutex::new(Some(events)),
        }
    }

    fn sent_turns(&self) -> Vec<(String, String)> {
        self.sent.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl TuiEnginePort for RecordingEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        Ok(self.session.clone())
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        self.sent
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?
            .push((thread_id.to_string(), prompt.to_string()));
        Ok(())
    }

    async fn fork_thread(&self, thread_id: &str, _anchor_id: &str) -> io::Result<String> {
        Ok(format!("{thread_id}-fork"))
    }

    async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
        Ok(())
    }

    async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let events = self
            .scripted_events
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?
            .take()
            .unwrap_or_default();
        let (tx, rx) = mpsc::unbounded_channel();
        for event in events {
            tx.send(event)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        Ok(rx)
    }
}

/// Engine that hydrates from a real SqliteStorage file (no in-memory fakes).
struct SqliteBackedEngine {
    db_path: PathBuf,
}

#[async_trait::async_trait]
impl TuiEnginePort for SqliteBackedEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let storage =
            SqliteStorage::open(&self.db_path).map_err(|e| io::Error::other(format!("{e:?}")))?;
        let tid_entry = storage
            .get_blackboard_entry("active_thread_id")
            .map_err(|e| io::Error::other(format!("{e:?}")))?
            .ok_or_else(|| io::Error::other("missing active_thread_id"))?;
        let tid: ThreadId = tid_entry
            .value
            .parse()
            .ok()
            .map(ThreadId)
            .ok_or_else(|| io::Error::other("bad thread id"))?;
        let thread = storage
            .get_thread(tid)
            .map_err(|e| io::Error::other(format!("{e:?}")))?
            .ok_or_else(|| io::Error::other("thread row missing"))?;
        let trunk = storage
            .get_trunk_path(thread.active_leaf_id)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let turns = trunk
            .iter()
            .enumerate()
            .map(|(i, node)| HydratedTurn {
                id: format!("turn-{i}"),
                role: format!("{:?}", node.role),
                content: String::new(),
            })
            .collect();
        Ok(HydratedSession {
            active_thread_id: tid_entry.value.clone(),
            threads: vec![HydratedThread {
                id: tid_entry.value.clone(),
                title: "persisted".to_string(),
                turns,
                              custom_name: None,
            }],
        })
    }

    async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
        Ok(())
    }

    async fn fork_thread(&self, thread_id: &str, _anchor_id: &str) -> io::Result<String> {
        Ok(thread_id.to_string())
    }

    async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
        Ok(())
    }

    async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }
}

fn sample_session() -> HydratedSession {
    HydratedSession {
        active_thread_id: "t-1".to_string(),
        threads: vec![HydratedThread {
            id: "t-1".to_string(),
            title: "Main".to_string(),
            turns: vec![
                HydratedTurn {
                    id: "u1".to_string(),
                    role: "User".to_string(),
                    content: "hello".to_string(),
                },
                HydratedTurn {
                    id: "a1".to_string(),
                    role: "Assistant".to_string(),
                    content: "world".to_string(),
                },
            ],
                          custom_name: None,
        }],
    }
}

#[tokio::test]
async fn test_engine_port_prompt_dispatch_records_thread_and_prompt() {
    let engine = Arc::new(RecordingEngine::new(sample_session(), Vec::new()));
    let port: Arc<dyn TuiEnginePort> = engine.clone();

    let session = port.hydrate_session().await.unwrap();
    assert_eq!(session.active_thread_id, "t-1");
    assert_eq!(session.threads.len(), 1);
    assert_eq!(session.threads[0].title, "Main");
    assert_eq!(session.threads[0].turns[1].role, "Assistant");
    assert_eq!(session.threads[0].turns[1].content, "world");

    port.send_turn(&session.active_thread_id, "echo first")
        .await
        .unwrap();
    port.send_turn(&session.active_thread_id, "second")
        .await
        .unwrap();

    assert_eq!(
        engine.sent_turns(),
        vec![
            ("t-1".to_string(), "echo first".to_string()),
            ("t-1".to_string(), "second".to_string()),
        ]
    );
    let forked = port.fork_thread("t-1", "u1").await.unwrap();
    assert_eq!(forked, "t-1-fork");
}

#[tokio::test]
async fn test_engine_port_streaming_events_ingested_into_app_transcript() {
    let engine = RecordingEngine::new(
        sample_session(),
        vec![
            TuiEngineEvent::StreamChunk {
                thread_id: "t-1".to_string(),
                content: "stream_".to_string(),
            },
            TuiEngineEvent::StreamChunk {
                thread_id: "t-1".to_string(),
                content: "ok".to_string(),
            },
            TuiEngineEvent::TurnCompleted {
                thread_id: "t-1".to_string(),
            },
        ],
    );

    let mut rx = engine.subscribe_events().await.unwrap();
    let mut app = App::new();
    app.submit_composer_prompt("go".to_string());

    let mut buffer = String::new();
    let mut completed = false;
    while let Some(event) = rx.recv().await {
        match event {
            TuiEngineEvent::StreamChunk { thread_id, content } => {
                assert_eq!(thread_id, "t-1");
                buffer.push_str(&content);
            }
            TuiEngineEvent::TurnCompleted { thread_id } => {
                assert_eq!(thread_id, "t-1");
                completed = true;
                app.apply_agent_response(&buffer);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    assert!(completed, "TurnCompleted must terminate the stream");
    assert_eq!(buffer, "stream_ok");
    assert!(app.active_transcript_md.contains("stream_ok"));
    assert_eq!(app.last_submitted_prompt(), Some("go".to_string()));
}

#[tokio::test]
async fn test_engine_port_hydrates_from_real_sqlite_storage() {
    let db_path =
        std::env::temp_dir().join(format!("grein-runner-test-{}.sqlite3", ThreadId::now_v7()));
    let _ = std::fs::remove_file(&db_path);

    let mut app = App::new();
    app.submit_composer_prompt("echo first".to_string());
    app.apply_agent_response("first_output");
    app.persist_state_to_sqlite(&db_path);

    let meta_path = PathBuf::from(format!("{}.meta", db_path.display()));
    assert!(!meta_path.exists(), "Sidecar .meta file must not exist");

    let storage = SqliteStorage::open(&db_path).unwrap();
    let tid_entry = storage
        .get_blackboard_entry("active_thread_id")
        .unwrap()
        .expect("active_thread_id must be persisted in blackboard_entries");
    let tid: ThreadId = tid_entry.value.parse().ok().map(ThreadId).unwrap();
    let thread = storage
        .get_thread(tid)
        .unwrap()
        .expect("Thread row must exist");
    let trunk = storage.get_trunk_path(thread.active_leaf_id).unwrap();
    assert_eq!(trunk.len(), 2);
    assert_eq!(trunk[0].role, AuthorRole::User);
    assert_eq!(trunk[1].role, AuthorRole::Assistant);
    drop(storage);

    let engine = SqliteBackedEngine {
        db_path: db_path.clone(),
    };
    let session = engine.hydrate_session().await.unwrap();
    assert_eq!(session.active_thread_id, tid_entry.value);
    assert_eq!(session.threads.len(), 1);
    let roles: Vec<&str> = session.threads[0]
        .turns
        .iter()
        .map(|t| t.role.as_str())
        .collect();
    assert_eq!(roles, vec!["User", "Assistant"]);

    let mut loaded = App::new();
    loaded.load_state_from_sqlite(&db_path);
    assert_eq!(loaded.threads.len(), 1);
    assert_ne!(loaded.threads[0].id, "thread-1");
    assert!(loaded.active_transcript_md.contains("first_output"));

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_engine_port_default_methods_and_nonblocking_event_loop() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use grein_tui::app::FocusedPane;
    use grein_tui::runner::run_with_engine_events;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tokio::sync::Notify;

    struct SlowBlockingEngine {
        session: HydratedSession,
        unblock_send: Arc<Notify>,
        tx: Mutex<Option<mpsc::UnboundedSender<TuiEngineEvent>>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for SlowBlockingEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(self.session.clone())
        }
        async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("send:{thread_id}:{prompt}"));
            self.unblock_send.notified().await;
            Ok(())
        }
        async fn fork_thread(&self, thread_id: &str, title: &str) -> io::Result<String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("fork:{thread_id}:{title}"));
            Ok("t-branch".to_string())
        }
        async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
            Ok(())
        }
        async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("perm:{request_id}:{approved}"));
            Ok(())
        }
        async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
            let (tx, rx) = mpsc::unbounded_channel();
            *self.tx.lock().unwrap() = Some(tx);
            Ok(rx)
        }
        async fn run_shell_command(&self, thread_id: &str, command: &str) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("shell:{thread_id}:{command}"));
            Err(io::Error::other("shell failed cleanly"))
        }
        async fn cancel_turn(&self, thread_id: &str) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("cancel:{thread_id}"));
            Ok(())
        }
    }

    let unblock = Arc::new(Notify::new());
    let engine = Arc::new(SlowBlockingEngine {
        session: sample_session(),
        unblock_send: Arc::clone(&unblock),
        tx: Mutex::new(None),
        calls: Mutex::new(Vec::new()),
    });

    let branched = engine.fork_branch("t-1", Some("side")).await.unwrap();
    assert_eq!(branched.id, "t-branch");
    assert_eq!(branched.title, "side");

    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    let app = App::new();
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let runner_handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, app, engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    for ch in "hello async".chars() {
        key_tx
            .send(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            )))
            .unwrap();
    }
    key_tx
        .send(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
        .unwrap();

    // Even while send_turn is blocked on Notify, UI keeps accepting commands and receives errors.
    for ch in "!false".chars() {
        key_tx
            .send(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            )))
            .unwrap();
    }
    key_tx
        .send(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
        .unwrap();
    for ch in "/fork side-check".chars() {
        key_tx
            .send(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            )))
            .unwrap();
    }
    key_tx
        .send(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    if let Some(ev_tx) = engine.tx.lock().unwrap().as_ref() {
        let _ = ev_tx.send(TuiEngineEvent::PermissionRequested {
            request_id: "perm-99".to_string(),
            description: "Allow tool?".to_string(),
        });
    }
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    key_tx
        .send(Event::Key(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        )))
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    for ch in "/quit".chars() {
        key_tx
            .send(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            )))
            .unwrap();
    }
    key_tx
        .send(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
        .unwrap();

    let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), runner_handle)
        .await
        .expect("event loop must not block on slow send_turn")
        .unwrap()
        .unwrap();

    unblock.notify_waiters();
    let recorded = engine.calls.lock().unwrap().clone();
    assert!(recorded.contains(&"send:t-1:hello async".to_string()));
    assert!(recorded.contains(&"fork:t-1:side-check".to_string()));
    assert!(recorded.contains(&"shell:t-1:false".to_string()));
    assert!(recorded.contains(&"perm:perm-99:true".to_string()));

    let buf = terminal.backend().buffer();
    let mut screen = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            screen.push_str(buf[(x, y)].symbol());
        }
        screen.push('\n');
    }
    assert!(screen.contains("side-check"));
}

#[test]
fn test_apply_engine_event_routes_branches_streaming_tools_shell_and_ignores_blackboard() {
    use grein_tui::app::FocusedPane;
    use grein_tui::runner::{apply_engine_event, apply_hydrated_session};

    let mut app = App::new();
    apply_hydrated_session(&mut app, &sample_session());
    assert_eq!(app.active_thread_id(), "t-1");

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: "t-2".to_string(),
                title: "Forked".to_string(),
                turns: Vec::new(),
                        custom_name: None,
            },
        },
    );
    assert_eq!(app.active_thread_id(), "t-2");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Background stream arrives on t-1 while t-2 is active.
    apply_engine_event(
        &mut app,
        TuiEngineEvent::StreamChunk {
            thread_id: "t-1".to_string(),
            content: "Hel".to_string(),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::StreamChunk {
            thread_id: "t-1".to_string(),
            content: "lo!".to_string(),
        },
    );
    assert!(!app.active_transcript_md.contains("Hello!"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolInvoked {
            thread_id: "t-1".to_string(),
            tool_name: "read".to_string(),
            tool_call_id: None,
            summary: None,
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolResult {
            thread_id: "t-1".to_string(),
            tool_name: "read".to_string(),
            output: "42 lines".to_string(),
            tool_call_id: None,
            is_error: false,
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::TurnCompleted {
            thread_id: "t-1".to_string(),
        },
    );
    assert!(!app.streaming_assistant_threads.contains("t-1"));
    assert_eq!(
        app.thread_statuses.get("t-1").map(String::as_str),
        Some("idle")
    );

    app.switch_thread("t-1");
    assert!(app.active_transcript_md.contains("\n\n## Assistant\nHello!"));
    assert_eq!(app.active_transcript_md.matches("## Assistant").count(), 1);
    assert!(app.active_transcript_md.contains("read"));
    assert!(app.active_transcript_md.contains("42 lines"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ShellOutput {
            thread_id: "t-1".to_string(),
            command: "echo hi".to_string(),
            stdout: "hi\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        },
    );
    assert!(app
        .active_transcript_md
        .contains("$ echo hi (exit 0)\n```ansi\nhi\n```"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadTitleUpdated {
            thread_id: "t-2".to_string(),
            title: "Renamed Branch".to_string(),
        },
    );
    assert_eq!(app.threads[1].title, "Renamed Branch");

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThoughtChunk {
            thread_id: "t-1".to_string(),
            content: "thinking".to_string(),
        },
    );
    assert!(!app.active_transcript_md.contains("thinking"));
}
