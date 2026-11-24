use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{
    display_thread_title, format_terminal_sidebar_cwd_with_home, shorten_terminal_cwd_with_home,
    thread_display_parts_with_home, App, AppAction, FocusedPane, SidebarView, ThreadItem,
};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, PtyScreenSnapshot, ThreadMeta, TuiEngineEvent, TuiEnginePort,
    WorktreeEntry,
};
use grein_tui::keymap::{BindableAction, Keymap};
use grein_tui::render::{compute_app_layout, render_app_mut};
use grein_tui::runner::{apply_engine_event, apply_hydrated_session, run_with_engine_events};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
}

fn alt(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::ALT)
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

fn render_rect(terminal: &Terminal<TestBackend>, rect: Rect) -> String {
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in rect.y..rect.y.saturating_add(rect.height) {
        for x in rect.x..rect.x.saturating_add(rect.width) {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn make_meta(id: &str, parent: Option<&str>, kind: ThreadKind, cwd: Option<&str>) -> ThreadMeta {
    ThreadMeta {
        thread_id: id.to_string(),
        parent_thread_id: parent.map(str::to_string),
        inherited_turn_count: 0,
        merged: false,
        kind,
        cwd: cwd.map(str::to_string),
        git_branch: None,
        git_dirty: 0,
        custom_name: None,
        agent_provider: None,
    }
}

#[test]
fn test_composer_multiline_up_down_preserves_desired_column_and_respects_autocomplete() {
    // 1. Headless logical \n lines with empty middle line preserve desired column
    let mut app = App::new();
    app.input_buffer = "abcdefg\n\nhijklmnop".to_string();
    app.cursor_position = app.input_buffer.len() - 4; // 'm' at col 5 on line 2
    assert_eq!(app.composer_cursor_2d(), (5, 2));

    assert!(app.handle_key_event(key(KeyCode::Up)));
    assert_eq!(app.composer_cursor_2d(), (0, 1));

    assert!(app.handle_key_event(key(KeyCode::Up)));
    assert_eq!(app.composer_cursor_2d(), (5, 0));

    // At top row, Up is boundary no-op and leaves cursor intact
    assert!(!app.handle_key_event(key(KeyCode::Up)));
    assert_eq!(app.composer_cursor_2d(), (5, 0));

    // Down crosses blank line back to col 5 on line 2
    assert!(app.handle_key_event(key(KeyCode::Down)));
    assert_eq!(app.composer_cursor_2d(), (0, 1));
    assert!(app.handle_key_event(key(KeyCode::Down)));
    assert_eq!(app.composer_cursor_2d(), (5, 2));

    // Horizontal edit resets desired column
    assert!(app.handle_key_event(key(KeyCode::Left)));
    assert_eq!(app.composer_cursor_2d(), (4, 2));
    assert!(app.handle_key_event(key(KeyCode::Up)));
    assert!(app.handle_key_event(key(KeyCode::Up)));
    assert_eq!(app.composer_cursor_2d(), (4, 0));

    // 2. Rendered soft-wrapped visual lines move visually on Up/Down
    let mut wrapped_app = App::new();
    wrapped_app.input_buffer =
        "alpha bravo charlie delta echo foxtrot golf hotel india juliet".to_string();
    wrapped_app.cursor_position = wrapped_app.input_buffer.len();
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    terminal.draw(|f| render_app_mut(f, &mut wrapped_app)).unwrap();
    assert!(wrapped_app.last_composer_inner_width > 0);
    let before_pos = wrapped_app.cursor_position;
    assert!(wrapped_app.handle_key_event(key(KeyCode::Up)));
    assert!(wrapped_app.cursor_position < before_pos);
    assert!(wrapped_app.handle_key_event(key(KeyCode::Down)));
    assert!(wrapped_app.cursor_position >= before_pos.saturating_sub(5));

    // 3. Shift+Tab (BackTab) is completely unbound in Composer when autocomplete closed,
    //    and cycles backwards when /worktree autocomplete is open.
    let mut ac_app = App::new();
    ac_app.worktrees = vec![WorktreeEntry {
        repo_root: "/repo".to_string(),
        repo_name: "repo".to_string(),
        path: "/repo/.grein/worktrees/wt-1".to_string(),
        branch: Some("wt-1".to_string()),
        head_short: None,
        is_detached: false,
        is_primary: false,
        is_grein_managed: true,
        dirty_count: 0,
        attached_thread_ids: Vec::new(),
    }];
    assert!(!ac_app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)));
    for ch in "/worktree ".chars() {
        ac_app.handle_key_event(key(KeyCode::Char(ch)));
    }
    assert!(ac_app.worktree_autocomplete_open);
    assert_eq!(ac_app.worktree_autocomplete_idx, 0);
    assert!(ac_app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)));
    assert_eq!(ac_app.worktree_autocomplete_idx, 1);
}

