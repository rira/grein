use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{App, AppAction, FocusedPane, PendingPermission};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, PtyCell, PtyColor, PtyScreenSnapshot, ThreadMeta,
    TuiEngineEvent, TuiEnginePort,
};
use grein_tui::layout::compute_layout_with_options;
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, draw_frame_mut, handle_event,
    run_with_engine_events,
};
use ratatui::backend::{Backend, TestBackend};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier};
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
    let mut screen = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            screen.push_str(buf[(x, y)].symbol());
        }
        screen.push('\n');
    }
    screen
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

#[test]
fn test_dual_mode_layout_hides_composer_for_terminal_and_preserves_agent() {
    let area = Rect::new(0, 0, 120, 30);

    let agent_layout = compute_layout_with_options(area, 0, false, false);
    assert_eq!(agent_layout.composer_area.height, 3);
    assert_eq!(agent_layout.transcript_area.height, 26);

    let term_layout = compute_layout_with_options(area, 0, false, true);
    assert_eq!(term_layout.composer_area.height, 0);
    assert_eq!(term_layout.transcript_area.height, 29);
    assert_eq!(term_layout.tree_area.height, 29);
    assert_eq!(term_layout.status_bar_area, Rect::new(0, 29, 120, 1));

    let zoomed_term = compute_layout_with_options(area, 0, true, true);
    assert_eq!(zoomed_term.tree_area.width, 0);
    assert_eq!(zoomed_term.composer_area.height, 0);
    assert_eq!(zoomed_term.transcript_area, Rect::new(0, 0, 120, 29));
    assert_eq!(zoomed_term.status_bar_area, Rect::new(0, 29, 120, 1));

    // Tiny & zero-sized rects must never panic
    for tiny in [Rect::new(0, 0, 0, 0), Rect::new(0, 0, 2, 2)] {
        let _ = compute_layout_with_options(tiny, 0, false, true);
        let _ = compute_layout_with_options(tiny, 2, true, true);
    }

    // End-to-end rendering switches between Agent (Transcript + Composer) and Terminal (no Composer)
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent".to_string(),
                    title: "Agent Main".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell 1".to_string(),
                    turns: Vec::new(),
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
                ThreadMeta {
                    thread_id: "t-agent".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/tmp/repo".to_string()),
                    ..ThreadMeta::default()
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyScreenUpdated {
            thread_id: "t-term".to_string(),
            snapshot: make_snapshot_from_lines(&["$ cargo test", "running 42 tests"], 24, 80),
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let agent_screen = render_screen(&terminal);
    assert!(agent_screen.contains("Transcript"));
    assert!(agent_screen.contains("Composer"));

    app.switch_thread("t-term");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let term_screen = render_screen(&terminal);
    assert!(
        !term_screen.contains("Composer"),
        "Terminal thread must hide Composer completely:\n{term_screen}"
    );
    assert!(
        term_screen.contains("$ cargo test"),
        "Terminal screen content must render:\n{term_screen}"
    );
    assert!(term_screen.contains("running 42 tests"));

    app.switch_thread("t-agent");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let back_to_agent = render_screen(&terminal);
    assert!(back_to_agent.contains("Composer"));
    assert!(back_to_agent.contains("Transcript"));
}

#[test]
fn test_tree_rendering_terminal_prefix_glyph_and_exit_marker() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-sub-agent".to_string(),
                    title: "Auth".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-term-live".to_string(),
                    title: "Terminal".to_string(),
                    turns: Vec::new(),
                    custom_name: Some("Shell".to_string()),
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-term-exit-ok".to_string(),
                    title: "Terminal".to_string(),
                    turns: Vec::new(),
                    custom_name: Some("Logs".to_string()),
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-term-root-fail".to_string(),
                    title: "Terminal".to_string(),
                    turns: Vec::new(),
                    custom_name: Some("Build".to_string()),
                    agent_provider: None,
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "t-agent".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-sub-agent".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term-live".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term-exit-ok".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term-root-fail".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
                    ..ThreadMeta::default()
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyExited {
            thread_id: "t-term-exit-ok".to_string(),
            exit_code: Some(0),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyExited {
            thread_id: "t-term-root-fail".to_string(),
            exit_code: Some(2),
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(130, 26)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);

    assert!(screen.contains("1 ᛉ Main"), "{screen}");
    assert!(screen.contains("├─ Auth"), "{screen}");
    assert!(!screen.contains(">_ Auth"), "{screen}");
    assert!(screen.contains("├─ >_ [Shell] Terminal"), "{screen}");
    assert!(screen.contains("└─ >_ [Logs] Terminal ✓ 0"), "{screen}");
    assert!(screen.contains("2 >_ [Build] Terminal ✗ 2"), "{screen}");
    assert!(!screen.contains("ᛉ Build"), "{screen}");
    assert!(!screen.contains("ᛉ [Build]"), "{screen}");
}

#[test]
fn test_live_terminal_rendering_colors_attributes_wide_utf8_and_cursor() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-term".to_string(),
            threads: vec![HydratedThread {
                id: "t-term".to_string(),
                title: "Term".to_string(),
                turns: Vec::new(),
                              custom_name: None,
                              agent_provider: None,
            }],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![ThreadMeta {
                thread_id: "t-term".to_string(),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
                kind: ThreadKind::Terminal,
                cwd: Some("/work".to_string()),
                ..ThreadMeta::default()
            }],
        },
    );

    let mut cells = vec![vec![PtyCell::default(); 20]; 6];
    // Row 0: "📦XY" with RGB + bold + italic + underline + inverse on indexed color
    cells[0][0] = PtyCell {
        contents: "📦".to_string(),
        fg: PtyColor::Rgb(255, 100, 50),
        bg: PtyColor::Default,
        bold: true,
        ..PtyCell::default()
    };
    cells[0][1] = PtyCell {
        contents: String::new(),
        is_wide_continuation: true,
        ..PtyCell::default()
    };
    cells[0][2] = PtyCell {
        contents: "X".to_string(),
        fg: PtyColor::Indexed(196),
        bg: PtyColor::Indexed(235),
        italic: true,
        underline: true,
        ..PtyCell::default()
    };
    cells[0][3] = PtyCell {
        contents: "Y".to_string(),
        fg: PtyColor::Default,
        bg: PtyColor::Rgb(10, 20, 30),
        inverse: true,
        ..PtyCell::default()
    };
    cells[1][4] = PtyCell {
        contents: "Z".to_string(),
        ..PtyCell::default()
    };

    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyScreenUpdated {
            thread_id: "t-term".to_string(),
            snapshot: PtyScreenSnapshot {
                rows: 6,
                cols: 20,
                cells,
                cursor_row: 1,
                cursor_col: 4,
                hide_cursor: false,
                application_cursor: false,
                bracketed_paste: false,
                alternate_screen: false,
                exited: false,
                exit_code: None,
                ..PtyScreenSnapshot::default()
            },
        },
    );

    app.set_focus(FocusedPane::Terminal);
    app.sidebar_hidden = true; // Terminal starts at x=0, y=0 with TOP|BOTTOM borders -> inner starts at (0, 1)

    let mut terminal = Terminal::new(TestBackend::new(80, 14)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let cursor = terminal.backend_mut().get_cursor_position().unwrap();
    assert_eq!(cursor, Position::new(4, 2));

    let buf = terminal.backend().buffer();
    assert_eq!(buf[(0, 1)].symbol(), "📦");
    assert_eq!(buf[(0, 1)].fg, Color::Rgb(255, 100, 50));
    assert!(buf[(0, 1)].modifier.contains(Modifier::BOLD));

    // Wide continuation skipped cleanly -> X lands immediately at col + 2 without shifting
    assert_eq!(buf[(2, 1)].symbol(), "X");
    assert_eq!(buf[(2, 1)].fg, Color::Indexed(196));
    assert_eq!(buf[(2, 1)].bg, Color::Indexed(235));
    assert!(buf[(2, 1)].modifier.contains(Modifier::ITALIC));
    assert!(buf[(2, 1)].modifier.contains(Modifier::UNDERLINED));

    assert_eq!(buf[(3, 1)].symbol(), "Y");
    assert_eq!(buf[(3, 1)].bg, Color::Rgb(10, 20, 30));
    assert!(buf[(3, 1)].modifier.contains(Modifier::REVERSED));

    // Tiny/0-sized terminal bounds never panic and do not emit 0-sized PtyResize
    app.pending_actions.clear();
    let mut tiny_term = Terminal::new(TestBackend::new(2, 2)).unwrap();
    draw_frame_mut(&mut tiny_term, &mut app).unwrap();
    let mut zero_term = Terminal::new(TestBackend::new(0, 0)).unwrap();
    draw_frame_mut(&mut zero_term, &mut app).unwrap();
    app.pty_screens.insert(
        "t-term".to_string(),
        PtyScreenSnapshot {
            rows: 0,
            cols: 0,
            cells: Vec::new(),
            ..PtyScreenSnapshot::default()
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(
        !app.pending_actions
            .iter()
            .any(|a| matches!(a, AppAction::PtyResize { rows: 0, .. } | AppAction::PtyResize { cols: 0, .. })),
        "0-sized inner pane must not emit 0-dim PtyResize: {:?}",
        app.pending_actions
    );
}

#[test]
fn test_terminal_focus_ergonomics_tree_preview_and_key_passthrough() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell".to_string(),
                    turns: Vec::new(),
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
                ThreadMeta {
                    thread_id: "t-agent".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/tmp".to_string()),
                    ..ThreadMeta::default()
                },
            ],
        },
    );
    let mut snap = make_snapshot_from_lines(&["LIVE_PTY_PREVIEW"], 20, 60);
    snap.application_cursor = true;
    snap.bracketed_paste = true;
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyScreenUpdated {
            thread_id: "t-term".to_string(),
            snapshot: snap,
        },
    );

    // Tree browsing keeps Tree focus while previewing Terminal live screen
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert_eq!(app.active_thread_id(), "t-agent");

    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let preview_screen = render_screen(&terminal);
    assert!(preview_screen.contains(">_ /tmp (Preview)"), "{preview_screen}");
    assert!(preview_screen.contains("● live"), "{preview_screen}");
    assert!(preview_screen.contains("LIVE_PTY_PREVIEW"), "{preview_screen}");

    // 'l' and Right on leaf Terminal stay in MANAGE Tree; Enter enters FocusedPane::Terminal
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.active_thread_id(), "t-term");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Ctrl+G returns to Tree without forwarding \x07
    app.pending_actions.clear();
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.pending_actions.is_empty());

    // Enter from Tree enters FocusedPane::Terminal; Tab in MANAGE switches to Worktrees view
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, grein_tui::app::SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, grein_tui::app::SidebarView::Grein);
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Render frame so pane dimensions are known, then Alt+Z toggles zoom & triggers PtyResize without forwarding \x1bz
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    app.pending_actions.clear();
    assert!(!app.sidebar_hidden);
    app.handle_key_event(alt('z'));
    assert!(app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyResize {
            thread_id: "t-term".to_string(),
            rows: 21,
            cols: 100,
        })
    );
    assert!(app.pending_actions.is_empty());

    // Ctrl+C, Esc, Tab, Enter, Up (application_cursor), char, Paste all go to PtyWrite!
    app.handle_key_event(ctrl('c'));
    assert!(!app.should_quit());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: vec![0x03],
        })
    );

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: vec![0x1b],
        })
    );

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"\t".to_vec(),
        })
    );

    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"\r".to_vec(),
        })
    );

    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"\x1bOA".to_vec(),
        })
    );

    app.handle_key_event(key(KeyCode::Char('z')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"z".to_vec(),
        })
    );

    handle_event(&mut app, Event::Paste("ls\npwd".to_string())).unwrap();
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: b"\x1b[200~ls\npwd\x1b[201~".to_vec(),
        })
    );

    // Permission modal overlay remains handleable & Ctrl+G still escapes cleanly without forwarding \x07
    app.push_permission(PendingPermission {
        request_id: "perm-term".to_string(),
        description: "Allow write?".to_string(),
    });
    app.handle_key_event(key(KeyCode::Char('y')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "perm-term".to_string(),
            approved: true,
        })
    );
    assert!(app.pending_actions.is_empty());

    app.push_permission(PendingPermission {
        request_id: "perm-term-2".to_string(),
        description: "Allow read?".to_string(),
    });
    // Modal priority blocks Ctrl+G from jumping out while permission prompt is open (and does NOT send \x07 to PTY)
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert!(app.pending_actions.is_empty());
    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "perm-term-2".to_string(),
            approved: false,
        })
    );
    // After resolving modal, Ctrl+G enters MANAGE Tree cleanly
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    // Ctrl+Q passes straight to PTY (\x11) in WRITE mode; Ctrl+G then 'q' opens quit confirm, 'y' quits cleanly
    app.set_focus(FocusedPane::Terminal);
    app.pending_actions.clear();
    app.handle_key_event(ctrl('q'));
    assert!(!app.should_quit());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term".to_string(),
            bytes: vec![0x11],
        })
    );
    app.handle_key_event(ctrl('g'));
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.pending_quit_confirm.is_some());
    assert!(!app.should_quit());
    app.handle_key_event(key(KeyCode::Char('y')));
    assert!(app.should_quit());
}

