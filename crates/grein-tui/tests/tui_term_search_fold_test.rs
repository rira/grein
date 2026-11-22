use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use grein_core::ThreadKind;
use grein_tui::app::{
    active_shell_label, shell_label_from_env, App, AppAction, FocusedPane,
};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, HydratedTurn, PtyCell, PtyScreenSnapshot, ThreadMeta,
    TuiEngineEvent, TuiEnginePort,
};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, compute_ordered_thread_entries,
    compute_ordered_thread_entries_with_folds, draw_frame_mut, handle_event,
    run_with_engine_events,
};
use grein_tui::search::SearchDirection;
use grein_tui::ui::tree::TreeNode;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

fn ctrl(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
}

fn render_screen(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn render_rect(terminal: &Terminal<TestBackend>, area: Rect) -> String {
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in area.y..(area.y + area.height).min(buf.area.height) {
        for x in area.x..(area.x + area.width).min(buf.area.width) {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn make_snapshot_from_lines(lines: &[&str], rows: u16, cols: u16) -> PtyScreenSnapshot {
    let mut cells = vec![vec![PtyCell::default(); cols as usize]; rows as usize];
    for (r, line) in lines.iter().enumerate() {
        if r >= rows as usize {
            break;
        }
        for (c, ch) in line.chars().enumerate() {
            if c >= cols as usize {
                break;
            }
            cells[r][c] = PtyCell {
                contents: ch.to_string(),
                ..PtyCell::default()
            };
        }
    }
    PtyScreenSnapshot {
        rows,
        cols,
        cells,
        ..PtyScreenSnapshot::default()
    }
}

fn meta_entry(
    id: &str,
    parent: Option<&str>,
    kind: ThreadKind,
    cwd: Option<&str>,
    git_branch: Option<&str>,
    git_dirty: usize,
) -> ThreadMeta {
    ThreadMeta {
        thread_id: id.to_string(),
        parent_thread_id: parent.map(str::to_string),
        inherited_turn_count: 0,
        merged: false,
        kind,
        cwd: cwd.map(str::to_string),
        git_branch: git_branch.map(str::to_string),
        git_dirty,
    }
}

#[test]
fn test_dynamic_shell_label_from_env_and_status_bar() {
    assert_eq!(shell_label_from_env(None), "sh");
    assert_eq!(shell_label_from_env(Some("")), "sh");
    assert_eq!(shell_label_from_env(Some("   ")), "sh");
    assert_eq!(shell_label_from_env(Some("/")), "sh");
    assert_eq!(shell_label_from_env(Some("/bin/zsh")), "zsh");
    assert_eq!(
        shell_label_from_env(Some("/nix/store/xyz-bash-interactive-5.2/bin/bash")),
        "bash"
    );
    assert_eq!(shell_label_from_env(Some("fish")), "fish");

    let expected_shell = active_shell_label();
    assert!(!expected_shell.is_empty());

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

    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let idle_screen = render_screen(&terminal);
    assert!(
        idle_screen.contains(&format!(
            "Ctrl+G tree · Alt+T {expected_shell} · F1 help"
        )),
        "Idle status bar must reflect dynamic shell and F1 help:\n{idle_screen}"
    );

    app.thread_statuses
        .insert("t-1".to_string(), "streaming".to_string());
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let busy_screen = render_screen(&terminal);
    assert!(
        busy_screen.contains(&format!(
            "Esc cancel · Ctrl+G tree · Alt+T {expected_shell}"
        )),
        "Busy status bar must reflect dynamic shell:\n{busy_screen}"
    );
}

#[test]
fn test_pty_scrollback_shortcuts_mouse_scroll_cwd_and_badge_rendering() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-term".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent".to_string(),
                    title: "Agent".to_string(),
                    turns: vec![HydratedTurn {
                        id: "u1".to_string(),
                        role: "User".to_string(),
                        content: (0..40)
                            .map(|i| format!("Agent line {i}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    }],
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell".to_string(),
                    turns: Vec::new(),
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                meta_entry("t-agent", None, ThreadKind::Agent, None, None, 0),
                meta_entry(
                    "t-term",
                    Some("t-agent"),
                    ThreadKind::Terminal,
                    Some("/tmp/initial-dir"),
                    None,
                    0,
                ),
            ],
        },
    );

    let mut snap = make_snapshot_from_lines(&["scrolled line 1"], 16, 60);
    snap.scrollback_offset = 5;
    snap.cwd = Some("/home/user/projects/grein".to_string());
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyScreenUpdated {
            thread_id: "t-term".to_string(),
            snapshot: snap,
        },
    );

    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Shift+PageUp / Alt+PageUp -> PtyScroll { delta: 5 } (no PtyWrite)
    app.pending_actions.clear();
    assert!(app.handle_key_event(shift(KeyCode::PageUp)));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "t-term".to_string(),
            delta: 5,
        })
    );
    assert!(app.handle_key_event(alt(KeyCode::PageUp)));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "t-term".to_string(),
            delta: 5,
        })
    );

    // Shift+PageDown / Alt+PageDown -> PtyScroll { delta: -5 } (no PtyWrite)
    assert!(app.handle_key_event(shift(KeyCode::PageDown)));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "t-term".to_string(),
            delta: -5,
        })
    );
    assert!(app.handle_key_event(alt(KeyCode::PageDown)));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "t-term".to_string(),
            delta: -5,
        })
    );

    // Plain PageUp / PageDown still pass through to PtyWrite
    assert!(app.handle_key_event(key(KeyCode::PageUp)));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"\x1b[5~".to_vec(),
        })
    );
    assert!(app.handle_key_event(key(KeyCode::PageDown)));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"\x1b[6~".to_vec(),
        })
    );

    // Mouse ScrollUp / ScrollDown on Terminal thread dispatches PtyScroll +/-3
    let mouse_up = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 40,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    let mouse_down = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 40,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    handle_event(&mut app, mouse_up.clone()).unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "t-term".to_string(),
            delta: 3,
        })
    );
    handle_event(&mut app, mouse_down.clone()).unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "t-term".to_string(),
            delta: -3,
        })
    );

    // Render Terminal status bar: shows Ctrl+G for tree, [scroll -5], and short cwd basename "grein"
    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let status = render_rect(&terminal, Rect::new(0, 23, 110, 1));
    assert!(status.contains("Ctrl+G for tree"), "{status}");
    assert!(status.contains("[scroll -5]"), "{status}");
    assert!(status.contains("grein"), "{status}");
    assert!(!status.contains("/home/user/projects/grein"), "{status}");

    // On Agent thread, mouse ScrollUp scrolls Transcript cleanly
    app.switch_thread("t-agent");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let max_off = app.active_transcript_scroll_offset();
    assert!(max_off >= 5);
    handle_event(&mut app, mouse_up).unwrap();
    assert_eq!(app.active_transcript_scroll_offset(), max_off - 3);
    handle_event(&mut app, mouse_down).unwrap();
    assert_eq!(app.active_transcript_scroll_offset(), max_off);
}

