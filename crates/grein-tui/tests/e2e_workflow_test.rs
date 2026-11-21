use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, AppAction, FocusedPane};
use grein_tui::engine_port::{
    HydratedBlackboardEntry, HydratedSession, HydratedThread, HydratedTurn, TuiEngineEvent,
    TuiEnginePort,
};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, draw_frame_mut, handle_event, run_with_engine_events,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;

#[derive(Debug, Default)]
struct MockEngineState {
    threads: Vec<HydratedThread>,
    active_thread_id: String,
    blackboard: BTreeMap<String, String>,
    recorded_turns: Vec<(String, String)>,
    recorded_blackboard_syncs: Vec<(String, String)>,
    resolved_permissions: Vec<(String, bool)>,
    cancelled_threads: Vec<String>,
    shell_commands: Vec<(String, String)>,
    branch_counter: usize,
    completed_turns: usize,
    event_tx: Option<mpsc::UnboundedSender<TuiEngineEvent>>,
    slow_turn_cancel: Option<Arc<Notify>>,
    turn_completed_notify: Arc<Notify>,
}

struct MockLiveEngine {
    state: Mutex<MockEngineState>,
}

impl MockLiveEngine {
    fn new_with_trunk(trunk_id: &str, title: &str) -> Arc<Self> {
        let state = MockEngineState {
            threads: vec![HydratedThread {
                id: trunk_id.to_string(),
                title: title.to_string(),
                turns: Vec::new(),
            }],
            active_thread_id: trunk_id.to_string(),
            turn_completed_notify: Arc::new(Notify::new()),
            ..MockEngineState::default()
        };
        Arc::new(Self {
            state: Mutex::new(state),
        })
    }

    fn emit(&self, event: TuiEngineEvent) {
        if let Some(tx) = self.state.lock().unwrap().event_tx.as_ref() {
            let _ = tx.send(event);
        }
    }

    fn snapshot_blackboard(&self) -> BTreeMap<String, String> {
        self.state.lock().unwrap().blackboard.clone()
    }

    fn recorded_turns(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().recorded_turns.clone()
    }

    fn recorded_blackboard_syncs(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().recorded_blackboard_syncs.clone()
    }

    fn resolved_permissions(&self) -> Vec<(String, bool)> {
        self.state.lock().unwrap().resolved_permissions.clone()
    }

    fn cancelled_threads(&self) -> Vec<String> {
        self.state.lock().unwrap().cancelled_threads.clone()
    }

    fn shell_commands(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().shell_commands.clone()
    }

    fn completed_turns_count(&self) -> usize {
        self.state.lock().unwrap().completed_turns
    }
}

