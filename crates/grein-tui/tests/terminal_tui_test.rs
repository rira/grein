use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
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
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell 1".to_string(),
                    turns: Vec::new(),
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
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/tmp/repo".to_string()),
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
                },
                HydratedThread {
                    id: "t-sub-agent".to_string(),
                    title: "Auth".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-term-live".to_string(),
                    title: "Shell".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-term-exit-ok".to_string(),
                    title: "Logs".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-term-root-fail".to_string(),
                    title: "Build".to_string(),
                    turns: Vec::new(),
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
                },
                ThreadMeta {
                    thread_id: "t-sub-agent".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                },
                ThreadMeta {
                    thread_id: "t-term-live".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
                },
                ThreadMeta {
                    thread_id: "t-term-exit-ok".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
                },
                ThreadMeta {
                    thread_id: "t-term-root-fail".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
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

    assert!(screen.contains("ᛉ Main"), "{screen}");
    assert!(screen.contains("├─ Auth"), "{screen}");
    assert!(!screen.contains(">_ Auth"), "{screen}");
    assert!(screen.contains("├─ >_ Shell"), "{screen}");
    assert!(screen.contains("└─ >_ Logs"), "{screen}");
    assert!(screen.contains("exit 0"), "{screen}");
    assert!(screen.contains(">_ Build"), "{screen}");
    assert!(screen.contains("exit 2"), "{screen}");
    assert!(!screen.contains("ᛉ Build"), "{screen}");
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
                ThreadMeta {
                    thread_id: "t-agent".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: None,
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/tmp".to_string()),
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
    assert!(preview_screen.contains("Terminal (Preview: Shell)"), "{preview_screen}");
    assert!(preview_screen.contains("LIVE_PTY_PREVIEW"), "{preview_screen}");

    // 'l' in Tree enters FocusedPane::Terminal on the selected terminal
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.active_thread_id(), "t-term");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Ctrl+G returns to Tree without forwarding \x07
    app.pending_actions.clear();
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.pending_actions.is_empty());

    // Enter (and Tab) from Tree enters FocusedPane::Terminal
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Render frame so pane dimensions are known, then Ctrl+T toggles zoom & triggers PtyResize without forwarding \x14
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    app.pending_actions.clear();
    assert!(!app.sidebar_hidden);
    app.handle_key_event(ctrl('t'));
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
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.pending_actions.is_empty());
    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "perm-term-2".to_string(),
            approved: false,
        })
    );

    // Ctrl+Q quits cleanly without forwarding \x11
    app.set_focus(FocusedPane::Terminal);
    app.pending_actions.clear();
    app.handle_key_event(ctrl('q'));
    assert!(app.should_quit());
    assert!(app.pending_actions.is_empty());
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
            }],
        },
    );

    // 't' and 'T' in Tree and Transcript
    for pane in [FocusedPane::Tree, FocusedPane::Transcript] {
        app.set_focus(pane);
        app.handle_key_event(key(KeyCode::Char('t')));
        assert!(matches!(
            app.pending_actions.pop_front(),
            Some(AppAction::CreateTerminal {
                parent_thread_id: Some(ref pid),
                cwd: None,
                ..
            }) if pid == "t-main"
        ));

        app.handle_key_event(key(KeyCode::Char('T')));
        assert!(matches!(
            app.pending_actions.pop_front(),
            Some(AppAction::CreateTerminal {
                parent_thread_id: None,
                cwd: None,
                ..
            })
        ));
    }

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
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-main".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: None,
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
    app.handle_key_event(alt('y'));
    assert_eq!(app.last_copied_text.as_deref(), Some("hello pty\nline two"));

    app.last_copied_text = None;
    app.submit_composer_prompt("/copy");
    assert_eq!(app.last_copied_text.as_deref(), Some("hello pty\nline two"));

    // Help modal includes t, T, Ctrl+G, Ctrl+T, /term [path], /worktree
    app.show_help_modal = true;
    let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let help = render_screen(&terminal);
    for expected in ["t / T", "Ctrl+G", "Ctrl+T", "/term [path]", "/worktree"] {
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
        };
        g.threads.push(t.clone());
        g.meta.push(ThreadMeta {
            thread_id: id.clone(),
            parent_thread_id: parent_thread_id.map(str::to_string),
            inherited_turn_count: 0,
            merged: false,
            kind: ThreadKind::Terminal,
            cwd: cwd.map(str::to_string),
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
        },
        HydratedThread {
            id: "t-persisted-term".to_string(),
            title: "Persisted Shell".to_string(),
            turns: Vec::new(),
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
        },
        ThreadMeta {
            thread_id: "t-persisted-term".to_string(),
            parent_thread_id: Some("t-agent".to_string()),
            inherited_turn_count: 0,
            merged: false,
            kind: ThreadKind::Terminal,
            cwd: Some("/tmp/persisted".to_string()),
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
    key_tx.send(Event::Key(ctrl('t'))).unwrap();
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

    // 4. Ctrl+G back to Tree, press 't' to spawn new child terminal, then Ctrl+Q quits
    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('t')))).unwrap();
    wait_for("create_terminal called", || {
        !engine.state.lock().unwrap().created_terminals.is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    key_tx.send(Event::Key(ctrl('q'))).unwrap();

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
        .any(|(_, b)| b == b"\x07" || b == b"\x14" || b == b"\x11"));
}

#[tokio::test]
async fn test_e2e_startup_on_persisted_terminal_loads_snapshot_and_focuses_terminal() {
    let threads = vec![HydratedThread {
        id: "t-term-boot".to_string(),
        title: "Boot Shell".to_string(),
        turns: Vec::new(),
    }];
    let meta = vec![ThreadMeta {
        thread_id: "t-term-boot".to_string(),
        parent_thread_id: None,
        inherited_turn_count: 0,
        merged: false,
        kind: ThreadKind::Terminal,
        cwd: Some("/tmp/boot".to_string()),
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

    key_tx.send(Event::Key(ctrl('q'))).unwrap();
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