#[test]
fn test_terminal_hotkeys_slash_commands_worktree_and_clipboard_copy() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-main".to_string(),
            threads: vec![HydratedThread {
                id: "t-main".to_string(),
                title: "Main".to_string(),
                turns: Vec::new(),
                              custom_name: None,
                              agent_provider: None,
            }],
        },
    );

    // 's' and 'S' in MANAGE Tree spawn child/root terminal; Composer does not hijack 's'
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('s')));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: Some(ref pid),
            cwd: None,
            ..
        }) if pid == "t-main"
    ));

    app.handle_key_event(key(KeyCode::Char('S')));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: None,
            cwd: None,
            ..
        })
    ));

    // /term [path] and /worktree in Composer
    app.set_focus(FocusedPane::Composer);
    app.submit_composer_prompt("/term");
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: Some(ref pid),
            cwd: None,
            ..
        }) if pid == "t-main"
    ));

    app.submit_composer_prompt("/term /tmp/custom-dir");
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: Some(ref pid),
            cwd: Some(ref path),
            ..
        }) if pid == "t-main" && path == "/tmp/custom-dir"
    ));

    app.submit_composer_prompt("/worktree");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::EnsureThreadWorktree {
            thread_id: "t-main".to_string(),
        })
    );

    // Clipboard copy on Terminal via y, Alt+Y, /copy
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "t-main".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-main".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
                    ..ThreadMeta::default()
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: "t-term".to_string(),
                title: "Term".to_string(),
                turns: Vec::new(),
                        custom_name: None,
                        agent_provider: None,
            },
        },
    );
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyScreenUpdated {
            thread_id: "t-term".to_string(),
            snapshot: make_snapshot_from_lines(&["hello pty", "line two"], 6, 40),
        },
    );

    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('y')));
    assert_eq!(app.last_copied_text.as_deref(), Some("hello pty\nline two"));

    app.last_copied_text = None;
    app.submit_composer_prompt("/copy");
    assert_eq!(app.last_copied_text.as_deref(), Some("hello pty\nline two"));

    // Help modal includes WRITE / MANAGE sections, s / S, Ctrl+G, Tab, Alt+Z, /term, /wt
    app.show_help_modal = true;
    let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let help = render_screen(&terminal);
    for expected in ["WRITE Mode", "MANAGE Mode", "s / S", "Ctrl+G", "Tab", "Alt+Z", "/term", "/wt"] {
        assert!(help.contains(expected), "Help modal missing {expected:?}:\n{help}");
    }
}

