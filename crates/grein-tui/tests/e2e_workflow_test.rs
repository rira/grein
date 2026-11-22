use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, AppAction, FocusedPane};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, HydratedTurn, SubagentSnapshot, SubagentStatus, ThreadMeta,
    TuiEngineEvent, TuiEnginePort,
};
use grein_tui::layout::{compute_layout_with_chrome, subagent_strip_height};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, draw_frame_mut, handle_event, run_with_engine_events,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;

#[derive(Debug, Default)]
struct MockEngineState {
    threads: Vec<HydratedThread>,
    thread_meta: Vec<ThreadMeta>,
    deleted_threads: Vec<String>,
    active_thread_id: String,
    recorded_turns: Vec<(String, String)>,
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
            thread_meta: vec![ThreadMeta {
                thread_id: trunk_id.to_string(),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
            }],
            active_thread_id: trunk_id.to_string(),
            turn_completed_notify: Arc::new(Notify::new()),
            ..MockEngineState::default()
        };
        Arc::new(Self {
            state: Mutex::new(state),
        })
    }

    fn new_with_session(
        active_thread_id: &str,
        threads: Vec<HydratedThread>,
        thread_meta: Vec<ThreadMeta>,
    ) -> Arc<Self> {
        let state = MockEngineState {
            threads,
            thread_meta,
            active_thread_id: active_thread_id.to_string(),
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

    fn recorded_turns(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().recorded_turns.clone()
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

    fn deleted_threads(&self) -> Vec<String> {
        self.state.lock().unwrap().deleted_threads.clone()
    }

    fn threads_count(&self) -> usize {
        self.state.lock().unwrap().threads.len()
    }
}

#[async_trait::async_trait]
impl TuiEnginePort for MockLiveEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let guard = self.state.lock().unwrap();
        Ok(HydratedSession {
            active_thread_id: guard.active_thread_id.clone(),
            threads: guard.threads.clone(),
        })
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        let (tx, slow_notify) = {
            let mut guard = self.state.lock().unwrap();
            guard
                .recorded_turns
                .push((thread_id.to_string(), prompt.to_string()));
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
            (guard.event_tx.clone(), notify)
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

        let reply = if prompt.contains("GFM_TABLE") {
            "### Auth Matrix\n\n| Layer | Role | Status |\n| :--- | :---: | ---: |\n| Tree | context-only | ok |\n| Transcript | `gfm_table` | 100 |".to_string()
        } else {
            format!("Reply({thread_id}): {prompt}")
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

    async fn fork_thread(&self, thread_id: &str, title: &str) -> io::Result<String> {
        let mut guard = self.state.lock().unwrap();
        guard.branch_counter += 1;
        let id = format!("t-branch-{}", guard.branch_counter);
        let resolved_title = if title.trim().is_empty() {
            format!("Branch {}", guard.branch_counter)
        } else {
            title.to_string()
        };
        let inherited_turn_count = guard
            .threads
            .iter()
            .find(|t| t.id == thread_id)
            .map(|t| t.turns.len())
            .unwrap_or(0);
        guard.threads.push(HydratedThread {
            id: id.clone(),
            title: resolved_title,
            turns: Vec::new(),
        });
        guard.thread_meta.push(ThreadMeta {
            thread_id: id.clone(),
            parent_thread_id: Some(thread_id.to_string()),
            inherited_turn_count,
            merged: false,
        });
        guard.active_thread_id = id.clone();
        Ok(id)
    }

    async fn create_root(&self, title: Option<&str>) -> io::Result<HydratedThread> {
        let mut guard = self.state.lock().unwrap();
        guard.branch_counter += 1;
        let id = format!("t-root-{}", guard.branch_counter);
        let resolved_title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Root {}", guard.branch_counter));
        let thread = HydratedThread {
            id: id.clone(),
            title: resolved_title,
            turns: Vec::new(),
        };
        guard.threads.push(thread.clone());
        guard.thread_meta.push(ThreadMeta {
            thread_id: id.clone(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
        });
        guard.active_thread_id = id;
        Ok(thread)
    }

    async fn rename_thread(&self, thread_id: &str, title: &str) -> io::Result<()> {
        let mut guard = self.state.lock().unwrap();
        let thread = guard
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "thread not found"))?;
        thread.title = title.to_string();
        Ok(())
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: &str,
        summary: Option<&str>,
    ) -> io::Result<()> {
        let tx = {
            let mut guard = self.state.lock().unwrap();
            if let Some(meta) = guard.thread_meta.iter_mut().find(|m| m.thread_id == source_id) {
                meta.merged = true;
            }
            guard.event_tx.clone()
        };
        if let Some(tx) = tx {
            let _ = tx.send(TuiEngineEvent::ThreadMerged {
                thread_id: source_id.to_string(),
                target_thread_id: target_id.to_string(),
                summary: summary.unwrap_or("merged summary").to_string(),
            });
        }
        Ok(())
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

    async fn delete_thread(&self, thread_id: &str) -> Result<String, String> {
        let mut guard = self.state.lock().unwrap();
        if guard.threads.len() <= 1 {
            return Err("Cannot delete the only context".to_string());
        }
        let parent_id = guard
            .thread_meta
            .iter()
            .find(|m| m.thread_id == thread_id)
            .and_then(|m| m.parent_thread_id.clone());
        guard.threads.retain(|t| t.id != thread_id);
        guard.thread_meta.retain(|m| m.thread_id != thread_id);
        for meta in &mut guard.thread_meta {
            if meta.parent_thread_id.as_deref() == Some(thread_id) {
                meta.parent_thread_id = parent_id.clone();
            }
        }
        let next_active = parent_id
            .filter(|pid| guard.threads.iter().any(|t| &t.id == pid))
            .unwrap_or_else(|| guard.threads[0].id.clone());
        guard.active_thread_id = next_active.clone();
        guard.deleted_threads.push(thread_id.to_string());
        Ok(next_active)
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
        Ok(self.state.lock().unwrap().thread_meta.clone())
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
    render_rect_to_string(terminal, terminal.backend().buffer().area)
}

fn render_rect_to_string(terminal: &Terminal<TestBackend>, area: Rect) -> String {
    let buf = terminal.backend().buffer();
    let max_y = (area.y + area.height).min(buf.area.height);
    let max_x = (area.x + area.width).min(buf.area.width);
    let mut screen = String::new();
    for y in area.y..max_y {
        for x in area.x..max_x {
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

    // 3. Shell escape !echo hi emits ShellOutput
    send_text(&key_tx, "!echo hi");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("shell command recorded", || {
        engine
            .shell_commands()
            .iter()
            .any(|(tid, cmd)| tid == "t-trunk" && cmd == "echo hi")
    })
    .await;

    // 4. Branching via Ctrl+N and /branch bugfix
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

    // 5. PermissionRequested popup -> 'y' approves, second popup -> 'n' denies
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

    // 6. Esc during streaming emits cancel_turn
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
        engine.completed_turns_count() >= 3
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 6b. Tree Esc + Up + Enter switches back to Trunk, submits turn, then switches back to bugfix.
    // Context-only Tree lists only thread branches: t-trunk (0), t-branch-1 (1), t-branch-2 (2).
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
        engine.completed_turns_count() >= 4
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Move Down back to t-branch-2 (index 2).
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
        engine.completed_turns_count() >= 5
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 7. Quit cleanly via Ctrl+Q and inspect rendered terminal buffer
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
        !screen.contains("Blackboard"),
        "Blackboard pane must not render in 2-column layout:\n{screen}"
    );

    // Re-hydrating confirms turns persisted on engine
    let rehydrated = engine.hydrate_session().await.unwrap();
    assert_eq!(rehydrated.threads.len(), 3);
}

#[tokio::test]
async fn test_e2e_stepped_workflow_composer_streaming_branch_isolation_shell_permission() {
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

    // 3. Shell escape !echo hi emits ShellOutput block into transcript
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
        .contains("$ echo hi (exit 0)\n```ansi\nhi\n```"));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_to_string(&terminal).contains("$ echo hi (exit 0)"));

    // 4. Branching via Ctrl+N and /branch bugfix + Tree Enter switching & transcript isolation
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

    // 5. PermissionRequested popup renders visibly -> 'y' approves, 'n' denies
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

#[tokio::test]
async fn test_e2e_context_only_nested_tree_and_gfm_table_transcript() {
    let threads = vec![
        HydratedThread {
            id: "t-root".to_string(),
            title: "Main".to_string(),
            turns: vec![
                HydratedTurn {
                    id: "u1".to_string(),
                    role: "User".to_string(),
                    content: "How does auth work?".to_string(),
                },
                HydratedTurn {
                    id: "a1".to_string(),
                    role: "Assistant".to_string(),
                    content: "### Auth Overview\nChecks JWT signatures.".to_string(),
                },
            ],
        },
        HydratedThread {
            id: "t-child".to_string(),
            title: "Auth Child".to_string(),
            turns: Vec::new(),
        },
        HydratedThread {
            id: "t-grand".to_string(),
            title: "JWT Grandchild".to_string(),
            turns: Vec::new(),
        },
    ];
    let meta = vec![
        ThreadMeta {
            thread_id: "t-root".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
        },
        ThreadMeta {
            thread_id: "t-child".to_string(),
            parent_thread_id: Some("t-root".to_string()),
            inherited_turn_count: 2,
            merged: false,
        },
        ThreadMeta {
            thread_id: "t-grand".to_string(),
            parent_thread_id: Some("t-child".to_string()),
            inherited_turn_count: 2,
            merged: false,
        },
    ];
    let engine = MockLiveEngine::new_with_session("t-grand", threads, meta);
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(140, 32)).unwrap();
    let app = App::new();

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner_handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, app, engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("engine subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    send_text(&key_tx, "GFM_TABLE");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("GFM_TABLE turn completed", || {
        engine.completed_turns_count() >= 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), runner_handle)
        .await
        .expect("runner exit")
        .unwrap()
        .unwrap();

    let layout = compute_layout_with_chrome(Rect::new(0, 0, 140, 32), 0);
    let tree_text = render_rect_to_string(&terminal, layout.tree_area);
    let transcript_text = render_rect_to_string(&terminal, layout.transcript_area);

    assert!(
        tree_text.contains("ᛉ Main"),
        "expected root context in Grein panel:\n{tree_text}"
    );
    assert!(
        tree_text.contains("└─ Auth Child"),
        "expected depth-1 box-connected idle child context without ᛉ:\n{tree_text}"
    );
    assert!(
        tree_text.contains("   └─ JWT Grandchild"),
        "expected depth-2 box-connected idle grandchild context without ᛉ:\n{tree_text}"
    );
    assert!(
        !tree_text.contains("ᛉ Auth Child") && !tree_text.contains("ᛉ JWT Grandchild"),
        "idle sub-branches must omit ᛉ:\n{tree_text}"
    );
    assert!(!tree_text.contains('⚓'), "Tree must not contain ⚓:\n{tree_text}");
    assert!(
        !tree_text.contains("User:"),
        "Tree must not contain User: rows:\n{tree_text}"
    );
    assert!(
        !tree_text.contains("Agent"),
        "Tree must not contain Agent rows:\n{tree_text}"
    );
    assert!(
        !tree_text.contains('↳'),
        "Tree must not contain ↳ rows:\n{tree_text}"
    );

    assert!(
        transcript_text.contains("❯ GFM_TABLE"),
        "expected user prompt in Transcript:\n{transcript_text}"
    );
    assert!(
        transcript_text.contains("Auth Matrix"),
        "expected markdown heading in Transcript:\n{transcript_text}"
    );
    for box_ch in ['┌', '┬', '┐', '├', '┼', '┤', '└', '┴', '┘', '│'] {
        assert!(
            transcript_text.contains(box_ch),
            "expected box-drawn GFM table char {box_ch:?} in Transcript:\n{transcript_text}"
        );
    }
    assert!(transcript_text.contains("context-only"));
    assert!(transcript_text.contains("gfm_table"));
    assert!(
        !transcript_text.contains("| :--- |"),
        "raw markdown table delimiter must not render unparsed:\n{transcript_text}"
    );
}

#[tokio::test]
async fn test_e2e_tree_vertical_scrolling_short_viewport() {
    let make_threads = || -> (Vec<HydratedThread>, Vec<ThreadMeta>) {
        let threads = (0..14)
            .map(|i| HydratedThread {
                id: format!("t-{i}"),
                title: format!("Branch-{i:02}"),
                turns: Vec::new(),
            })
            .collect();
        let meta = (0..14)
            .map(|i| ThreadMeta {
                thread_id: format!("t-{i}"),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
            })
            .collect();
        (threads, meta)
    };

    // Phase A: scroll Down from Branch-00 to Branch-13 inside short 100x10 viewport
    let (threads_a, meta_a) = make_threads();
    let engine_a = MockLiveEngine::new_with_session("t-0", threads_a, meta_a);
    let engine_a_dyn: Arc<dyn TuiEnginePort> = engine_a.clone();
    let mut terminal_a = Terminal::new(TestBackend::new(100, 10)).unwrap();
    let (key_tx_a, key_rx_a) = mpsc::unbounded_channel();
    let handle_a = tokio::spawn(async move {
        run_with_engine_events(&mut terminal_a, App::new(), engine_a_dyn, key_rx_a).await?;
        Ok::<_, io::Error>(terminal_a)
    });

    wait_for("engine A subscription", || {
        engine_a.state.lock().unwrap().event_tx.is_some()
    })
    .await;
    send_key(&key_tx_a, KeyCode::Esc, KeyModifiers::NONE);
    for _ in 0..13 {
        send_key(&key_tx_a, KeyCode::Down, KeyModifiers::NONE);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    send_key(&key_tx_a, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal_a = timeout(Duration::from_secs(3), handle_a)
        .await
        .expect("runner A exit")
        .unwrap()
        .unwrap();

    let layout = compute_layout_with_chrome(Rect::new(0, 0, 100, 10), 0);
    let tree_down = render_rect_to_string(&terminal_a, layout.tree_area);
    assert!(
        tree_down.contains("Branch-13"),
        "Down scrolling must keep Branch-13 visible:\n{tree_down}"
    );
    assert!(
        !tree_down.contains("Branch-00"),
        "Branch-00 must scroll out of short viewport:\n{tree_down}"
    );

    // Phase B: start selected at Branch-13, scroll Up back to Branch-00
    let (threads_b, meta_b) = make_threads();
    let engine_b = MockLiveEngine::new_with_session("t-13", threads_b, meta_b);
    let engine_b_dyn: Arc<dyn TuiEnginePort> = engine_b.clone();
    let mut terminal_b = Terminal::new(TestBackend::new(100, 10)).unwrap();
    let (key_tx_b, key_rx_b) = mpsc::unbounded_channel();
    let handle_b = tokio::spawn(async move {
        run_with_engine_events(&mut terminal_b, App::new(), engine_b_dyn, key_rx_b).await?;
        Ok::<_, io::Error>(terminal_b)
    });

    wait_for("engine B subscription", || {
        engine_b.state.lock().unwrap().event_tx.is_some()
    })
    .await;
    send_key(&key_tx_b, KeyCode::Esc, KeyModifiers::NONE);
    for _ in 0..13 {
        send_key(&key_tx_b, KeyCode::Up, KeyModifiers::NONE);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    send_key(&key_tx_b, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal_b = timeout(Duration::from_secs(3), handle_b)
        .await
        .expect("runner B exit")
        .unwrap()
        .unwrap();

    let tree_up = render_rect_to_string(&terminal_b, layout.tree_area);
    assert!(
        tree_up.contains("Branch-00"),
        "Up scrolling must restore Branch-00 into view:\n{tree_up}"
    );
    assert!(
        !tree_up.contains("Branch-13"),
        "Branch-13 must scroll out when returning to top:\n{tree_up}"
    );

    // Stepped scroll_offset verification on single App instance
    let (threads_c, _) = make_threads();
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-0".to_string(),
            threads: threads_c,
        },
    );
    let mut terminal_c = Terminal::new(TestBackend::new(100, 10)).unwrap();
    app.set_focus(FocusedPane::Tree);
    for _ in 0..13 {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        )
        .unwrap();
    }
    draw_frame_mut(&mut terminal_c, &mut app).unwrap();
    assert!(app.tree.scroll_offset > 0);
    for _ in 0..13 {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        )
        .unwrap();
    }
    draw_frame_mut(&mut terminal_c, &mut app).unwrap();
    assert_eq!(app.tree.scroll_offset, 0);
}

#[tokio::test]
async fn test_e2e_fork_grein_close_delete_tree_x_d_and_sole_thread_guard() {
    let engine = MockLiveEngine::new_with_trunk("t-trunk", "Main");
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("engine subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    // 1. Sole-thread guard refuses /close and /delete in Composer
    send_text(&key_tx, "/close");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    send_text(&key_tx, "/delete");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(engine.deleted_threads().is_empty());
    assert_eq!(engine.threads_count(), 1);

    // 2. Create 4 forks via /grein, /fork, /branch
    for (cmd, expected_count) in [
        ("/grein idea-a", 2usize),
        ("/grein idea-b", 3usize),
        ("/fork idea-c", 4usize),
        ("/branch idea-d", 5usize),
    ] {
        send_text(&key_tx, cmd);
        send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
        wait_for(cmd, || engine.threads_count() == expected_count).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    // 3. Close/delete forks via Composer /close and /delete
    send_text(&key_tx, "/close");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("/close deleted t-branch-4", || {
        engine.deleted_threads().contains(&"t-branch-4".to_string())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_text(&key_tx, "/delete");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("/delete deleted t-branch-3", || {
        engine.deleted_threads().contains(&"t-branch-3".to_string())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 4. Close/delete remaining forks via Tree 'x' and 'd'
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Char('x'), KeyModifiers::NONE);
    wait_for("Tree x deleted t-branch-2", || {
        engine.deleted_threads().contains(&"t-branch-2".to_string())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_key(&key_tx, KeyCode::Char('d'), KeyModifiers::NONE);
    wait_for("Tree d deleted t-branch-1", || {
        engine.deleted_threads().contains(&"t-branch-1".to_string())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 5. Sole-thread guard on remaining Main context via Tree x/d and Composer /close
    send_key(&key_tx, KeyCode::Char('x'), KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Char('d'), KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    send_text(&key_tx, "/close");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), handle)
        .await
        .expect("runner exit")
        .unwrap()
        .unwrap();

    assert_eq!(
        engine.deleted_threads(),
        vec!["t-branch-4", "t-branch-3", "t-branch-2", "t-branch-1"]
    );
    assert_eq!(engine.threads_count(), 1);

    let layout = compute_layout_with_chrome(Rect::new(0, 0, 120, 30), 0);
    let tree_text = render_rect_to_string(&terminal, layout.tree_area);
    let status_text = render_rect_to_string(&terminal, layout.status_bar_area);

    assert!(tree_text.contains("Main"));
    for gone in ["idea-a", "idea-b", "idea-c", "idea-d"] {
        assert!(
            !tree_text.contains(gone),
            "deleted fork {gone} must not remain in Tree:\n{tree_text}"
        );
    }
    assert!(
        status_text.contains("Cannot delete the only context"),
        "expected sole-thread guard notice on status bar:\n{status_text}"
    );
}

#[tokio::test]
async fn test_e2e_transcript_fork_banner_on_branch_vs_omitted_on_root() {
    let engine = MockLiveEngine::new_with_trunk("t-trunk", "Main Trunk");
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal_branch = Terminal::new(TestBackend::new(140, 28)).unwrap();

    // Phase A: turn on root (2 turns), fork via /grein, send branch turn -> check fork banner
    let (key_tx_a, key_rx_a) = mpsc::unbounded_channel();
    let handle_a = tokio::spawn(async move {
        run_with_engine_events(&mut terminal_branch, App::new(), engine_dyn, key_rx_a).await?;
        Ok::<_, io::Error>(terminal_branch)
    });

    wait_for("engine subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    send_text(&key_tx_a, "explain dag");
    send_key(&key_tx_a, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("root turn completed", || engine.completed_turns_count() >= 1).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_text(&key_tx_a, "/grein experiment");
    send_key(&key_tx_a, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("fork created", || engine.threads_count() == 2).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_text(&key_tx_a, "branch idea");
    send_key(&key_tx_a, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("branch turn completed", || engine.completed_turns_count() >= 2).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_key(&key_tx_a, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal_branch = timeout(Duration::from_secs(3), handle_a)
        .await
        .expect("runner A exit")
        .unwrap()
        .unwrap();

    let layout = compute_layout_with_chrome(Rect::new(0, 0, 140, 28), 0);
    let branch_transcript = render_rect_to_string(&terminal_branch, layout.transcript_area);
    assert!(
        branch_transcript
            .contains("> ↳ Forked from Main Trunk (2 ancestor turns in LLM context)"),
        "expected fork banner on branched transcript:\n{branch_transcript}"
    );
    assert!(branch_transcript.contains("branch idea"));

    // Phase B: re-run and switch back to Main Trunk via Tree -> verify fork banner omitted on root
    let engine_dyn_b: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal_root = Terminal::new(TestBackend::new(140, 28)).unwrap();
    let (key_tx_b, key_rx_b) = mpsc::unbounded_channel();
    let handle_b = tokio::spawn(async move {
        run_with_engine_events(&mut terminal_root, App::new(), engine_dyn_b, key_rx_b).await?;
        Ok::<_, io::Error>(terminal_root)
    });

    wait_for("engine B subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;
    send_key(&key_tx_b, KeyCode::Esc, KeyModifiers::NONE);
    send_key(&key_tx_b, KeyCode::Up, KeyModifiers::NONE);
    send_key(&key_tx_b, KeyCode::Enter, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_key(&key_tx_b, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal_root = timeout(Duration::from_secs(3), handle_b)
        .await
        .expect("runner B exit")
        .unwrap()
        .unwrap();

    let root_transcript = render_rect_to_string(&terminal_root, layout.transcript_area);
    assert!(root_transcript.contains("explain dag"));
    assert!(
        !root_transcript.contains("Forked from"),
        "root transcript must omit fork banner:\n{root_transcript}"
    );
}

#[tokio::test]
async fn test_e2e_subagent_strip_and_bottom_status_bar_live_activity() {
    // Phase A: idle + 0 subagents -> strip auto-hidden, status bar shows ? for help + idle
    let engine_idle = MockLiveEngine::new_with_trunk("t-trunk", "Main");
    let engine_idle_dyn: Arc<dyn TuiEnginePort> = engine_idle.clone();
    let mut terminal_idle = Terminal::new(TestBackend::new(120, 32)).unwrap();
    let (key_tx_a, key_rx_a) = mpsc::unbounded_channel();
    let handle_a = tokio::spawn(async move {
        run_with_engine_events(&mut terminal_idle, App::new(), engine_idle_dyn, key_rx_a).await?;
        Ok::<_, io::Error>(terminal_idle)
    });

    wait_for("idle engine subscription", || {
        engine_idle.state.lock().unwrap().event_tx.is_some()
    })
    .await;
    engine_idle.emit(TuiEngineEvent::AgentMetaUpdated {
        thread_id: "t-trunk".to_string(),
        model: Some("claude-sonnet".to_string()),
        mode: Some("code".to_string()),
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    send_key(&key_tx_a, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal_idle = timeout(Duration::from_secs(3), handle_a)
        .await
        .expect("runner A exit")
        .unwrap()
        .unwrap();

    let idle_layout = compute_layout_with_chrome(Rect::new(0, 0, 120, 32), 0);
    let idle_full = render_to_string(&terminal_idle);
    let idle_status = render_rect_to_string(&terminal_idle, idle_layout.status_bar_area);
    assert!(!idle_full.contains("Subagents"));
    assert!(idle_status.contains("? for help"));
    assert!(idle_status.contains("code · claude-sonnet"));
    assert!(idle_status.contains("✓ idle"));
    assert!(idle_status.contains("0 subagents"));

    // Phase B: active turn + 7 SubagentUpdated + ThoughtChunk + ToolActivityUpdated -> strip below Composer + +2 more + esc to cancel
    let engine_busy = MockLiveEngine::new_with_trunk("t-trunk", "Main");
    let engine_busy_dyn: Arc<dyn TuiEnginePort> = engine_busy.clone();
    let mut terminal_busy = Terminal::new(TestBackend::new(120, 32)).unwrap();
    let (key_tx_b, key_rx_b) = mpsc::unbounded_channel();
    let handle_b = tokio::spawn(async move {
        run_with_engine_events(&mut terminal_busy, App::new(), engine_busy_dyn, key_rx_b).await?;
        Ok::<_, io::Error>(terminal_busy)
    });

    wait_for("busy engine subscription", || {
        engine_busy.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    send_text(&key_tx_b, "SLOW_TURN");
    send_key(&key_tx_b, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("slow turn started", || {
        engine_busy.state.lock().unwrap().slow_turn_cancel.is_some()
    })
    .await;

    engine_busy.emit(TuiEngineEvent::AgentMetaUpdated {
        thread_id: "t-trunk".to_string(),
        model: Some("gemini-2.5-pro".to_string()),
        mode: Some("agent".to_string()),
    });
    engine_busy.emit(TuiEngineEvent::ThoughtChunk {
        thread_id: "t-trunk".to_string(),
        content: "analyzing diff".to_string(),
    });
    engine_busy.emit(TuiEngineEvent::ToolActivityUpdated {
        thread_id: "t-trunk".to_string(),
        tool_name: Some("read_file".to_string()),
    });
    for i in 1..=7 {
        let status = match i {
            6 => SubagentStatus::Completed,
            7 => SubagentStatus::Failed,
            _ => SubagentStatus::Running,
        };
        engine_busy.emit(TuiEngineEvent::SubagentUpdated {
            thread_id: "t-trunk".to_string(),
            subagent: SubagentSnapshot {
                id: format!("sa-{i}"),
                role: format!("Worker {i}"),
                agent_type: "research".to_string(),
                model: Some("gemini-2.5-pro".to_string()),
                activity: Some(format!("step {i}")),
                status,
                started_at_ms: 0,
            },
        });
    }
    tokio::time::sleep(Duration::from_millis(70)).await;
    send_key(&key_tx_b, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal_busy = timeout(Duration::from_secs(3), handle_b)
        .await
        .expect("runner B exit")
        .unwrap()
        .unwrap();

    let busy_layout =
        compute_layout_with_chrome(Rect::new(0, 0, 120, 32), subagent_strip_height(7));
    let strip_area = busy_layout
        .subagent_strip_area
        .expect("subagent strip area must be allocated");
    assert!(
        strip_area.y >= busy_layout.composer_area.y + busy_layout.composer_area.height,
        "Subagent strip must render below Composer"
    );
    assert_eq!(
        strip_area.y + strip_area.height,
        busy_layout.status_bar_area.y
    );

    let strip_text = render_rect_to_string(&terminal_busy, strip_area);
    let busy_status = render_rect_to_string(&terminal_busy, busy_layout.status_bar_area);
    assert!(strip_text.contains("Subagents"));
    assert!(strip_text.contains("Agent(research: Worker 1)"));
    assert!(strip_text.contains("gemini-2.5-pro"));
    assert!(strip_text.contains("+2 more"));
    assert!(busy_status.contains("esc to cancel"));
    assert!(busy_status.contains("tool: read_file"));
    assert!(busy_status.contains("7 subagents"));

    // Stepped transition check: ThoughtChunk preview and SubagentRemoved auto-hide
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![HydratedThread {
                id: "t-1".to_string(),
                title: "Main".to_string(),
                turns: Vec::new(),
            }],
        },
    );
    let mut stepped_terminal = Terminal::new(TestBackend::new(120, 26)).unwrap();
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThoughtChunk {
            thread_id: "t-1".to_string(),
            content: "inspecting logs".to_string(),
        },
    );
    draw_frame_mut(&mut stepped_terminal, &mut app).unwrap();
    let thought_screen = render_to_string(&stepped_terminal);
    assert!(thought_screen.contains("thinking: inspecting logs"));
    assert!(thought_screen.contains("esc to cancel"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::SubagentUpdated {
            thread_id: "t-1".to_string(),
            subagent: SubagentSnapshot {
                id: "sa-only".to_string(),
                role: "Checker".to_string(),
                agent_type: "research".to_string(),
                model: None,
                activity: Some("running".to_string()),
                status: SubagentStatus::Running,
                started_at_ms: 0,
            },
        },
    );
    draw_frame_mut(&mut stepped_terminal, &mut app).unwrap();
    assert!(render_to_string(&stepped_terminal).contains("Agent(research: Checker)"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::SubagentRemoved {
            thread_id: "t-1".to_string(),
            subagent_id: "sa-only".to_string(),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::TurnCompleted {
            thread_id: "t-1".to_string(),
        },
    );
    draw_frame_mut(&mut stepped_terminal, &mut app).unwrap();
    let cleared_screen = render_to_string(&stepped_terminal);
    assert!(!cleared_screen.contains("Subagents"));
    assert!(cleared_screen.contains("? for help"));
    assert!(cleared_screen.contains("0 subagents"));
}

#[tokio::test]
async fn test_e2e_minimal_composer_and_help_modal_triggers_dismissal() {
    // 1. Empty Composer '?' opens centered Help modal & Composer bottom border stays minimal
    for trigger in ["?", "/?", "/help"] {
        let engine = MockLiveEngine::new_with_trunk("t-trunk", "Main");
        let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let (key_tx, key_rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
            Ok::<_, io::Error>(terminal)
        });

        wait_for("engine subscription", || {
            engine.state.lock().unwrap().event_tx.is_some()
        })
        .await;

        send_text(&key_tx, trigger);
        if trigger != "?" {
            send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
        let terminal = timeout(Duration::from_secs(3), handle)
            .await
            .expect("runner exit")
            .unwrap()
            .unwrap();

        let layout = compute_layout_with_chrome(Rect::new(0, 0, 110, 30), 0);
        let composer_bottom_row = render_rect_to_string(
            &terminal,
            Rect::new(
                layout.composer_area.x,
                layout.composer_area.y + layout.composer_area.height - 1,
                layout.composer_area.width,
                1,
            ),
        );
        assert!(
            composer_bottom_row
                .trim_end_matches('\n')
                .chars()
                .all(|ch| matches!(ch, '└' | '─' | '┘')),
            "Composer bottom border must not contain keybind legend ({trigger}): {composer_bottom_row:?}"
        );

        let centered_modal_rect = Rect::new(18, 3, 74, 24);
        let modal_text = render_rect_to_string(&terminal, centered_modal_rect);
        assert!(
            modal_text.contains("Keybindings & Commands"),
            "Help modal must render centered for trigger {trigger}:\n{modal_text}"
        );
        assert!(modal_text.contains("/fork, /grein"));
        assert!(modal_text.contains("/root, /new"));
        assert!(modal_text.contains("/rename"));
        assert!(modal_text.contains("/join, /merge"));
        assert!(modal_text.contains("/close, /delete"));
        assert!(modal_text.contains("/?, /help, ?"));
        assert!(modal_text.contains("Alt+N"));
        assert!(modal_text.contains("Alt+R / F2"));
        assert!(modal_text.contains("Press Esc, q, ?, or Enter to close"));
        assert!(engine.recorded_turns().is_empty());
    }

    // 2. Full dismissal (Esc, q, ?, Enter) and mid-sentence '?' typing in one E2E run
    let engine = MockLiveEngine::new_with_trunk("t-trunk", "Main");
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("engine subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    // Open with '?', dismiss with Esc
    send_key(&key_tx, KeyCode::Char('?'), KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);

    // Mid-sentence '?' inserts '?' normally and submits turn "why?"
    send_text(&key_tx, "why?");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("mid-sentence '?' turn submitted", || {
        engine
            .recorded_turns()
            .iter()
            .any(|(_, p)| p == "why?")
    })
    .await;

    // Open with '/?', dismiss with 'q' without quitting
    send_text(&key_tx, "/?");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::NONE);

    // Open with '/help', dismiss with '?'
    send_text(&key_tx, "/help");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Char('?'), KeyModifiers::NONE);

    // Open with '/help', dismiss with Enter
    send_text(&key_tx, "/help");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);

    // Verify Composer remains focused & functional after all modal dismissals
    send_text(&key_tx, "still interactive");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("post-help turn submitted", || {
        engine
            .recorded_turns()
            .iter()
            .any(|(_, p)| p == "still interactive")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), handle)
        .await
        .expect("runner exit")
        .unwrap()
        .unwrap();

    let final_screen = render_to_string(&terminal);
    assert!(!final_screen.contains("Keybindings & Commands"));
    assert!(!final_screen.contains("Enter: Send"));
    assert!(final_screen.contains("Reply(t-trunk): why?"));
    assert!(final_screen.contains("Reply(t-trunk): still interactive"));
}

#[tokio::test]
async fn test_e2e_optimistic_fork_nesting_startup_subscription_order_and_metadata_error_notice() {
    struct OrderingAndSlowMetadataEngine {
        call_log: Mutex<Vec<&'static str>>,
        event_tx: Mutex<Option<mpsc::UnboundedSender<TuiEngineEvent>>>,
        allow_fork_meta: Arc<Notify>,
        fail_metadata: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for OrderingAndSlowMetadataEngine {
        async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
            self.call_log.lock().unwrap().push("subscribe_events");
            let (tx, rx) = mpsc::unbounded_channel();
            *self.event_tx.lock().unwrap() = Some(tx);
            Ok(rx)
        }
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            self.call_log.lock().unwrap().push("hydrate_session");
            // Emitting during hydrate_session only succeeds if subscribe_events ran first
            if let Some(tx) = self.event_tx.lock().unwrap().as_ref() {
                let _ = tx.send(TuiEngineEvent::AgentMetaUpdated {
                    thread_id: "t-root".to_string(),
                    model: Some("early-model".to_string()),
                    mode: Some("code".to_string()),
                });
            }
            Ok(HydratedSession {
                active_thread_id: "t-root".to_string(),
                threads: vec![HydratedThread {
                    id: "t-root".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                }],
            })
        }
        async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
            let calls = {
                let mut log = self.call_log.lock().unwrap();
                log.push("thread_metadata");
                log.iter().filter(|&&c| c == "thread_metadata").count()
            };
            if calls >= 2 {
                self.allow_fork_meta.notified().await;
            }
            if self.fail_metadata.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("metadata backend offline".to_string());
            }
            Ok(vec![ThreadMeta {
                thread_id: "t-root".to_string(),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
            }])
        }
        async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
            Ok(())
        }
        async fn fork_thread(&self, _thread_id: &str, title: &str) -> io::Result<String> {
            Ok(if title.is_empty() {
                "t-fork-1".to_string()
            } else {
                format!("t-fork-{title}")
            })
        }
        async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
            Ok(())
        }
        async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
            Ok(())
        }
    }

    let allow_meta = Arc::new(Notify::new());
    let engine = Arc::new(OrderingAndSlowMetadataEngine {
        call_log: Mutex::new(Vec::new()),
        event_tx: Mutex::new(None),
        allow_fork_meta: Arc::clone(&allow_meta),
        fail_metadata: std::sync::atomic::AtomicBool::new(true),
    });
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(120, 26)).unwrap();

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("startup metadata called", || {
        engine
            .call_log
            .lock()
            .unwrap()
            .contains(&"thread_metadata")
    })
    .await;

    // subscribe_events must precede hydrate_session and thread_metadata
    {
        let log = engine.call_log.lock().unwrap();
        let sub_idx = log.iter().position(|&c| c == "subscribe_events").unwrap();
        let hyd_idx = log.iter().position(|&c| c == "hydrate_session").unwrap();
        let meta_idx = log.iter().position(|&c| c == "thread_metadata").unwrap();
        assert!(sub_idx < hyd_idx && hyd_idx < meta_idx, "call order: {log:?}");
    }

    // Fork a branch while second thread_metadata() call is still blocked on Notify
    send_text(&key_tx, "/branch opt-child");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Unblock failing metadata poll -> surfaces non-intrusive status_notice
    allow_meta.notify_waiters();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Switch back to t-root via Tree so status bar displays t-root's early-model metadata too
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Up, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), handle)
        .await
        .expect("runner exit")
        .unwrap()
        .unwrap();

    let layout = compute_layout_with_chrome(Rect::new(0, 0, 120, 26), 0);
    let tree_text = render_rect_to_string(&terminal, layout.tree_area);
    let status_text = render_rect_to_string(&terminal, layout.status_bar_area);

    assert!(
        tree_text.contains("└─ opt-child"),
        "Optimistic fork must nest under parent even before metadata resolves:\n{tree_text}"
    );
    assert!(
        !tree_text.contains("ᛉ opt-child"),
        "Idle optimistic sub-branch must omit ᛉ:\n{tree_text}"
    );
    assert!(
        status_text.contains("metadata backend offline") && status_text.contains("code · early-model"),
        "Status bar must surface metadata failure non-intrusively and retain early subscription events:\n{status_text}"
    );
}

#[tokio::test]
async fn test_e2e_root_create_rename_shortcuts_and_join_workflow() {
    let engine = MockLiveEngine::new_with_trunk("t-main", "Main");
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(130, 30)).unwrap();

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("engine subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    // 1. Fork child from Main and rename it via Alt+R
    send_text(&key_tx, "/branch temp-name");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("child forked", || engine.threads_count() == 2).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_key(&key_tx, KeyCode::Char('r'), KeyModifiers::ALT);
    // Ctrl+U clears "/rename temp-name", then enter new /rename command
    send_key(&key_tx, KeyCode::Char('u'), KeyModifiers::CONTROL);
    send_text(&key_tx, "/rename Auth Spike");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("child renamed", || {
        engine
            .state
            .lock()
            .unwrap()
            .threads
            .iter()
            .any(|t| t.id == "t-branch-1" && t.title == "Auth Spike")
    })
    .await;

    // 2. Join child into Main via /join with summary
    send_text(&key_tx, "/join Token rotation verified");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("child marked merged", || {
        engine
            .state
            .lock()
            .unwrap()
            .thread_meta
            .iter()
            .any(|m| m.thread_id == "t-branch-1" && m.merged)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 3. Create new root via Alt+N, then rename it via F2
    send_key(&key_tx, KeyCode::Char('n'), KeyModifiers::ALT);
    wait_for("new root created", || engine.threads_count() == 3).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_key(&key_tx, KeyCode::F(2), KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Char('u'), KeyModifiers::CONTROL);
    send_text(&key_tx, "/rename Docs Root");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("new root renamed", || {
        engine
            .state
            .lock()
            .unwrap()
            .threads
            .iter()
            .any(|t| t.title == "Docs Root")
    })
    .await;

    // 4. Switch to Main in Tree so its transcript (with Joined note) renders on screen
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    for _ in 0..4 {
        send_key(&key_tx, KeyCode::Up, KeyModifiers::NONE);
    }
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(40)).await;

    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), handle)
        .await
        .expect("runner exit")
        .unwrap()
        .unwrap();

    let layout = compute_layout_with_chrome(Rect::new(0, 0, 130, 30), 0);
    let tree_text = render_rect_to_string(&terminal, layout.tree_area);
    let transcript_text = render_rect_to_string(&terminal, layout.transcript_area);

    assert!(tree_text.contains("ᛉ Main"), "{tree_text}");
    assert!(
        tree_text.contains("└─ Auth Spike [joined]"),
        "Expected joined badge and tree connector without ᛉ on idle sub-branch:\n{tree_text}"
    );
    assert!(!tree_text.contains("ᛉ Auth Spike"), "{tree_text}");
    assert!(tree_text.contains("ᛉ Docs Root"), "{tree_text}");
    assert!(
        transcript_text.contains("Joined from Auth Spike: Token rotation verified"),
        "Expected joined summary blockquote in Main transcript:\n{transcript_text}"
    );
}

#[tokio::test]
async fn test_e2e_compact_keymap_branch_cycling_root_switching_scroll_and_join_guard() {
    let long_lines = (1..=45)
        .map(|i| format!("Long root line {i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let preview_lines = (1..=45)
        .map(|i| format!("Preview branch line {i:02}"))
        .collect::<Vec<_>>()
        .join("\n");

    let threads = vec![
        HydratedThread {
            id: "r1".to_string(),
            title: "Root One".to_string(),
            turns: vec![HydratedTurn {
                id: "u-r1".to_string(),
                role: "Assistant".to_string(),
                content: long_lines,
            }],
        },
        HydratedThread {
            id: "b1".to_string(),
            title: "Branch 1A".to_string(),
            turns: vec![HydratedTurn {
                id: "u-b1".to_string(),
                role: "Assistant".to_string(),
                content: preview_lines,
            }],
        },
        HydratedThread {
            id: "b2".to_string(),
            title: "Branch 1B".to_string(),
            turns: vec![HydratedTurn {
                id: "u-b2".to_string(),
                role: "Assistant".to_string(),
                content: "MARKER_BRANCH_1B".to_string(),
            }],
        },
        HydratedThread {
            id: "r2".to_string(),
            title: "Root Two".to_string(),
            turns: vec![HydratedTurn {
                id: "u-r2".to_string(),
                role: "Assistant".to_string(),
                content: "MARKER_ROOT_TWO".to_string(),
            }],
        },
        HydratedThread {
            id: "r3".to_string(),
            title: "Lone Root".to_string(),
            turns: vec![HydratedTurn {
                id: "u-r3".to_string(),
                role: "Assistant".to_string(),
                content: "MARKER_LONE_ROOT".to_string(),
            }],
        },
    ];
    let meta = vec![
        ThreadMeta {
            thread_id: "r1".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
        },
        ThreadMeta {
            thread_id: "b1".to_string(),
            parent_thread_id: Some("r1".to_string()),
            inherited_turn_count: 1,
            merged: false,
        },
        ThreadMeta {
            thread_id: "b2".to_string(),
            parent_thread_id: Some("r1".to_string()),
            inherited_turn_count: 1,
            merged: false,
        },
        ThreadMeta {
            thread_id: "r2".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
        },
        ThreadMeta {
            thread_id: "r3".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
        },
    ];

    let engine = MockLiveEngine::new_with_session("r1", threads, meta);
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(120, 26)).unwrap();

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("engine subscription", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 1. Alt+1..9 remain unbound; Alt+Home scrolls active r1 to top from Composer
    for digit in '1'..='9' {
        send_key(&key_tx, KeyCode::Char(digit), KeyModifiers::ALT);
    }
    send_key(&key_tx, KeyCode::Home, KeyModifiers::ALT);
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 2. Tab cycles r1 -> b1 -> b2, then Shift+Tab -> b1 -> r1; Alt+J -> r2 -> r3
    send_key(&key_tx, KeyCode::Tab, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Tab, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Char('j'), KeyModifiers::ALT);
    // Now on r2; Alt+J moves to r3 (lone root), Tab stays on r3
    send_key(&key_tx, KeyCode::Char('j'), KeyModifiers::ALT);
    send_key(&key_tx, KeyCode::Tab, KeyModifiers::NONE);
    // Alt+K wraps back to r2, Alt+Shift+Up returns to r1
    send_key(&key_tx, KeyCode::Char('k'), KeyModifiers::ALT);
    send_key(&key_tx, KeyCode::Up, KeyModifiers::ALT | KeyModifiers::SHIFT);

    // 3. Ctrl+G toggles Composer -> Tree during SLOW_TURN without cancelling, Esc cancels turn
    send_text(&key_tx, "SLOW_TURN");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("slow turn started on r1", || {
        engine.state.lock().unwrap().slow_turn_cancel.is_some()
    })
    .await;
    send_key(&key_tx, KeyCode::Char('g'), KeyModifiers::CONTROL);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        engine.cancelled_threads().is_empty(),
        "Ctrl+G must toggle focus without cancelling streaming turn"
    );
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    wait_for("Esc cancelled r1 turn", || {
        engine.cancelled_threads().contains(&"r1".to_string())
    })
    .await;
    wait_for("slow turn finished", || engine.completed_turns_count() >= 1).await;

    // 4. Attempting Tree 'm' and Composer /join on root r1 sets notice without merging
    send_key(&key_tx, KeyCode::Char('m'), KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    send_text(&key_tx, "/join");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);

    // 5. Enter Tree, preview b1, scroll preview transcript to top via Alt+Home so line 01 renders
    send_key(&key_tx, KeyCode::Esc, KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Down, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, KeyCode::Home, KeyModifiers::ALT);
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), handle)
        .await
        .expect("runner exit")
        .unwrap()
        .unwrap();

    let layout = compute_layout_with_chrome(Rect::new(0, 0, 120, 26), 0);
    let transcript_text = render_rect_to_string(&terminal, layout.transcript_area);
    let status_text = render_rect_to_string(&terminal, layout.status_bar_area);

    assert!(
        transcript_text.contains("Transcript (Preview: Branch 1A)"),
        "Expected Tree preview of Branch 1A:\n{transcript_text}"
    );
    assert!(
        transcript_text.contains("Preview branch line 01"),
        "Expected Alt+Home in Tree preview to scroll b1 to top:\n{transcript_text}"
    );
    assert!(
        status_text.contains("Cannot join a root context (no parent branch)"),
        "Expected root join guard notice in status bar:\n{status_text}"
    );
    assert!(
        engine
            .state
            .lock()
            .unwrap()
            .thread_meta
            .iter()
            .all(|m| !m.merged),
        "Root /join attempt must not mark any thread merged"
    );

    // Help modal must contain new keymap rows and zero /wb or Ctrl+B
    let mut help_app = App::new();
    help_app.show_help_modal = true;
    let mut help_term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut help_term, &mut help_app).unwrap();
    let help_screen = render_to_string(&help_term);
    assert!(help_screen.contains("Tab / Shift+Tab"), "{help_screen}");
    assert!(help_screen.contains("Alt+↑/↓, PgUp/PgDn"), "{help_screen}");
    assert!(help_screen.contains("Alt+Shift+↑/↓, Alt+J/K"), "{help_screen}");
    assert!(help_screen.contains("Esc / Ctrl+G"), "{help_screen}");
    assert!(!help_screen.contains("/wb"), "{help_screen}");
    assert!(help_screen.contains("Ctrl+B"), "{help_screen}");
}