#[async_trait::async_trait]
impl TuiEnginePort for MockLiveEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let guard = self.state.lock().unwrap();
        let blackboard = guard
            .blackboard
            .iter()
            .map(|(k, v)| HydratedBlackboardEntry {
                key: k.clone(),
                value: v.clone(),
            })
            .collect();
        Ok(HydratedSession {
            active_thread_id: guard.active_thread_id.clone(),
            threads: guard.threads.clone(),
            blackboard,
        })
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        let (tx, slow_notify, wb_snapshot) = {
            let mut guard = self.state.lock().unwrap();
            guard
                .recorded_turns
                .push((thread_id.to_string(), prompt.to_string()));
            let wb_md = guard
                .blackboard
                .iter()
                .map(|(k, v)| format!("- {k}: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            guard
                .recorded_blackboard_syncs
                .push((thread_id.to_string(), wb_md.clone()));
            if let Some(thread) = guard.threads.iter_mut().find(|t| t.id == thread_id) {
                thread.turns.push(HydratedTurn {
                    id: format!("u-{}", thread.turns.len() + 1),
                    role: "User".to_string(),
                    content: prompt.to_string(),
                });
            }
            let notify = if prompt.contains("SLOW_TURN") {
                let n = Arc::new(Notify::new());
                guard.slow_turn_cancel = Some(Arc::clone(&n));
                Some(n)
            } else {
                None
            };
            (guard.event_tx.clone(), notify, wb_md)
        };

        let Some(tx) = tx else {
            return Ok(());
        };

        let _ = tx.send(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "streaming".to_string(),
        });

        if let Some(cancel_wait) = slow_turn_cancel_or_none(slow_notify) {
            let _ = tx.send(TuiEngineEvent::StreamChunk {
                thread_id: thread_id.to_string(),
                content: "Streaming ".to_string(),
            });
            cancel_wait.notified().await;
            let _ = tx.send(TuiEngineEvent::TurnCompleted {
                thread_id: thread_id.to_string(),
            });
            let mut guard = self.state.lock().unwrap();
            guard.completed_turns += 1;
            guard.turn_completed_notify.notify_waiters();
            return Ok(());
        }

        let reply = if wb_snapshot.is_empty() {
            format!("Reply({thread_id}): {prompt}")
        } else {
            format!("Reply({thread_id}): {prompt} [WB:{wb_snapshot}]")
        };

        // Deliver token-by-token StreamChunk slices to verify seamless concatenation.
        for chunk in reply.as_bytes().chunks(3) {
            let piece = String::from_utf8_lossy(chunk).into_owned();
            let _ = tx.send(TuiEngineEvent::StreamChunk {
                thread_id: thread_id.to_string(),
                content: piece,
            });
        }

        {
            let mut guard = self.state.lock().unwrap();
            if let Some(thread) = guard.threads.iter_mut().find(|t| t.id == thread_id) {
                thread.turns.push(HydratedTurn {
                    id: format!("a-{}", thread.turns.len() + 1),
                    role: "Assistant".to_string(),
                    content: reply,
                });
            }
            guard.completed_turns += 1;
            guard.turn_completed_notify.notify_waiters();
        }

        let _ = tx.send(TuiEngineEvent::TurnCompleted {
            thread_id: thread_id.to_string(),
        });
        Ok(())
    }

    async fn fork_thread(&self, _thread_id: &str, title: &str) -> io::Result<String> {
        let mut guard = self.state.lock().unwrap();
        guard.branch_counter += 1;
        let id = format!("t-branch-{}", guard.branch_counter);
        let resolved_title = if title.trim().is_empty() {
            format!("Branch {}", guard.branch_counter)
        } else {
            title.to_string()
        };
        guard.threads.push(HydratedThread {
            id: id.clone(),
            title: resolved_title,
            turns: Vec::new(),
        });
        guard.active_thread_id = id.clone();
        Ok(id)
    }

    async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
        Ok(())
    }

    async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .resolved_permissions
            .push((request_id.to_string(), approved));
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state.lock().unwrap().event_tx = Some(tx);
        Ok(rx)
    }

    async fn cancel_turn(&self, thread_id: &str) -> io::Result<()> {
        let notify = {
            let mut guard = self.state.lock().unwrap();
            guard.cancelled_threads.push(thread_id.to_string());
            guard.slow_turn_cancel.take()
        };
        if let Some(n) = notify {
            n.notify_waiters();
        }
        Ok(())
    }

    async fn upsert_blackboard(&self, key: &str, value: &str) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .blackboard
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn delete_blackboard(&self, key: &str) -> io::Result<()> {
        self.state.lock().unwrap().blackboard.remove(key);
        Ok(())
    }

    async fn run_shell_command(&self, thread_id: &str, command: &str) -> io::Result<()> {
        let tx = {
            let mut guard = self.state.lock().unwrap();
            guard
                .shell_commands
                .push((thread_id.to_string(), command.to_string()));
            guard.event_tx.clone()
        };
        if let Some(tx) = tx {
            let stdout = if command == "echo hi" {
                "hi\n".to_string()
            } else {
                format!("executed: {command}\n")
            };
            let _ = tx.send(TuiEngineEvent::ShellOutput {
                thread_id: thread_id.to_string(),
                command: command.to_string(),
                stdout,
                stderr: String::new(),
                exit_code: 0,
            });
        }
        Ok(())
    }
}