#[test]
fn test_full_height_sidebar_singular_plural_footer_and_worktree_git_badges() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "r1".to_string(),
            threads: vec![HydratedThread {
                id: "r1".to_string(),
                title: "Main".to_string(),
                turns: Vec::new(),
                custom_name: None,
                agent_provider: None,
            }],
        },
    );
    app.thread_meta.insert(
        "r1".to_string(),
        ThreadMeta {
            git_branch: Some("worktree-01a20285-dead-beef-cafe-1234567890ab".to_string()),
            ..make_meta("r1", None, ThreadKind::Agent, None)
        },
    );
    app.input_buffer = "l1\nl2\nl3\nl4\nl5".to_string();

    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let area = Rect::new(0, 0, 110, 24);
    let layout = compute_app_layout(&app, area, 0, false);
    assert_eq!(layout.tree_area.y + layout.tree_area.height, layout.status_bar_area.y);

    // Bottom-left corner of sidebar must align cleanly above status bar without shrinking for Composer auto-grow
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(0, layout.status_bar_area.y - 1)].symbol(), "╰");

    let screen = render_screen(&terminal);
    assert!(screen.contains("1 agent · 0 shells"), "{screen}");
    assert!(!screen.contains(": cmd · ?"), "{screen}");
    assert!(screen.contains("⎇ worktree: 01a20285"), "{screen}");

    // Worktree branch suffix -wt formatting and Terminal/Worktrees full height
    app.thread_meta.get_mut("r1").unwrap().git_branch = Some("feat-auth-wt".to_string());
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: "t-term".to_string(),
                title: "Terminal".to_string(),
                turns: Vec::new(),
                custom_name: None,
                agent_provider: None,
            },
        },
    );
    app.thread_meta.insert(
        "t-term".to_string(),
        make_meta("t-term", Some("r1"), ThreadKind::Terminal, Some("/tmp")),
    );
    app.switch_thread("r1");
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let two_screen = render_screen(&terminal);
    assert!(two_screen.contains("1 agent · 1 shell"), "{two_screen}");
    assert!(two_screen.contains("⎇ worktree: feat-auth"), "{two_screen}");

    app.switch_thread("t-term");
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let term_layout = compute_app_layout(&app, area, 0, true);
    assert_eq!(
        term_layout.tree_area.y + term_layout.tree_area.height,
        term_layout.status_bar_area.y
    );

    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let wt_buf = terminal.backend().buffer();
    assert_eq!(wt_buf[(0, term_layout.status_bar_area.y - 1)].symbol(), "╰");
}