#[derive(Default)]
struct MockTerminalState {
    threads: Vec<HydratedThread>,
    meta: Vec<ThreadMeta>,
    active_thread_id: String,
    created_terminals: Vec<(Option<String>, Option<String>, u16, u16)>,
    pty_writes: Vec<(String, Vec<u8>)>,
    pty_resizes: Vec<(String, u16, u16)>,
    pty_snapshots: Vec<(String, u16, u16)>,
    worktree_calls: Vec<String>,
    snapshots_by_thread: HashMap<String, PtyScreenSnapshot>,
    event_tx: Option<mpsc::UnboundedSender<TuiEngineEvent>>,
}

struct MockTerminalEngine {
    state: Mutex<MockTerminalState>,
}

impl MockTerminalEngine {
    fn new(
        active_thread_id: &str,
        threads: Vec<HydratedThread>,
        meta: Vec<ThreadMeta>,
        snapshots_by_thread: HashMap<String, PtyScreenSnapshot>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(MockTerminalState {
                threads,
                meta,
                active_thread_id: active_thread_id.to_string(),
                snapshots_by_thread,
                ..Default::default()
            }),
        })
    }
}

#[async_trait::async_trait]
impl TuiEnginePort for MockTerminalEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let g = self.state.lock().unwrap();
        Ok(HydratedSession {
            active_thread_id: g.active_thread_id.clone(),
            threads: g.threads.clone(),
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
        Ok(self.state.lock().unwrap().meta.clone())
    }

    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> io::Result<HydratedThread> {
        let mut g = self.state.lock().unwrap();
        g.created_terminals.push((
            parent_thread_id.map(str::to_string),
            cwd.map(str::to_string),
            rows,
            cols,
        ));
        let id = format!("term-{}", g.created_terminals.len());
        let t = HydratedThread {
            id: id.clone(),
            title: title.unwrap_or("Terminal").to_string(),
            turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
        };
        g.threads.push(t.clone());
        g.meta.push(ThreadMeta {
            thread_id: id.clone(),
            parent_thread_id: parent_thread_id.map(str::to_string),
            inherited_turn_count: 0,
            merged: false,
            kind: ThreadKind::Terminal,
            cwd: cwd.map(str::to_string),
            ..ThreadMeta::default()
        });
        g.snapshots_by_thread.insert(
            id,
            make_snapshot_from_lines(&["created shell ready"], rows.max(1), cols.max(1)),
        );
        Ok(t)
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .pty_writes
            .push((thread_id.to_string(), bytes));
        Ok(())
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .pty_resizes
            .push((thread_id.to_string(), rows, cols));
        Ok(())
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> io::Result<PtyScreenSnapshot> {
        let mut g = self.state.lock().unwrap();
        g.pty_snapshots.push((thread_id.to_string(), rows, cols));
        Ok(g.snapshots_by_thread
            .get(thread_id)
            .cloned()
            .unwrap_or_else(|| make_snapshot_from_lines(&["cold snap"], rows.max(1), cols.max(1))))
    }

    async fn ensure_thread_worktree(&self, thread_id: &str) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.worktree_calls.push(thread_id.to_string());
        let wt = format!("/tmp/worktree/{thread_id}");
        if let Some(m) = g.meta.iter_mut().find(|m| m.thread_id == thread_id) {
            m.cwd = Some(wt.clone());
        }
        Ok(wt)
    }
}