#[tokio::test]
async fn test_exited_pty_enter_restart_and_runner_scroll_restart_roundtrip() {
    #[derive(Default)]
    struct ScrollRestartState {
        scrolls: Vec<(String, isize)>,
        restarts: Vec<(String, u16, u16)>,
        writes: Vec<(String, Vec<u8>)>,
        scrollback_offset: usize,
        exited: bool,
        exit_code: Option<i32>,
        event_tx: Option<mpsc::UnboundedSender<TuiEngineEvent>>,
    }

    #[derive(Default)]
    struct ScrollRestartEngine {
        state: Mutex<ScrollRestartState>,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for ScrollRestartEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-term".to_string(),
                threads: vec![HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell".to_string(),
                    turns: Vec::new(),
                }],
            })
        }
        async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
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
            let (tx, rx) = mpsc::unbounded_channel();
            self.state.lock().unwrap().event_tx = Some(tx);
            Ok(rx)
        }
        async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
            Ok(vec![meta_entry(
                "t-term",
                None,
                ThreadKind::Terminal,
                Some("/tmp/repo"),
                None,
                0,
            )])
        }
        async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> io::Result<()> {
            self.state
                .lock()
                .unwrap()
                .writes
                .push((thread_id.to_string(), bytes));
            Ok(())
        }
        async fn pty_snapshot(
            &self,
            _thread_id: &str,
            rows: u16,
            cols: u16,
        ) -> io::Result<PtyScreenSnapshot> {
            let g = self.state.lock().unwrap();
            let mut snap = make_snapshot_from_lines(&["LIVE_SHELL"], rows.max(1), cols.max(1));
            snap.scrollback_offset = g.scrollback_offset;
            snap.exited = g.exited;
            snap.exit_code = g.exit_code;
            snap.cwd = Some("/tmp/repo".to_string());
            Ok(snap)
        }
        async fn pty_scroll(
            &self,
            thread_id: &str,
            delta: isize,
        ) -> io::Result<PtyScreenSnapshot> {
            let mut g = self.state.lock().unwrap();
            g.scrolls.push((thread_id.to_string(), delta));
            if delta >= 0 {
                g.scrollback_offset = g.scrollback_offset.saturating_add(delta as usize);
            } else {
                g.scrollback_offset = g.scrollback_offset.saturating_sub(delta.unsigned_abs());
            }
            let mut snap = make_snapshot_from_lines(&["HISTORY_LINE"], 20, 60);
            snap.scrollback_offset = g.scrollback_offset;
            snap.cwd = Some("/tmp/repo".to_string());
            Ok(snap)
        }
        async fn pty_restart(
            &self,
            thread_id: &str,
            rows: u16,
            cols: u16,
        ) -> io::Result<PtyScreenSnapshot> {
            let mut g = self.state.lock().unwrap();
            g.restarts.push((thread_id.to_string(), rows, cols));
            g.exited = false;
            g.exit_code = None;
            g.scrollback_offset = 0;
            let mut snap =
                make_snapshot_from_lines(&["RESTARTED_SHELL"], rows.max(1), cols.max(1));
            snap.exited = false;
            snap.exit_code = None;
            snap.cwd = Some("/tmp/repo".to_string());
            Ok(snap)
        }
    }

    // Unit-level fallback dimensions when terminal_pane_size == (0, 0)
    let mut unit_app = App::new();
    apply_hydrated_session(
        &mut unit_app,
        &HydratedSession {
            active_thread_id: "t-term".to_string(),
            threads: vec![HydratedThread {
                id: "t-term".to_string(),
                title: "Shell".to_string(),
                turns: Vec::new(),
            }],
        },
    );
    apply_engine_event(
        &mut unit_app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![meta_entry(
                "t-term",
                None,
                ThreadKind::Terminal,
                None,
                None,
                0,
            )],
        },
    );
    apply_engine_event(
        &mut unit_app,
        TuiEngineEvent::PtyExited {
            thread_id: "t-term".to_string(),
            exit_code: Some(1),
        },
    );
    assert_eq!(
        unit_app.thread_statuses.get("t-term").map(String::as_str),
        Some("error")
    );
    unit_app.pending_actions.clear();
    assert!(unit_app.handle_key_event(key(KeyCode::Enter)));
    assert_eq!(
        unit_app.pending_actions.pop_front(),
        Some(AppAction::PtyRestart {
            thread_id: "t-term".to_string(),
            rows: 24,
            cols: 80,
        })
    );

    // Receiving non-exited PtyScreenUpdated clears pty_exit_codes and resets error status
    apply_engine_event(
        &mut unit_app,
        TuiEngineEvent::PtyScreenUpdated {
            thread_id: "t-term".to_string(),
            snapshot: make_snapshot_from_lines(&["fresh"], 24, 80),
        },
    );
    assert!(!unit_app.is_terminal_exited("t-term"));
    assert_ne!(
        unit_app.thread_statuses.get("t-term").map(String::as_str),
        Some("error")
    );

    // Full async runner E2E
    let engine = Arc::new(ScrollRestartEngine::default());
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    for _ in 0..100 {
        if engine.state.lock().unwrap().event_tx.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 1. Shift+PageUp calls engine.pty_scroll(5) and shows [scroll -5]
    key_tx.send(Event::Key(shift(KeyCode::PageUp))).unwrap();
    for _ in 0..100 {
        if !engine.state.lock().unwrap().scrolls.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 2. Emit PtyExited -> status error; Enter restarts with terminal inner dimensions (21, 64)
    {
        let tx = engine.state.lock().unwrap().event_tx.clone().unwrap();
        tx.send(TuiEngineEvent::PtyExited {
            thread_id: "t-term".to_string(),
            exit_code: Some(127),
        })
        .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(40)).await;
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();

    for _ in 0..100 {
        if !engine.state.lock().unwrap().restarts.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(40)).await;
    key_tx.send(Event::Key(ctrl('q'))).unwrap();

    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("RESTARTED_SHELL"), "{screen}");
    assert!(screen.contains("● running"), "{screen}");

    let g = engine.state.lock().unwrap();
    assert_eq!(g.scrolls, vec![("t-term".to_string(), 5)]);
    assert_eq!(g.restarts, vec![("t-term".to_string(), 21, 64)]);
    assert!(g.writes.is_empty());
}