#[test]
fn test_unified_naming_uuid_redaction_and_inline_rename_clearing() {
    let uuid_cwd =
        "/home/alice/repo/.grein/worktrees/01a20285-dead-beef-cafe-1234567890ab/crates/tui";
    let short_sidebar = format_terminal_sidebar_cwd_with_home(uuid_cwd, Some("/home/alice"), true);
    assert!(
        !short_sidebar.contains("1234567890ab"),
        "Must never leak 36-char UUID: {short_sidebar}"
    );
    assert!(short_sidebar.starts_with("worktree: 01a20285"), "{short_sidebar}");

    let short_border = shorten_terminal_cwd_with_home(uuid_cwd, Some("/home/alice"));
    assert!(
        !short_border.contains("1234567890ab"),
        "Top border must never leak 36-char UUID: {short_border}"
    );
    assert_eq!(short_border, "⎇ 01a20285/crates/tui");

    let deep_home = "/home/alice/projects/org/subrepo/deep-leaf";
    let elided = format_terminal_sidebar_cwd_with_home(deep_home, Some("/home/alice"), true);
    assert_eq!(elided, "~/.../deep-leaf");

    // Agent custom_name hides generic placeholder ("Main", "Branch 2"), preserves real ACP title
    let agent_placeholder = ThreadItem {
        id: "a1".to_string(),
        title: "Branch 2".to_string(),
        custom_name: Some("Auth".to_string()),
        anchors: Vec::new(),
    };
    let agent_meta = make_meta("a1", None, ThreadKind::Agent, None);
    assert_eq!(display_thread_title(&agent_placeholder, Some(&agent_meta)), "[Auth]");

    let agent_real = ThreadItem {
        id: "a2".to_string(),
        title: "Test Conversation".to_string(),
        custom_name: Some("Auth".to_string()),
        anchors: Vec::new(),
    };
    assert_eq!(
        display_thread_title(&agent_real, Some(&agent_meta)),
        "[Auth] Test Conversation"
    );

    // Terminal custom_name + auto_label
    let term_named = ThreadItem {
        id: "t1".to_string(),
        title: "Terminal".to_string(),
        custom_name: Some("Tests".to_string()),
        anchors: Vec::new(),
    };
    let term_meta = make_meta("t1", None, ThreadKind::Terminal, Some("/home/alice/projects"));
    let parts = thread_display_parts_with_home(&term_named, Some(&term_meta), Some("/home/alice"), true);
    assert_eq!(parts.custom_name.as_deref(), Some("Tests"));
    assert_eq!(parts.auto_label, "~/projects");
    assert_eq!(parts.combined(true), "[Tests] ~/projects");

    // Inline rename: starts empty when unnamed, pre-fills custom_name when named, empty Enter clears to None,
    // and leaving Tree (or ThreadCreated) clears rename_prompt
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t1".to_string(),
            threads: vec![HydratedThread {
                id: "t1".to_string(),
                title: "Test Conversation".to_string(),
                turns: Vec::new(),
                custom_name: None,
                agent_provider: None,
            }],
        },
    );
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('r')));
    assert_eq!(app.rename_prompt.as_ref().unwrap().buffer, "");

    // Leaving Tree clears rename_prompt
    app.set_focus(FocusedPane::Composer);
    assert!(app.rename_prompt.is_none());

    // ThreadCreated clears rename_prompt
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('r')));
    assert!(app.rename_prompt.is_some());
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: "t2".to_string(),
                title: "Branch".to_string(),
                turns: Vec::new(),
                custom_name: None,
                agent_provider: None,
            },
        },
    );
    assert!(app.rename_prompt.is_none());

    // Empty Enter clears custom_name back to None via ThreadCustomNameUpdated
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCustomNameUpdated {
            thread_id: "t1".to_string(),
            custom_name: Some("Auth".to_string()),
        },
    );
    app.switch_thread("t1");
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('r')));
    assert_eq!(app.rename_prompt.as_ref().unwrap().buffer, "Auth");
    app.handle_key_event(ctrl('u'));
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::RenameThread {
            thread_id: "t1".to_string(),
            title: String::new(),
        })
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCustomNameUpdated {
            thread_id: "t1".to_string(),
            custom_name: None,
        },
    );
    assert_eq!(app.threads[0].custom_name, None);
    assert_eq!(app.display_title_for("t1", ""), "Test Conversation");
}