async fn wait_for<F>(desc: &str, mut cond: F)
where
    F: FnMut() -> bool,
{
    for _ in 0..120 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!("timed out waiting for: {desc}");
}

#[tokio::test]
async fn test_e2e_cold_reopen_terminal_create_passthrough_zoom_resize_and_worktree() {
    let threads = vec![
        HydratedThread {
            id: "t-agent".to_string(),
            title: "Main".to_string(),
            turns: Vec::new(),
            custom_name: None,
            agent_provider: None,
        },
        HydratedThread {
            id: "t-persisted-term".to_string(),
            title: "Persisted Shell".to_string(),
            turns: Vec::new(),
            custom_name: None,
            agent_provider: None,
        },
    ];
    let meta = vec![
        ThreadMeta {
            thread_id: "t-agent".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
            kind: ThreadKind::Agent,
            cwd: None,
            ..ThreadMeta::default()
        },
        ThreadMeta {
            thread_id: "t-persisted-term".to_string(),
            parent_thread_id: Some("t-agent".to_string()),
            inherited_turn_count: 0,
            merged: false,
            kind: ThreadKind::Terminal,
            cwd: Some("/tmp/persisted".to_string()),
            ..ThreadMeta::default()
        },
    ];
    let mut snaps = HashMap::new();
    snaps.insert(
        "t-persisted-term".to_string(),
        make_snapshot_from_lines(&["COLD_REOPEN_RESTORED"], 20, 60),
    );
    let engine = MockTerminalEngine::new("t-agent", threads, meta, snaps);
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("subscribed", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    // 1. /worktree on t-agent
    for ch in "/worktree".chars() {
        key_tx.send(Event::Key(key(KeyCode::Char(ch)))).unwrap();
    }
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    wait_for("ensure_thread_worktree called", || {
        engine.state.lock().unwrap().worktree_calls == vec!["t-agent".to_string()]
    })
    .await;

    // 2. Cold reopen: switch onto t-persisted-term via Tree -> Enter -> triggers pty_snapshot + pty_resize
    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    wait_for("cold reopen pty_snapshot", || {
        !engine.state.lock().unwrap().pty_snapshots.is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 3. Key passthrough & paste & zoom resize inside FocusedPane::Terminal
    key_tx.send(Event::Key(ctrl('c'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    key_tx
        .send(Event::Paste("pwd".to_string()))
        .unwrap();
    key_tx.send(Event::Key(alt('z'))).unwrap();
    wait_for("zoom resize sent", || {
        engine
            .state
            .lock()
            .unwrap()
            .pty_resizes
            .iter()
            .any(|(id, _, cols)| id == "t-persisted-term" && *cols == 100)
    })
    .await;

    // 4. Ctrl+G back to Tree, press 's' to spawn new child terminal, then q+y quits
    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('s')))).unwrap();
    wait_for("create_terminal called", || {
        !engine.state.lock().unwrap().created_terminals.is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('q')))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('y')))).unwrap();

    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let final_screen = render_screen(&terminal);
    assert!(
        final_screen.contains("created shell ready") || final_screen.contains("COLD_REOPEN_RESTORED"),
        "Expected terminal snapshot rendered:\n{final_screen}"
    );

    let g = engine.state.lock().unwrap();
    assert!(g
        .pty_writes
        .contains(&("t-persisted-term".to_string(), vec![0x03])));
    assert!(g
        .pty_writes
        .contains(&("t-persisted-term".to_string(), b"\r".to_vec())));
    assert!(g
        .pty_writes
        .contains(&("t-persisted-term".to_string(), b"pwd".to_vec())));
    assert!(!g
        .pty_writes
        .iter()
        .any(|(_, b)| b == b"\x07" || b == b"\x14" || b == b"\x1bz" || b == b"\x11"));
}