#[test]
fn test_vim_transcript_search_and_f1_g_question_help_triggers() {
    let mut app = App::new();
    let body = (0..45)
        .map(|i| match i {
            2 => "Line 02: alpha needle_one here".to_string(),
            20 => "Line 20: middle needle_two here".to_string(),
            42 => "Line 42: bottom needle_three here".to_string(),
            _ => format!("Line {i:02}: filler content"),
        })
        .collect::<Vec<_>>()
        .join("\n");

    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![HydratedThread {
                id: "t-1".to_string(),
                title: "Main".to_string(),
                turns: vec![HydratedTurn {
                    id: "a1".to_string(),
                    role: "Assistant".to_string(),
                    content: body,
                }],
            }],
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(110, 22)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();

    // 1. F1 opens Help from Composer, Tree, Transcript
    for pane in [
        FocusedPane::Composer,
        FocusedPane::Tree,
        FocusedPane::Transcript,
    ] {
        app.set_focus(pane);
        app.help_scroll_offset = 5;
        assert!(app.handle_key_event(key(KeyCode::F(1))));
        assert!(app.show_help_modal, "F1 must open Help in {pane:?}");
        assert_eq!(app.help_scroll_offset, 0);
        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.show_help_modal);
    }

    // 2. g? opens Help in Tree and Transcript; g followed by another key clears pending_g_prefix
    for pane in [FocusedPane::Tree, FocusedPane::Transcript] {
        app.set_focus(pane);
        app.handle_key_event(key(KeyCode::Char('g')));
        assert!(app.pending_g_prefix);
        app.handle_key_event(key(KeyCode::Char('?')));
        assert!(!app.pending_g_prefix);
        assert!(app.show_help_modal, "g? must open Help in {pane:?}");
        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.show_help_modal);

        // g followed by End clears pending_g_prefix, then ? opens Reverse Search (not Help)
        app.handle_key_event(key(KeyCode::Char('g')));
        assert!(app.pending_g_prefix);
        app.handle_key_event(key(KeyCode::End));
        assert!(!app.pending_g_prefix);
        app.handle_key_event(key(KeyCode::Char('?')));
        assert!(!app.show_help_modal);
        assert!(app.transcript_search.editing);
        assert_eq!(app.transcript_search.direction, SearchDirection::Reverse);
        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.transcript_search.is_visible());
    }

    // 3. Forward search '/' in Transcript: live recompute, highlight, Enter confirm, n/N + scroll
    app.set_focus(FocusedPane::Transcript);
    app.handle_key_event(key(KeyCode::Home)); // scroll_offset = 0
    assert_eq!(app.active_transcript_scroll_offset(), 0);

    app.handle_key_event(key(KeyCode::Char('/')));
    assert!(app.transcript_search.editing);
    assert_eq!(app.transcript_search.direction, SearchDirection::Forward);
    for ch in "needle".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    assert_eq!(app.transcript_search.matches.len(), 3);
    assert_eq!(app.transcript_search.current_index, Some(0));

    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let searching_screen = render_screen(&terminal);
    assert!(
        searching_screen.contains("/needle [1/3]"),
        "Expected live search bar:\n{searching_screen}"
    );
    assert!(searching_screen.contains("needle_one"));

    // Verify active match has Yellow bg highlight
    let buf = terminal.backend().buffer();
    let mut found_yellow_active = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(byte_idx) = row.find("needle_one") {
            let col = row[..byte_idx].chars().count() as u16;
            if buf[(col, y)].bg == Color::Yellow {
                found_yellow_active = true;
            }
        }
    }
    assert!(found_yellow_active, "Active search match must have Yellow bg");

    // Confirm with Enter, then 'n' jumps to match 2 (line 20) & match 3 (line 42) and scrolls viewport
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.transcript_search.editing);
    assert!(app.transcript_search.is_visible());

    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(app.transcript_search.current_index, Some(1));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_screen(&terminal).contains("needle_two"));
    assert!(render_screen(&terminal).contains("/needle [2/3]"));

    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(app.transcript_search.current_index, Some(2));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_screen(&terminal).contains("needle_three"));

    // 'N' steps back to match 2
    app.handle_key_event(key(KeyCode::Char('N')));
    assert_eq!(app.transcript_search.current_index, Some(1));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_screen(&terminal).contains("needle_two"));

    // Streaming a new match while search is active updates count cleanly while preserving current_index
    apply_engine_event(
        &mut app,
        TuiEngineEvent::StreamChunk {
            thread_id: "t-1".to_string(),
            content: "\nLine 46: extra needle_four streamed".to_string(),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let streamed_screen = render_screen(&terminal);
    assert_eq!(app.transcript_search.matches.len(), 4);
    assert_eq!(app.transcript_search.current_index, Some(1));
    assert!(streamed_screen.contains("/needle [2/4]"), "{streamed_screen}");

    // Esc clears search
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.transcript_search.is_visible());
    assert!(app.transcript_search.query.is_empty());

    // 4. Reverse search '?' in Tree from bottom: 'n' walks backward, 'N' walks forward, 'n' doesn't create root
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('G'))); // jump to bottom first
    let initial_threads = app.threads.len();
    app.handle_key_event(key(KeyCode::Char('?')));
    for ch in "needlex".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    assert_eq!(app.transcript_search.matches.len(), 0);
    app.handle_key_event(key(KeyCode::Backspace));
    assert_eq!(app.transcript_search.matches.len(), 4);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.transcript_search.current_index, Some(3));
    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(app.transcript_search.current_index, Some(2));
    assert!(app.pending_actions.is_empty(), "'n' during search must not CreateRoot");
    assert_eq!(app.threads.len(), initial_threads);
    app.handle_key_event(key(KeyCode::Char('N')));
    assert_eq!(app.transcript_search.current_index, Some(3));
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(!app.transcript_search.is_visible());

    // 5. Ctrl+F opens Forward Search in Composer, Tree, Transcript — but NOT in Terminal
    for pane in [
        FocusedPane::Composer,
        FocusedPane::Tree,
        FocusedPane::Transcript,
    ] {
        app.set_focus(pane);
        assert!(app.handle_key_event(ctrl('f')));
        assert!(app.transcript_search.editing);
        assert_eq!(app.transcript_search.direction, SearchDirection::Forward);
        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.transcript_search.is_visible());
    }

    // Inside Terminal, F1 and Ctrl+F pass through to PtyWrite
    app.threads.push(grein_tui::app::ThreadItem {
        id: "t-term".to_string(),
        title: "Shell".to_string(),
        anchors: Vec::new(),
    });
    app.thread_meta.insert(
        "t-term".to_string(),
        meta_entry("t-term", Some("t-1"), ThreadKind::Terminal, None, None, 0),
    );
    app.switch_thread("t-term");
    app.pending_actions.clear();
    assert!(app.handle_key_event(key(KeyCode::F(1))));
    assert!(!app.show_help_modal);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"\x1bOP".to_vec(),
        })
    );
    assert!(app.handle_key_event(ctrl('f')));
    assert!(!app.transcript_search.is_visible());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: vec![0x06],
        })
    );
}