fn slow_turn_cancel_or_none(opt: Option<Arc<Notify>>) -> Option<Arc<Notify>> {
    opt
}

fn send_text(tx: &mpsc::UnboundedSender<Event>, text: &str) {
    for ch in text.chars() {
        tx.send(Event::Key(KeyEvent::new(
            KeyCode::Char(ch),
            KeyModifiers::NONE,
        )))
        .unwrap();
    }
}

fn send_key(tx: &mpsc::UnboundedSender<Event>, code: KeyCode, modifiers: KeyModifiers) {
    tx.send(Event::Key(KeyEvent::new(code, modifiers)))
        .unwrap();
}

fn render_to_string(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut screen = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            screen.push_str(buf[(x, y)].symbol());
        }
        screen.push('\n');
    }
    screen
}

async fn wait_for<F>(desc: &str, mut cond: F)
where
    F: FnMut() -> bool,
{
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!("timed out waiting for condition: {desc}");
}

#[tokio::test]
async fn test_e2e_run_with_engine_full_interactive_workflow() {
    let engine = MockLiveEngine::new_with_trunk("t-trunk", "Trunk");
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let backend = TestBackend::new(120, 34);
    let mut terminal = Terminal::new(backend).unwrap();
    let app = App::new();

    // 1. Composer-first startup verified before event loop starts
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner_handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, app, engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    // Wait until subscribe_events has attached
    wait_for("engine subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    // 2. Typing directly in Composer submits prompt & streams token-by-token without newline splitting
    send_text(&key_tx, "trunk hello");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("trunk turn recorded", || {
        engine
            .recorded_turns()
            .iter()
            .any(|(tid, p)| tid == "t-trunk" && p == "trunk hello")
    })
    .await;

    // 3. Editable Whiteboard: /wb set api_port 8080 persists across turns
    send_text(&key_tx, "/wb set api_port 8080");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("whiteboard api_port stored", || {
        engine.snapshot_blackboard().get("api_port").map(String::as_str) == Some("8080")
    })
    .await;

    // Send another turn to prove whiteboard state syncs with turns
    send_text(&key_tx, "sync check");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("sync check recorded", || {
        engine
            .recorded_blackboard_syncs()
            .iter()
            .any(|(_, wb)| wb.contains("api_port: 8080"))
    })
    .await;

    // 4. Shell escape !echo hi emits ShellOutput
    send_text(&key_tx, "!echo hi");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("shell command recorded", || {
        engine
            .shell_commands()
            .iter()
            .any(|(tid, cmd)| tid == "t-trunk" && cmd == "echo hi")
    })
    .await;

    // 5. Branching via Ctrl+N and /branch bugfix
    send_key(&key_tx, KeyCode::Char('n'), KeyModifiers::CONTROL);
    wait_for("ctrl+n branch created", || {
        engine.state.lock().unwrap().threads.len() == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_text(&key_tx, "/branch bugfix");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("/branch bugfix created", || {
        engine.state.lock().unwrap().threads.len() == 3
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Send prompt on bugfix branch
    send_text(&key_tx, "fix bug");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("bugfix turn recorded", || {
        engine
            .recorded_turns()
            .iter()
            .any(|(tid, p)| tid == "t-branch-2" && p == "fix bug")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 6. PermissionRequested popup -> 'y' approves, second popup -> 'n' denies
    engine.emit(TuiEngineEvent::PermissionRequested {
        request_id: "perm-1".to_string(),
        description: "Allow run_command: echo ok?".to_string(),
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    send_key(&key_tx, KeyCode::Char('y'), KeyModifiers::NONE);
    wait_for("perm-1 approved", || {
        engine
            .resolved_permissions()
            .contains(&("perm-1".to_string(), true))
    })
    .await;

    engine.emit(TuiEngineEvent::PermissionRequested {
        request_id: "perm-2".to_string(),
        description: "Allow write_file: secret?".to_string(),
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    send_key(&key_tx, KeyCode::Char('n'), KeyModifiers::NONE);
    wait_for("perm-2 denied", || {
        engine
            .resolved_permissions()
            .contains(&("perm-2".to_string(), false))
    })
    .await;

    // 7. Esc during streaming emits cancel_turn
    send_text(&key_tx, "SLOW_TURN");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("slow turn started", || {
        engine.state.lock().unwrap().slow_turn_cancel.is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    wait_for("cancel_turn emitted", || {
        engine
            .cancelled_threads()
            .contains(&"t-branch-2".to_string())
    })
    .await;
    wait_for("slow turn completed after cancel", || {
        engine.completed_turns_count() >= 4
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 7b. Tree Esc + Up + Enter switches back to Trunk, submits turn, then switches back to bugfix.
    // AnchorParser indexes both "# User: ..." and "## Assistant" headings as anchors:
    // t-trunk (0) + 4 anchors (1..=4) + t-branch-1 (5) + t-branch-2 (6).
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    for _ in 0..6 {
        send_key(&key_tx, KeyCode::Up, KeyModifiers::NONE);
    }
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    send_text(&key_tx, "back on trunk");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("turn recorded on t-trunk after Tree Enter", || {
        engine
            .recorded_turns()
            .iter()
            .any(|(tid, p)| tid == "t-trunk" && p == "back on trunk")
    })
    .await;
    wait_for("trunk turn completed", || {
        engine.completed_turns_count() >= 5
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // t-trunk now has 6 anchors (1..=6), t-branch-1 is at 7, t-branch-2 is at 8.
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    for _ in 0..8 {
        send_key(&key_tx, KeyCode::Down, KeyModifiers::NONE);
    }
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    send_text(&key_tx, "back on bugfix");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("turn recorded on t-branch-2 after Tree Enter", || {
        engine
            .recorded_turns()
            .iter()
            .any(|(tid, p)| tid == "t-branch-2" && p == "back on bugfix")
    })
    .await;
    wait_for("bugfix return turn completed", || {
        engine.completed_turns_count() >= 6
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 8. Pane 3 (Blackboard) 'd' deletion
    send_key(&key_tx, KeyCode::Tab, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, KeyCode::Char('d'), KeyModifiers::NONE);
    wait_for("api_port deleted from blackboard", || {
        !engine.snapshot_blackboard().contains_key("api_port")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 9. Quit cleanly via Ctrl+Q and inspect rendered terminal buffer
    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), runner_handle)
        .await
        .expect("event loop should exit on Ctrl+Q")
        .unwrap()
        .unwrap();

    let screen = render_to_string(&terminal);
    // Token-by-token reply on bugfix concatenated cleanly without newline splits,
    // and trunk turn "back on trunk" stays isolated from active bugfix transcript.
    assert!(
        screen.contains("Reply(t-branch-2): fix bug"),
        "expected streamed bugfix reply on screen:\n{screen}"
    );
    assert!(
        screen.contains("Reply(t-branch-2): back on bugfix"),
        "expected switched bugfix reply on screen:\n{screen}"
    );
    assert!(
        !screen.contains("Reply(t-trunk): back on trunk"),
        "trunk transcript must remain isolated from bugfix:\n{screen}"
    );
    assert!(
        screen.contains("bugfix"),
        "expected bugfix branch in Tree pane:\n{screen}"
    );
    assert!(
        screen.contains("No shared facts yet."),
        "deleted blackboard entry should leave Blackboard empty:\n{screen}"
    );
    assert!(
        !screen.contains("• api_port: 8080"),
        "deleted blackboard bullet should no longer be visible:\n{screen}"
    );

    // Re-hydrating confirms deleted key stays deleted and turns persisted on engine
    let rehydrated = engine.hydrate_session().await.unwrap();
    assert!(rehydrated.blackboard.is_empty());
    assert_eq!(rehydrated.threads.len(), 3);
}

#[tokio::test]
async fn test_e2e_stepped_workflow_composer_streaming_branch_isolation_whiteboard_shell_permission() {
    let engine = MockLiveEngine::new_with_trunk("t-trunk", "Trunk");
    let mut rx = engine.subscribe_events().await.unwrap();
    let session = engine.hydrate_session().await.unwrap();

    let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
    let mut app = App::new();
    apply_hydrated_session(&mut app, &session);

    // 1. Composer-first startup: typing directly populates input_buffer without Tab
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    for ch in "hello".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        )
        .unwrap();
    }
    assert_eq!(app.input_buffer, "hello");
    assert_eq!(app.composer_content(), "hello");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_to_string(&terminal).contains("hello"));

    // 2. Live token-by-token StreamChunk delivery concatenates smoothly without '\n' splitting
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SendPrompt {
            thread_id: "t-trunk".to_string(),
            prompt: "hello".to_string(),
        })
    );
    for token in ["To", "ken", "s ", "concat", "enate", " smoothly"] {
        apply_engine_event(
            &mut app,
            TuiEngineEvent::StreamChunk {
                thread_id: "t-trunk".to_string(),
                content: token.to_string(),
            },
        );
    }
    apply_engine_event(
        &mut app,
        TuiEngineEvent::TurnCompleted {
            thread_id: "t-trunk".to_string(),
        },
    );
    assert!(app.active_transcript_md.contains("Tokens concatenate smoothly"));
    assert_eq!(app.active_transcript_md.matches("## Assistant").count(), 1);
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_to_string(&terminal).contains("Tokens concatenate smoothly"));

    // 3. Editable Whiteboard: /wb set api_port 8080 + persistence across turn sync + Pane 3 'd' delete
    app.input_buffer = "/wb set api_port 8080".to_string();
    app.cursor_position = app.input_buffer.len();
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::UpsertBlackboard {
            key: "api_port".to_string(),
            value: "8080".to_string(),
        })
    );
    engine.upsert_blackboard("api_port", "8080").await.unwrap();
    apply_engine_event(
        &mut app,
        TuiEngineEvent::BlackboardUpdated {
            key: "api_port".to_string(),
            value: "8080".to_string(),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_to_string(&terminal).contains("api_port: 8080"));

    // Simulated turn sync preserves whiteboard entries in both app and engine
    engine.send_turn("t-trunk", "check wb").await.unwrap();
    while let Ok(ev) = rx.try_recv() {
        apply_engine_event(&mut app, ev);
    }
    assert!(app.blackboard_entries.iter().any(|e| e.key == "api_port" && e.value == "8080"));
    assert!(
        engine
            .recorded_blackboard_syncs()
            .iter()
            .any(|(_, wb)| wb.contains("- api_port: 8080"))
    );

    // Re-hydrating a fresh App from engine session still retains api_port: 8080
    let synced_session = engine.hydrate_session().await.unwrap();
    let mut fresh_app = App::new();
    apply_hydrated_session(&mut fresh_app, &synced_session);
    assert!(fresh_app
        .blackboard_entries
        .iter()
        .any(|e| e.key == "api_port" && e.value == "8080"));

    // 4. Shell escape !echo hi emits ShellOutput block into transcript
    app.input_buffer = "!echo hi".to_string();
    app.cursor_position = app.input_buffer.len();
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::RunShell {
            thread_id: "t-trunk".to_string(),
            command: "echo hi".to_string(),
        })
    );
    engine.run_shell_command("t-trunk", "echo hi").await.unwrap();
    while let Ok(ev) = rx.try_recv() {
        apply_engine_event(&mut app, ev);
    }
    assert!(app
        .active_transcript_md
        .contains("$ echo hi (exit 0)\n```text\nhi\n```"));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_to_string(&terminal).contains("$ echo hi (exit 0)"));

    // 5. Branching via Ctrl+N and /branch bugfix + Tree Enter switching & transcript isolation
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch {
            source_thread_id: "t-trunk".to_string(),
            title: None,
        })
    );
    let b1 = engine.fork_branch("t-trunk", None).await.unwrap();
    apply_engine_event(&mut app, TuiEngineEvent::ThreadCreated { thread: b1.clone() });
    assert_eq!(app.active_thread_id(), b1.id);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    app.input_buffer = "/branch bugfix".to_string();
    app.cursor_position = app.input_buffer.len();
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch {
            source_thread_id: b1.id.clone(),
            title: Some("bugfix".to_string()),
        })
    );
    let b2 = engine.fork_branch(&b1.id, Some("bugfix")).await.unwrap();
    apply_engine_event(&mut app, TuiEngineEvent::ThreadCreated { thread: b2.clone() });
    assert_eq!(app.active_thread_id(), b2.id);

    // Stream content exclusively into bugfix branch
    apply_engine_event(
        &mut app,
        TuiEngineEvent::StreamChunk {
            thread_id: b2.id.clone(),
            content: "BUGFIX_ISOLATED".to_string(),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::TurnCompleted {
            thread_id: b2.id.clone(),
        },
    );
    assert!(app.active_transcript_md.contains("BUGFIX_ISOLATED"));
    assert!(!app.active_transcript_md.contains("Tokens concatenate smoothly"));

    // Switch back to Trunk via Esc (idle Composer -> Tree) + Up navigation + Enter
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    // Move Up until Tree selection lands on Thread(t-trunk)
    for _ in 0..app.tree.nodes.len() {
        if let Some(idx) = app.tree.selected_index() {
            if matches!(
                app.tree.nodes.get(idx),
                Some(grein_tui::ui::tree::TreeNode::Thread(t)) if t.id == "t-trunk"
            ) {
                break;
            }
        }
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        )
        .unwrap();
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(app.active_thread_id(), "t-trunk");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert!(app.active_transcript_md.contains("Tokens concatenate smoothly"));
    assert!(!app.active_transcript_md.contains("BUGFIX_ISOLATED"));

    // Switch back to bugfix branch via Tree Enter and confirm isolation in both directions
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    )
    .unwrap();
    for _ in 0..app.tree.nodes.len() {
        if let Some(idx) = app.tree.selected_index() {
            if matches!(
                app.tree.nodes.get(idx),
                Some(grein_tui::ui::tree::TreeNode::Thread(t)) if t.id == b2.id
            ) {
                break;
            }
        }
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        )
        .unwrap();
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(app.active_thread_id(), b2.id);
    assert!(app.active_transcript_md.contains("BUGFIX_ISOLATED"));
    assert!(!app.active_transcript_md.contains("Tokens concatenate smoothly"));

    // 6. Pane 3 (Blackboard) deletion via 'd'
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(app.focused_pane(), FocusedPane::Blackboard);
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::DeleteBlackboard {
            key: "api_port".to_string(),
        })
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::BlackboardDeleted {
            key: "api_port".to_string(),
        },
    );
    assert!(app.blackboard_entries.is_empty());

    // 7. PermissionRequested popup renders visibly -> 'y' approves, 'n' denies
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PermissionRequested {
            request_id: "perm-a".to_string(),
            description: "Run tool echo ok?".to_string(),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let popup_screen = render_to_string(&terminal);
    assert!(popup_screen.contains("Permission Required"));
    assert!(popup_screen.contains("Run tool echo ok?"));
    assert!(popup_screen.contains("[y] Allow  [n] Deny"));

    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "perm-a".to_string(),
            approved: true,
        })
    );
    assert!(app.pending_permission.is_none());

    apply_engine_event(
        &mut app,
        TuiEngineEvent::PermissionRequested {
            request_id: "perm-b".to_string(),
            description: "Write sensitive file?".to_string(),
        },
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "perm-b".to_string(),
            approved: false,
        })
    );
    assert!(app.pending_permission.is_none());

    // 8. Esc during "streaming" emits CancelTurn
    apply_engine_event(
        &mut app,
        TuiEngineEvent::StreamChunk {
            thread_id: b2.id.clone(),
            content: "partial".to_string(),
        },
    );
    assert_eq!(
        app.thread_statuses.get(&b2.id).map(String::as_str),
        Some("streaming")
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    )
    .unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CancelTurn {
            thread_id: b2.id.clone(),
        })
    );
}