#[tokio::test]
async fn test_e2e_startup_on_persisted_terminal_loads_snapshot_and_focuses_terminal() {
    let threads = vec![HydratedThread {
        id: "t-term-boot".to_string(),
        title: "Boot Shell".to_string(),
        turns: Vec::new(),
                           custom_name: None,
                           agent_provider: None,
    }];
    let meta = vec![ThreadMeta {
        thread_id: "t-term-boot".to_string(),
        parent_thread_id: None,
        inherited_turn_count: 0,
        merged: false,
        kind: ThreadKind::Terminal,
        cwd: Some("/tmp/boot".to_string()),
        ..ThreadMeta::default()
    }];
    let mut snaps = HashMap::new();
    snaps.insert(
        "t-term-boot".to_string(),
        make_snapshot_from_lines(&["BOOT_TERMINAL_READY"], 20, 60),
    );
    let engine = MockTerminalEngine::new("t-term-boot", threads, meta, snaps);
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("startup pty_snapshot for active terminal", || {
        !engine.state.lock().unwrap().pty_snapshots.is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Immediate keystrokes go straight to PTY because startup auto-focused FocusedPane::Terminal
    key_tx.send(Event::Key(key(KeyCode::Char('l')))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('s')))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    wait_for("startup terminal pty_write", || {
        engine.state.lock().unwrap().pty_writes.len() >= 3
    })
    .await;

    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('q')))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('y')))).unwrap();
    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("BOOT_TERMINAL_READY"), "{screen}");
    assert!(!screen.contains("Composer"), "{screen}");

    let g = engine.state.lock().unwrap();
    assert_eq!(g.pty_snapshots[0].0, "t-term-boot");
    assert!(g.pty_writes.contains(&("t-term-boot".to_string(), b"l".to_vec())));
    assert!(g.pty_writes.contains(&("t-term-boot".to_string(), b"s".to_vec())));
    assert!(g.pty_writes.contains(&("t-term-boot".to_string(), b"\r".to_vec())));
}