#[test]
fn test_tree_folding_space_h_l_rendering_and_auto_unfold_ancestors() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "r1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "r1".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "b1".to_string(),
                    title: "Branch A".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "g1".to_string(),
                    title: "Grand A1".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "gg1".to_string(),
                    title: "TermChild".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "b2".to_string(),
                    title: "Branch B".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "r2".to_string(),
                    title: "Root 2".to_string(),
                    turns: Vec::new(),
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                meta_entry("r1", None, ThreadKind::Agent, None, None, 0),
                meta_entry("b1", Some("r1"), ThreadKind::Agent, None, None, 0),
                meta_entry("g1", Some("b1"), ThreadKind::Agent, None, None, 0),
                meta_entry("gg1", Some("g1"), ThreadKind::Terminal, None, None, 0),
                meta_entry("b2", Some("r1"), ThreadKind::Agent, None, None, 0),
                meta_entry("r2", None, ThreadKind::Agent, None, None, 0),
            ],
        },
    );

    // compute_ordered_thread_entries remains full while compute_ordered_thread_entries_with_folds hides folded subtrees
    assert_eq!(
        compute_ordered_thread_entries(&app.threads, &app.thread_meta).len(),
        6
    );
    let mut collapsed = BTreeSet::new();
    collapsed.insert("b1".to_string());
    let folded_entries =
        compute_ordered_thread_entries_with_folds(&app.threads, &app.thread_meta, &collapsed);
    assert_eq!(folded_entries.len(), 4);
    assert_eq!(folded_entries[1].hidden_descendants, 2);
    assert!(folded_entries[1].has_children);

    // Space in Tree folds b1 (selected) -> hides g1 & gg1, renders ▸ and (+2)
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1); // b1
    app.handle_key_event(key(KeyCode::Char(' ')));
    assert!(app.collapsed_threads.contains("b1"));
    assert_eq!(app.tree.nodes.len(), 4);

    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let folded_screen = render_screen(&terminal);
    assert!(
        folded_screen.contains("├─ ▸ Branch A (+2)"),
        "Expected folded indicator and hidden count:\n{folded_screen}"
    );
    assert!(!folded_screen.contains("Grand A1"), "{folded_screen}");
    assert!(!folded_screen.contains("TermChild"), "{folded_screen}");
    assert!(folded_screen.contains("└─ Branch B"), "{folded_screen}");

    // 'h' on already-folded b1 jumps selection to parent r1
    app.handle_key_event(key(KeyCode::Char('h')));
    let selected_id = match app.tree.nodes.get(app.tree.selected_index().unwrap()) {
        Some(TreeNode::Thread(t)) => t.id.as_str(),
        _ => "",
    };
    assert_eq!(selected_id, "r1");

    // 'h' on expanded r1 folds r1 (all 4 descendants hidden -> (+4))
    app.handle_key_event(key(KeyCode::Char('h')));
    assert!(app.collapsed_threads.contains("r1"));
    assert_eq!(app.tree.nodes.len(), 2); // r1, r2
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let root_folded = render_screen(&terminal);
    assert!(
        root_folded.contains("▸ ᛉ Main (+4)"),
        "Expected root fold badge:\n{root_folded}"
    );

    // 'l' / Right on folded r1 unfolds r1 while staying in Tree
    app.handle_key_event(key(KeyCode::Char('l')));
    assert!(!app.collapsed_threads.contains("r1"));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    // b1 is still folded (4 visible nodes: r1, b1, b2, r2)
    assert_eq!(app.tree.nodes.len(), 4);

    // Left on expanded r1 folds it again; Right unfolds it; Right again activates r1 -> Composer
    app.handle_key_event(key(KeyCode::Left));
    assert!(app.collapsed_threads.contains("r1"));
    app.handle_key_event(key(KeyCode::Right));
    assert!(!app.collapsed_threads.contains("r1"));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Space on leaf b2 is a safe no-op
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(2); // b2
    app.handle_key_event(key(KeyCode::Char(' ')));
    assert!(!app.collapsed_threads.contains("b2"));

    // Fold r1 & b1, then Tab cycle from r1 to b1 -> g1 -> gg1 auto-unfolds ancestors!
    app.tree.select_index(0); // r1
    app.handle_key_event(key(KeyCode::Char(' ')));
    assert!(app.collapsed_threads.contains("r1"));
    assert!(app.collapsed_threads.contains("b1"));
    assert_eq!(app.tree.nodes.len(), 2);

    // Tab cycles r1 -> b1 (unfolds r1), Tab -> g1 (unfolds b1)
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.active_thread_id(), "b1");
    assert!(!app.collapsed_threads.contains("r1"));
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.active_thread_id(), "g1");
    assert!(!app.collapsed_threads.contains("b1"));
    assert_eq!(app.tree.nodes.len(), 6);

    // Fold r1 again, then Alt+T from g1 jumps to child terminal gg1 and unfolds r1 + b1
    app.switch_thread("r1");
    app.collapsed_threads.insert("r1".to_string());
    app.collapsed_threads.insert("b1".to_string());
    app.switch_thread("g1");
    assert!(!app.collapsed_threads.contains("r1"));
    assert!(!app.collapsed_threads.contains("b1"));
    app.collapsed_threads.insert("r1".to_string());
    app.handle_key_event(alt(KeyCode::Char('t'))); // Wait: alt('t') via helper
    assert_eq!(app.active_thread_id(), "gg1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert!(!app.collapsed_threads.contains("r1"));
    assert_eq!(app.tree.nodes.len(), 6);

    // ThreadCreated under folded parent unfolds ancestors automatically
    app.switch_thread("r1");
    app.collapsed_threads.insert("r1".to_string());
    app.collapsed_threads.insert("b1".to_string());
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                meta_entry("r1", None, ThreadKind::Agent, None, None, 0),
                meta_entry("b1", Some("r1"), ThreadKind::Agent, None, None, 0),
                meta_entry("g1", Some("b1"), ThreadKind::Agent, None, None, 0),
                meta_entry("gg1", Some("g1"), ThreadKind::Terminal, None, None, 0),
                meta_entry("b2", Some("r1"), ThreadKind::Agent, None, None, 0),
                meta_entry("r2", None, ThreadKind::Agent, None, None, 0),
                meta_entry("g1-new", Some("b1"), ThreadKind::Agent, None, None, 0),
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: "g1-new".to_string(),
                title: "Created Child".to_string(),
                turns: Vec::new(),
            },
        },
    );
    assert_eq!(app.active_thread_id(), "g1-new");
    assert!(!app.collapsed_threads.contains("r1"));
    assert!(!app.collapsed_threads.contains("b1"));
    assert!(app
        .tree
        .nodes
        .iter()
        .any(|n| matches!(n, TreeNode::Thread(t) if t.id == "g1-new")));
}