#[test]
fn test_dim_root_numbers_alt_siblings_alt_screen_passthrough_and_preview_target() {
    // 1. Strict 1-to-1 keymap defaults (zero duplicates, zero Shift+Tab)
    let km = Keymap::default();
    assert_eq!(km.chords_for(BindableAction::EnterManageMode).len(), 1);
    assert_eq!(km.chords_for(BindableAction::ScrollUp).len(), 1);
    assert_eq!(km.chords_for(BindableAction::ScrollDown).len(), 1);
    assert_eq!(km.chords_for(BindableAction::PrevSibling).len(), 1);
    assert_eq!(km.chords_for(BindableAction::NextSibling).len(), 1);
    assert!(km
        .match_write_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        .is_none());

    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "r1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "r1".to_string(),
                    title: "Root One".to_string(),
                    turns: vec![],
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "c1".to_string(),
                    title: "Child A".to_string(),
                    turns: vec![],
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "c2".to_string(),
                    title: "Child B".to_string(),
                    turns: vec![],
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t1".to_string(),
                    title: "Terminal".to_string(),
                    turns: vec![],
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "r2".to_string(),
                    title: "Root Two".to_string(),
                    turns: vec![],
                    custom_name: None,
                    agent_provider: None,
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                make_meta("r1", None, ThreadKind::Agent, None),
                make_meta("c1", Some("r1"), ThreadKind::Agent, None),
                make_meta("c2", Some("r1"), ThreadKind::Agent, None),
                make_meta("t1", Some("r1"), ThreadKind::Terminal, Some("/tmp")),
                make_meta("r2", None, ThreadKind::Agent, None),
            ],
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("1 ᛉ Root One"), "{screen}");
    assert!(screen.contains("2 ᛉ Root Two"), "{screen}");
    assert!(!screen.contains("3 "), "{screen}");
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(2, 1)].symbol(), "1");
    assert!(buf[(2, 1)].modifier.contains(Modifier::DIM));
    assert_eq!(buf[(2, 5)].symbol(), "2");
    assert!(buf[(2, 5)].modifier.contains(Modifier::DIM));

    // Alt+H / Alt+L cycles siblings under same parent (c1 -> c2 -> t1 -> c1)
    app.switch_thread("c1");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert!(app.handle_key_event(alt('l')));
    assert_eq!(app.active_thread_id(), "c2");
    assert!(app.handle_key_event(alt('l')));
    assert_eq!(app.active_thread_id(), "t1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Alt+H/J/K/L bypass interception when PTY alternate_screen == true
    app.pty_screens.insert(
        "t1".to_string(),
        PtyScreenSnapshot {
            alternate_screen: true,
            ..PtyScreenSnapshot::default()
        },
    );
    app.pending_actions.clear();
    for ch in ['h', 'j', 'k', 'l'] {
        assert!(app.handle_key_event(alt(ch)));
    }
    assert_eq!(app.active_thread_id(), "t1", "Alt+H/L must pass through on alt-screen");
    let pty_writes: Vec<Vec<u8>> = app
        .pending_actions
        .drain(..)
        .filter_map(|a| match a {
            AppAction::PtyWrite { bytes, .. } => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(
        pty_writes,
        vec![b"\x1bh".to_vec(), b"\x1bj".to_vec(), b"\x1bk".to_vec(), b"\x1bl".to_vec()]
    );

    // Normal screen (alternate_screen == false): Alt+H returns to c2
    app.pty_screens.get_mut("t1").unwrap().alternate_screen = false;
    assert!(app.handle_key_event(alt('h')));
    assert_eq!(app.active_thread_id(), "c2");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // MANAGE preview scroll target fix: active is Terminal t1, highlighted is Agent c1 -> Alt+K scrolls c1
    app.switch_thread("t1");
    app.transcript_viewport_mut("c1").set_max_scroll(50);
    app.transcript_viewport_mut("c1").scroll_to_bottom();
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1); // c1
    app.pending_actions.clear();
    assert!(app.handle_key_event(alt('k')));
    assert_eq!(app.transcript_viewport("c1").scroll_offset(), 40);
    assert!(app.pending_actions.is_empty(), "Must not scroll hidden t1 PTY");

    // Reverse: active is Agent c1, highlighted is Terminal t1 -> Alt+K emits PtyScroll for t1
    app.switch_thread("c1");
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(3); // t1
    assert!(app.handle_key_event(alt('k')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "t1".to_string(),
            delta: 5,
        })
    );

    // H collapse-all moves child selection safely to visible root, L expand-all restores
    app.handle_key_event(key(KeyCode::Char('H')));
    let sel_id = match app.tree.nodes.get(app.tree.selected_index().unwrap()) {
        Some(grein_tui::ui::tree::TreeNode::Thread(t)) => t.id.as_str(),
        _ => "",
    };
    assert_eq!(sel_id, "r1");
    assert_eq!(app.tree.nodes.len(), 2);
    app.handle_key_event(key(KeyCode::Char('L')));
    assert_eq!(app.tree.nodes.len(), 5);
}

#[tokio::test]
async fn test_e2e_terminal_error_sets_visible_status_notice() {
    struct ErrTerminalEngine {
        event_tx: Mutex<Option<mpsc::UnboundedSender<TuiEngineEvent>>>,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for ErrTerminalEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-term".to_string(),
                threads: vec![HydratedThread {
                    id: "t-term".to_string(),
                    title: "Terminal".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
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
            *self.event_tx.lock().unwrap() = Some(tx);
            Ok(rx)
        }
        async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
            Ok(vec![make_meta("t-term", None, ThreadKind::Terminal, Some("/tmp"))])
        }
        async fn rename_thread(&self, _thread_id: &str, _title: &str) -> io::Result<()> {
            Err(io::Error::other("rename backend offline"))
        }
        async fn pty_write(&self, _thread_id: &str, _bytes: Vec<u8>) -> io::Result<()> {
            Err(io::Error::other("pty backend offline"))
        }
    }

    let engine = Arc::new(ErrTerminalEngine {
        event_tx: Mutex::new(None),
    });
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    for _ in 0..100 {
        if engine.event_tx.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Trigger err_rx via failing action on active Terminal thread
    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('r')))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('X')))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    drop(key_tx);

    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let status = render_rect(&terminal, Rect::new(0, 23, 110, 1));
    assert!(
        status.contains("Error: rename backend offline"),
        "Terminal err_rx must surface visible status_notice:\n{status}"
    );
}