fn key_with_kind(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    }
}

#[test]
fn test_key_release_events_are_ignored_while_press_and_repeat_are_handled() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-1".to_string(),
                    title: "One".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-2".to_string(),
                    title: "Two".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
            ],
        },
    );

    // Ctrl+G Press then Ctrl+G Release toggles Composer -> Tree once (ignores Release)
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    app.handle_key_event(key_with_kind(
        KeyCode::Char('g'),
        KeyModifiers::CONTROL,
        KeyEventKind::Press,
    ));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(!app.handle_key_event(key_with_kind(
        KeyCode::Char('g'),
        KeyModifiers::CONTROL,
        KeyEventKind::Release,
    )));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    // Esc Press then Esc Release in Tree returns to Composer without re-triggering
    handle_event(
        &mut app,
        Event::Key(key_with_kind(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
    )
    .unwrap();
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    handle_event(
        &mut app,
        Event::Key(key_with_kind(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )),
    )
    .unwrap();
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Repeat events must still work in Composer and Tree, while Release is ignored
    app.handle_key_event(key_with_kind(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ));
    app.handle_key_event(key_with_kind(
        KeyCode::Char('b'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert_eq!(app.composer_input(), "a");

    // Enter Release via runner::handle_key_event must not clear last_submitted_prompt
    app.last_submitted_prompt = Some("keep-me".to_string());
    grein_tui::runner::handle_key_event(
        &mut app,
        key_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Release),
    )
    .unwrap();
    assert_eq!(app.last_submitted_prompt().as_deref(), Some("keep-me"));

    // Repeat Down in Tree advances selection
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(0);
    app.handle_key_event(key_with_kind(
        KeyCode::Down,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ));
    assert_eq!(app.tree.selected_index(), Some(1));
}

#[test]
fn test_alt_t_opens_or_jumps_to_terminal_and_passes_through_inside_terminal() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent-1".to_string(),
                    title: "Agent 1".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-agent-2".to_string(),
                    title: "Agent 2".to_string(),
                    turns: Vec::new(),
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
                ThreadMeta {
                    thread_id: "t-agent-1".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-agent-2".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
            ],
        },
    );

    // 1. In WRITE Composer -> Alt+T is NOT intercepted by default (stays in Composer)
    app.set_focus(FocusedPane::Composer);
    assert!(!app.handle_key_event(alt('t')));
    assert!(app.pending_actions.is_empty());

    // 2. In MANAGE Tree with t-agent-1 selected -> 's' dispatches CreateTerminal under t-agent-1
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(0);
    assert!(app.handle_key_event(key(KeyCode::Char('s'))));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: Some(ref pid),
            title: None,
            cwd: None,
            ..
        }) if pid == "t-agent-1"
    ));

    // 3. In Tree with t-agent-2 selected -> 's' targets selected t-agent-2; optional :bind jump_terminal alt+t works
    app.tree.select_index(1);
    app.keymap
        .bind(grein_tui::keymap::BindableAction::JumpTerminal, "alt+t")
        .unwrap();
    assert!(app.handle_key_event(alt('t')));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: Some(ref pid),
            title: None,
            cwd: None,
            ..
        }) if pid == "t-agent-2"
    ));

    // 4. With child terminal t-term-1 under t-agent-1 (no snapshot yet):
    //    Bound Alt+T from MANAGE Tree jumps directly to t-term-1, focuses Terminal, and requests PtySnapshot.
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent-1".to_string(),
                    title: "Agent 1".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-term-1".to_string(),
                    title: "Shell".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-agent-2".to_string(),
                    title: "Agent 2".to_string(),
                    turns: Vec::new(),
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
                ThreadMeta {
                    thread_id: "t-agent-1".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term-1".to_string(),
                    parent_thread_id: Some("t-agent-1".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/tmp".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-agent-2".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
            ],
        },
    );
    app.pending_actions.clear();
    app.set_focus(FocusedPane::Tree);
    assert!(app.handle_key_event(alt('t')));
    assert_eq!(app.active_thread_id(), "t-term-1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtySnapshot { ref thread_id, .. }) if thread_id == "t-term-1"
    ));

    // 5. With default keymap inside FocusedPane::Terminal, Alt+T MUST NOT be intercepted — passes through to PTY
    app.keymap = grein_tui::keymap::Keymap::default();
    app.pending_actions.clear();
    assert!(app.handle_key_event(alt('t')));
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "t-term-1".to_string(),
            bytes: b"\x1bt".to_vec(),
        })
    );

    // 6. From Tree hovering on t-term-1 while t-agent-1 is active -> bound JumpTerminal switches to t-term-1 & focuses Terminal
    app.keymap
        .bind(grein_tui::keymap::BindableAction::JumpTerminal, "alt+t")
        .unwrap();
    app.switch_thread("t-agent-1");
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1);
    app.pending_actions.clear();
    assert!(app.handle_key_event(alt('t')));
    assert_eq!(app.active_thread_id(), "t-term-1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert!(!app
        .pending_actions
        .iter()
        .any(|a| matches!(a, AppAction::CreateTerminal { .. })));
}