#[test]
fn test_help_modal_documents_search_folding_pty_scroll_restart_and_git_badge() {
    let mut app = App::new();
    app.show_help_modal = true;
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let help = render_screen(&terminal);

    let shell = active_shell_label();
    for expected in [
        "F1",
        "g?",
        "Ctrl+F",
        "n / N",
        "Space",
        "Shift+PgUp/PgDn",
        "Enter restarts exited >_",
        "⎇ branch*",
        &format!("Alt+T >_ {shell}"),
    ] {
        assert!(help.contains(expected), "Help modal missing {expected:?}:\n{help}");
    }
}

#[test]
fn test_worktree_git_status_badge_clean_dirty_and_none() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-clean".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-clean".to_string(),
                    title: "Clean".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-dirty".to_string(),
                    title: "Dirty".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-none".to_string(),
                    title: "Plain".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Term".to_string(),
                    turns: Vec::new(),
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                meta_entry(
                    "t-clean",
                    None,
                    ThreadKind::Agent,
                    Some("/tmp/wt-clean"),
                    Some("feat/clean"),
                    0,
                ),
                meta_entry(
                    "t-dirty",
                    None,
                    ThreadKind::Agent,
                    Some("/tmp/wt-dirty"),
                    Some("feat/dirty"),
                    4,
                ),
                meta_entry("t-none", None, ThreadKind::Agent, None, None, 0),
                meta_entry(
                    "t-term",
                    Some("t-dirty"),
                    ThreadKind::Terminal,
                    Some("/tmp/wt-dirty"),
                    Some("feat/dirty"),
                    2,
                ),
            ],
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();

    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let clean_status = render_rect(&terminal, Rect::new(0, 23, 110, 1));
    assert!(clean_status.contains("⎇ feat/clean"), "{clean_status}");
    assert!(!clean_status.contains("⎇ feat/clean*"), "{clean_status}");

    app.switch_thread("t-dirty");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let dirty_status = render_rect(&terminal, Rect::new(0, 23, 110, 1));
    assert!(dirty_status.contains("⎇ feat/dirty*"), "{dirty_status}");

    app.switch_thread("t-none");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let none_status = render_rect(&terminal, Rect::new(0, 23, 110, 1));
    assert!(!none_status.contains('⎇'), "{none_status}");

    app.switch_thread("t-term");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let term_status = render_rect(&terminal, Rect::new(0, 23, 110, 1));
    assert!(term_status.contains("⎇ feat/dirty*"), "{term_status}");
    assert!(term_status.contains("wt-dirty"), "{term_status}");
}