#[test]
fn test_tree_escape_ctrl_g_right_t_attach_sidebar_unhide_preview_and_exited_pty() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent-1".to_string(),
                    title: "Agent 1".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t-agent-2".to_string(),
                    title: "Agent 2".to_string(),
                    turns: Vec::new(),
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
                ThreadMeta {
                    thread_id: "t-agent-1".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent-1".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/tmp".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-agent-2".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                    ..ThreadMeta::default()
                },
            ],
        },
    );

    // 1. Idle Esc stays in WRITE Composer; Ctrl+G auto-unhides sidebar when entering MANAGE Tree
    app.set_focus(FocusedPane::Composer);
    app.sidebar_hidden = true;
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(!app.sidebar_hidden, "Ctrl+G into Tree must unhide sidebar");

    // 2. Moving Tree selection onto Terminal with missing pty_screens requests PtySnapshot,
    //    and Terminal preview hides the Agent Composer completely.
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Down)); // lands on t-term
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtySnapshot { ref thread_id, .. }) if thread_id == "t-term"
    ));

    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let preview_screen = render_screen(&terminal);
    assert!(
        preview_screen.contains(">_ /tmp (Preview)") && preview_screen.contains("● live"),
        "{preview_screen}"
    );
    assert!(
        !preview_screen.contains("Composer"),
        "Terminal preview in Tree must hide Agent Composer:\n{preview_screen}"
    );

    // 3. 's' in Tree attaches child terminal to selected_tree_thread_info().0 (t-agent-2), not active_thread_id
    app.handle_key_event(key(KeyCode::Char('j'))); // lands on t-agent-2
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Char('s')));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: Some(ref pid),
            cwd: None,
            ..
        }) if pid == "t-agent-2"
    ));

    // 4. In Tree, Esc, Ctrl+G, and Enter activate the currently selected node (entering Terminal when Terminal is selected);
    //    Right on a leaf Terminal stays in MANAGE Tree.
    app.handle_key_event(key(KeyCode::Char('k'))); // back on t-term
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.active_thread_id(), "t-term");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    app.switch_thread("t-agent-1");
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1); // t-term
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.active_thread_id(), "t-term");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    app.switch_thread("t-agent-1");
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1); // t-term
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.active_thread_id(), "t-term");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // 5. Exited PTY in FocusedPane::Terminal:
    //    - Status bar shows "exited · Enter restart · Esc/Ctrl+G manage"
    //    - Further keys / paste do NOT emit PtyWrite
    //    - Esc escapes back to FocusedPane::Tree and unhides sidebar if zoomed
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyScreenUpdated {
            thread_id: "t-term".to_string(),
            snapshot: make_snapshot_from_lines(&["process finished"], 10, 40),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::PtyExited {
            thread_id: "t-term".to_string(),
            exit_code: Some(0),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let exited_screen = render_screen(&terminal);
    assert!(
        exited_screen.contains("exited · Enter restart · Esc/Ctrl+G manage"),
        "Exited terminal status bar must guide user back to MANAGE:\n{exited_screen}"
    );

    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Char('a')));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(ctrl('c'));
    handle_event(&mut app, Event::Paste("echo no".to_string())).unwrap();
    assert!(
        !app.pending_actions
            .iter()
            .any(|a| matches!(a, AppAction::PtyWrite { .. })),
        "Exited PTY must not forward keys or paste to PtyWrite: {:?}",
        app.pending_actions
    );

    app.sidebar_hidden = true;
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(!app.sidebar_hidden, "Esc from exited terminal into Tree must unhide sidebar");
}

#[test]
fn test_terminal_slash_command_aliases_case_insensitive() {
    let mut app = App::new();
    app.active_thread_id = "t-main".to_string();

    for cmd in [
        "/term",
        "/TERM",
        "/terminal",
        "/Terminal",
        "/zsh",
        "/ZSH",
        "/sh",
        "/SH",
        "/shell",
        "/Shell",
        "/bash",
        "/Bash",
    ] {
        app.pending_actions.clear();
        app.submit_composer_prompt(cmd);
        assert!(
            matches!(
                app.pending_actions.pop_front(),
                Some(AppAction::CreateTerminal {
                    parent_thread_id: Some(ref pid),
                    cwd: None,
                    ..
                }) if pid == "t-main"
            ),
            "Command {cmd} should dispatch CreateTerminal"
        );

        app.pending_actions.clear();
        app.submit_composer_prompt(format!("{cmd} \"/tmp/work dir\""));
        assert!(
            matches!(
                app.pending_actions.pop_front(),
                Some(AppAction::CreateTerminal {
                    parent_thread_id: Some(ref pid),
                    cwd: Some(ref path),
                    ..
                }) if pid == "t-main" && path == "/tmp/work dir"
            ),
            "Command {cmd} with path should dispatch CreateTerminal with parsed cwd"
        );
    }
}

#[tokio::test]
async fn test_pty_write_serialization_fifo_order() {
    struct DelayedPtyEngine {
        writes: Mutex<Vec<Vec<u8>>>,
        event_tx: Mutex<Option<mpsc::UnboundedSender<TuiEngineEvent>>>,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for DelayedPtyEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-term".to_string(),
                threads: vec![HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell".to_string(),
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
            Ok(vec![ThreadMeta {
                thread_id: "t-term".to_string(),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
                kind: ThreadKind::Terminal,
                cwd: None,
                ..ThreadMeta::default()
            }])
        }
        async fn pty_snapshot(
            &self,
            _thread_id: &str,
            rows: u16,
            cols: u16,
        ) -> io::Result<PtyScreenSnapshot> {
            Ok(make_snapshot_from_lines(&["ready"], rows.max(1), cols.max(1)))
        }
        async fn pty_write(&self, _thread_id: &str, bytes: Vec<u8>) -> io::Result<()> {
            // Invert timing: earlier bytes sleep longer; unordered tokio::spawn would scramble them!
            let delay_ms = match bytes.first().copied() {
                Some(b'0') => 25,
                Some(b'1') => 15,
                Some(b'2') => 8,
                _ => 1,
            };
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            self.writes.lock().unwrap().push(bytes);
            Ok(())
        }
    }

    let engine = Arc::new(DelayedPtyEngine {
        writes: Mutex::new(Vec::new()),
        event_tx: Mutex::new(None),
    });
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();

    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("subscribed", || {
        engine.event_tx.lock().unwrap().is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;

    let expected: Vec<Vec<u8>> = "0123456789abcdef"
        .bytes()
        .map(|b| vec![b])
        .collect();
    for b in "0123456789abcdef".chars() {
        key_tx.send(Event::Key(key(KeyCode::Char(b)))).unwrap();
    }

    wait_for("all 16 pty_writes delivered", || {
        engine.writes.lock().unwrap().len() == expected.len()
    })
    .await;

    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('q')))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('y')))).unwrap();
    let _terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(engine.writes.lock().unwrap().as_slice(), expected.as_slice());
}

#[test]
fn test_composer_status_bar_and_help_modal_document_terminal_shortcuts() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-main".to_string(),
            threads: vec![HydratedThread {
                id: "t-main".to_string(),
                title: "Main".to_string(),
                turns: Vec::new(),
                              custom_name: None,
                              agent_provider: None,
            }],
        },
    );

    let _shell = grein_tui::app::active_shell_label();
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let idle_screen = render_screen(&terminal);
    assert!(
        idle_screen.contains("WRITE · AGENT") && idle_screen.contains("Ctrl+G manage · Alt+Z sidebar"),
        "Idle Composer status bar missing expected hint:\n{idle_screen}"
    );

    app.thread_statuses
        .insert("t-main".to_string(), "streaming".to_string());
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let busy_screen = render_screen(&terminal);
    assert!(
        busy_screen.contains("Esc cancel · Ctrl+G manage · Alt+Z sidebar"),
        "Busy Composer status bar missing expected hint:\n{busy_screen}"
    );

    app.show_help_modal = true;
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let help_screen = render_screen(&terminal);
    for expected in [
        "WRITE Mode",
        "MANAGE Mode",
        "Alt+H / Alt+L",
        "Ctrl+G",
        "s / S",
        "/term",
        "→/l, ↵",
        "Ctrl+F/B, Ctrl+U/D",
        "Esc, q, ?, or Enter to close",
    ] {
        assert!(
            help_screen.contains(expected),
            "Help modal missing {expected:?}:\n{help_screen}"
        );
    }
}
