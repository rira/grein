use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use grein_core::ThreadKind;
use grein_tui::app::{
    App, AppAction, FocusedPane, PendingPermission, PendingWorktreeDelete, SidebarView,
};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, ThreadMeta, TuiEngineEvent, TuiEnginePort, WorktreeEntry,
};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, draw_frame_mut, handle_event,
    run_with_engine_events,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
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

fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
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

fn sample_worktrees() -> Vec<WorktreeEntry> {
    vec![
        WorktreeEntry {
            repo_root: "/repos/alpha".to_string(),
            repo_name: "repo-a".to_string(),
            path: "/repos/alpha".to_string(),
            branch: Some("main".to_string()),
            head_short: Some("1111111".to_string()),
            is_primary: true,
            is_detached: false,
            is_grein_managed: false,
            dirty_count: 0,
            attached_thread_ids: vec!["t-main".to_string()],
        },
        WorktreeEntry {
            repo_root: "/repos/alpha".to_string(),
            repo_name: "repo-a".to_string(),
            path: "/repos/alpha/.grein/worktrees/feat-auth".to_string(),
            branch: Some("feat/auth".to_string()),
            head_short: Some("2222222".to_string()),
            is_primary: false,
            is_detached: false,
            is_grein_managed: true,
            dirty_count: 3,
            attached_thread_ids: vec!["t-auth".to_string(), "t-term".to_string()],
        },
        WorktreeEntry {
            repo_root: "/repos/alpha".to_string(),
            repo_name: "repo-a".to_string(),
            path: "/repos/alpha/.grein/worktrees/feat-auth-v2".to_string(),
            branch: None,
            head_short: Some("deadbeef".to_string()),
            is_primary: false,
            is_detached: true,
            is_grein_managed: true,
            dirty_count: 0,
            attached_thread_ids: Vec::new(),
        },
        WorktreeEntry {
            repo_root: "/repos/beta".to_string(),
            repo_name: "repo-b".to_string(),
            path: "/repos/beta".to_string(),
            branch: Some("develop".to_string()),
            head_short: Some("3333333".to_string()),
            is_primary: true,
            is_detached: false,
            is_grein_managed: false,
            dirty_count: 1,
            attached_thread_ids: Vec::new(),
        },
    ]
}

fn setup_multi_repo_app() -> App {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-auth".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-main".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-auth".to_string(),
                    title: "Auth".to_string(),
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
                    thread_id: "t-main".to_string(),
                    parent_thread_id: None,
                    kind: ThreadKind::Agent,
                    cwd: Some("/repos/alpha".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-auth".to_string(),
                    parent_thread_id: Some("t-main".to_string()),
                    kind: ThreadKind::Agent,
                    cwd: Some("/repos/alpha/.grein/worktrees/feat-auth/crates/core".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-auth".to_string()),
                    kind: ThreadKind::Terminal,
                    cwd: Some("/repos/alpha/.grein/worktrees/feat-auth".to_string()),
                    ..ThreadMeta::default()
                },
            ],
        },
    );
    app.worktrees = sample_worktrees();
    app
}

#[test]
fn test_ctrl_t_toggles_views_unhides_zoom_and_works_in_terminal_passthrough() {
    let mut app = setup_multi_repo_app();
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Ctrl+T from Composer -> Worktrees view, focuses Tree, queues ListWorktrees
    assert!(app.handle_key_event(ctrl('t')));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(!app.sidebar_hidden);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ListWorktrees)
    );

    // Ctrl+T again -> returns to Grein view and focuses Tree without ListWorktrees
    assert!(app.handle_key_event(ctrl('t')));
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.pending_actions.is_empty());

    // Enter live Terminal thread, zoom out with Alt+Z, then press Ctrl+T inside Terminal passthrough
    app.switch_thread("t-term");
    app.set_focus(FocusedPane::Terminal);
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    let mut terminal = Terminal::new(TestBackend::new(110, 26)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();

    app.pending_actions.clear();
    assert!(app.handle_key_event(alt('z')));
    assert!(app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert!(
        app.pending_actions
            .iter()
            .any(|a| matches!(a, AppAction::PtyResize { thread_id, cols: 110, .. } if thread_id == "t-term")),
        "Alt+Z must resize Terminal PTY: {:?}",
        app.pending_actions
    );
    assert!(
        !app.pending_actions
            .iter()
            .any(|a| matches!(a, AppAction::PtyWrite { .. })),
        "Alt+Z must NOT forward to PtyWrite"
    );

    // Now Ctrl+T while zoomed inside Terminal unhides sidebar -> Worktrees, focuses Tree, queues ListWorktrees & PtyResize
    app.pending_actions.clear();
    assert!(app.handle_key_event(ctrl('t')));
    assert!(!app.sidebar_hidden);
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app
        .pending_actions
        .iter()
        .any(|a| matches!(a, AppAction::ListWorktrees)));
    assert!(
        app.pending_actions
            .iter()
            .any(|a| matches!(a, AppAction::PtyResize { thread_id, cols, .. } if thread_id == "t-term" && *cols < 110)),
        "Ctrl+T unhiding Terminal sidebar must emit PtyResize: {:?}",
        app.pending_actions
    );
    assert!(
        !app.pending_actions
            .iter()
            .any(|a| matches!(a, AppAction::PtyWrite { .. })),
        "Ctrl+T must NOT forward \\x14 to PtyWrite"
    );

    // While unhidden inside Terminal, Ctrl+T still switches directly to Worktrees without PtyWrite
    app.sidebar_view = SidebarView::Grein;
    app.set_focus(FocusedPane::Terminal);
    app.pending_actions.clear();
    assert!(app.handle_key_event(ctrl('t')));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ListWorktrees)
    );
}

#[test]
fn test_alt_z_toggles_zoom_globally_without_interfering_with_ctrl_t() {
    let mut app = setup_multi_repo_app();

    for pane in [
        FocusedPane::Composer,
        FocusedPane::Tree,
        FocusedPane::Transcript,
    ] {
        app.set_focus(pane);
        assert!(!app.sidebar_hidden);
        assert!(app.handle_key_event(alt('z')));
        assert!(app.sidebar_hidden, "Alt+Z must hide sidebar from {pane:?}");
        assert!(app.handle_key_event(alt('z')));
        assert!(!app.sidebar_hidden, "Alt+Z must restore sidebar from {pane:?}");
    }

    // Preserve Worktrees view when Alt+Z hides & restores sidebar
    app.handle_key_event(ctrl('t'));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert!(app.handle_key_event(alt('z')));
    assert!(app.sidebar_hidden);
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert!(app.handle_key_event(alt('z')));
    assert!(!app.sidebar_hidden);
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);

    // 'z' in Worktrees sidebar and '/zoom' also toggle zoom
    app.set_focus(FocusedPane::Tree);
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(app.sidebar_hidden);
    app.submit_composer_prompt("/zoom");
    assert!(!app.sidebar_hidden);
}

#[test]
fn test_worktrees_sidebar_grouping_longest_prefix_fold_filter_actions_and_delete_modal() {
    let mut app = setup_multi_repo_app();
    app.handle_key_event(ctrl('t'));
    app.pending_actions.clear();

    let mut terminal = Terminal::new(TestBackend::new(120, 28)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);

    // Tab strip and per-repo grouping + longest path prefix ● active indicator
    let status = render_rect(&terminal, Rect::new(0, 27, 120, 1));
    assert!(
        status.contains("Enter attach · t term · w/n new · d/x del · / filter · Ctrl+T tree"),
        "{status}"
    );
    assert!(screen.contains("ᛇ Grein │ ⎇ Worktrees"), "{screen}");
    assert!(screen.contains("▾ repo-a (3)"), "{screen}");
    assert!(screen.contains("▾ repo-b (1)"), "{screen}");
    assert!(screen.contains("main (primary)"), "{screen}");
    assert!(screen.contains("● feat/auth"), "{screen}");
    assert!(screen.contains("*3"), "{screen}");
    assert!(screen.contains("[2]"), "{screen}");
    assert!(screen.contains("detached@deadbeef"), "{screen}");
    assert!(screen.contains("develop (primary)"), "{screen}");
    assert!(!screen.contains("● main"), "Primary must NOT get ● when thread is in feat-auth:\n{screen}");
    assert!(
        !screen.contains("● detached@deadbeef"),
        "Sibling prefix feat-auth-v2 must NOT get ●:\n{screen}"
    );

    // Verify tab strip styling (Grein dim, Worktrees cyan+bold) and yellow *3
    let buf = terminal.backend().buffer();
    let mut worktrees_tab_cyan_bold = false;
    let mut grein_tab_dim = false;
    let mut dirty_yellow = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(byte_idx) = row.find("ᛇ Grein") {
            let col = row[..byte_idx].chars().count() as u16;
            if buf[(col, y)].fg == Color::DarkGray {
                grein_tab_dim = true;
            }
        }
        if let Some(byte_idx) = row.find("⎇ Worktrees") {
            let col = row[..byte_idx].chars().count() as u16;
            let c = &buf[(col, y)];
            if c.fg == Color::Cyan && c.modifier.contains(Modifier::BOLD) {
                worktrees_tab_cyan_bold = true;
            }
        }
        if let Some(byte_idx) = row.find("*3") {
            let col = row[..byte_idx].chars().count() as u16;
            if buf[(col, y)].fg == Color::Yellow {
                dirty_yellow = true;
            }
        }
    }
    assert!(grein_tab_dim, "Inactive Grein tab must be DarkGray");
    assert!(worktrees_tab_cyan_bold, "Active Worktrees tab must be Cyan+Bold");
    assert!(dirty_yellow, "Dirty badge *3 must be Yellow");

    // Navigation & folding: 0=repo-a header, 1=main, 2=feat/auth, 3=detached@deadbeef, 4=repo-b header, 5=develop
    assert_eq!(app.worktree_selected_idx, 0);
    app.handle_key_event(key(KeyCode::Char('j')));
    app.handle_key_event(key(KeyCode::Char('j')));
    assert_eq!(app.worktree_selected_idx, 2); // feat/auth

    // 'h' on worktree entry moves up to its repo header; 'h' on repo header collapses it
    app.handle_key_event(key(KeyCode::Char('h')));
    assert_eq!(app.worktree_selected_idx, 0);
    app.handle_key_event(key(KeyCode::Char('h')));
    assert!(app.collapsed_worktree_repos.contains("/repos/alpha"));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let folded = render_screen(&terminal);
    assert!(folded.contains("▸ repo-a (3)"), "{folded}");
    assert!(!folded.contains("feat/auth"), "{folded}");

    // 'l' unfolds repo-a
    app.handle_key_event(key(KeyCode::Char('l')));
    assert!(!app.collapsed_worktree_repos.contains("/repos/alpha"));

    // Inline '/' filter intercepts ahead of transcript search
    app.handle_key_event(key(KeyCode::Char('/')));
    assert!(app.worktree_filter_active);
    assert!(!app.transcript_search.is_visible());
    type_str(&mut app, "auth");
    assert_eq!(app.worktree_filter_query, "auth");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let filtered_screen = render_screen(&terminal);
    assert!(filtered_screen.contains("/auth"), "{filtered_screen}");
    assert!(filtered_screen.contains("▾ repo-a (2)"), "{filtered_screen}");
    assert!(!filtered_screen.contains("repo-b"), "{filtered_screen}");
    assert!(!filtered_screen.contains("main (primary)"), "{filtered_screen}");

    // Enter exits filter editing while keeping filtered list
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.worktree_filter_active);
    assert_eq!(app.worktree_filter_query, "auth");

    // Filter mode Esc exits & clears query
    app.handle_key_event(key(KeyCode::Char('/')));
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.worktree_filter_active);
    assert!(app.worktree_filter_query.is_empty());

    // Case-insensitive filter by repo_name and head_short
    app.handle_key_event(key(KeyCode::Char('/')));
    type_str(&mut app, "DEADBEEF");
    app.handle_key_event(key(KeyCode::Down));
    assert!(!app.worktree_filter_active);
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let sha_filtered = render_screen(&terminal);
    assert!(sha_filtered.contains("detached@deadbeef"), "{sha_filtered}");
    assert!(!sha_filtered.contains("feat/auth "), "{sha_filtered}");
    app.handle_key_event(key(KeyCode::Esc)); // clears remaining filter query
    assert!(app.worktree_filter_query.is_empty());

    // Actions on rows: r refresh, Enter on header folds, Enter on entry switches worktree, t creates terminal, w/n ensures worktree
    app.handle_key_event(key(KeyCode::Char('r')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ListWorktrees)
    );
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Refreshing worktrees...")
    );

    // Select feat/auth (idx 2)
    app.worktree_selected_idx = 2;
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SwitchThreadWorktree {
            thread_id: "t-auth".to_string(),
            target_path: "/repos/alpha/.grein/worktrees/feat-auth".to_string(),
        })
    );
    assert_eq!(app.status_notice.as_deref(), Some("Switching worktree..."));

    app.handle_key_event(key(KeyCode::Char('t')));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal {
            parent_thread_id: Some(ref pid),
            title: Some(ref title),
            cwd: Some(ref cwd),
            ..
        }) if pid == "t-auth" && title.contains("feat/auth") && cwd == "/repos/alpha/.grein/worktrees/feat-auth"
    ));

    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::EnsureThreadWorktree {
            thread_id: "t-auth".to_string(),
        })
    );
    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::EnsureThreadWorktree {
            thread_id: "t-auth".to_string(),
        })
    );

    // Primary deletion guard
    app.worktree_selected_idx = 1; // main (primary)
    app.handle_key_event(key(KeyCode::Char('d')));
    assert!(app.pending_worktree_delete.is_none());
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Cannot delete primary repository worktree")
    );

    // Clean non-primary delete modal: detached@deadbeef (idx 3)
    app.worktree_selected_idx = 3;
    app.handle_key_event(key(KeyCode::Char('x')));
    assert_eq!(
        app.pending_worktree_delete,
        Some(PendingWorktreeDelete {
            path: "/repos/alpha/.grein/worktrees/feat-auth-v2".to_string(),
            label: "detached@deadbeef".to_string(),
            dirty_count: 0,
            attached_count: 0,
            force: false,
        })
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let clean_modal = render_screen(&terminal);
    assert!(clean_modal.contains("Delete Worktree"), "{clean_modal}");
    assert!(
        clean_modal.contains("Delete worktree detached@deadbeef? [y/N]"),
        "{clean_modal}"
    );
    assert!(clean_modal.contains("feat-auth-v2"), "{clean_modal}");
    // Unrelated keys ignored while modal open; 'n' cancels
    app.handle_key_event(key(KeyCode::Char('j')));
    assert!(app.pending_worktree_delete.is_some());
    app.handle_key_event(key(KeyCode::Char('n')));
    assert!(app.pending_worktree_delete.is_none());
    assert!(app.pending_actions.is_empty());

    // Dirty + attached worktree delete modal: feat/auth (idx 2) -> dirty=3, attached=2, force=true
    app.worktree_selected_idx = 2;
    app.handle_key_event(key(KeyCode::Char('d')));
    assert_eq!(
        app.pending_worktree_delete,
        Some(PendingWorktreeDelete {
            path: "/repos/alpha/.grein/worktrees/feat-auth".to_string(),
            label: "feat/auth".to_string(),
            dirty_count: 3,
            attached_count: 2,
            force: true,
        })
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let dirty_modal = render_screen(&terminal);
    assert!(
        dirty_modal.contains("Dirty (3 files)! Force delete feat/auth? [y/N]"),
        "{dirty_modal}"
    );
    assert!(
        dirty_modal.contains("2 attached thread(s) will reset to repo root"),
        "{dirty_modal}"
    );

    // PendingPermission takes priority over PendingWorktreeDelete
    app.push_permission(PendingPermission {
        request_id: "perm-1".to_string(),
        description: "Allow tool?".to_string(),
    });
    app.handle_key_event(key(KeyCode::Char('y')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "perm-1".to_string(),
            approved: true,
        })
    );
    assert!(app.pending_worktree_delete.is_some());

    // Now confirm delete with 'y'
    app.handle_key_event(key(KeyCode::Char('y')));
    assert!(app.pending_worktree_delete.is_none());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::RemoveWorktree {
            path: "/repos/alpha/.grein/worktrees/feat-auth".to_string(),
            force: true,
        })
    );
}

#[test]
fn test_composer_worktree_autocomplete_and_slash_commands() {
    let mut app = setup_multi_repo_app();
    // Put repo-b FIRST in self.worktrees, while active thread is in repo-a -> verify active repo prioritized!
    let mut reordered = sample_worktrees();
    let beta = reordered.pop().unwrap();
    reordered.insert(0, beta);
    app.worktrees = reordered;

    // Outside '/worktree ', Tab still cycles branches
    assert_eq!(app.active_thread_id(), "t-auth");
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.active_thread_id(), "t-term");
    app.switch_thread("t-auth");
    app.set_focus(FocusedPane::Composer);
    app.pending_actions.clear();

    // Typing '/worktree ' opens autocomplete and queues ListWorktrees ONCE
    app.worktree_list_in_flight = false;
    type_str(&mut app, "/worktree ");
    assert!(app.worktree_autocomplete_open);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ListWorktrees)
    );
    assert!(app.worktree_list_in_flight);

    let candidates = app.worktree_autocomplete_candidates();
    assert_eq!(candidates.len(), 5);
    assert_eq!(
        candidates[0],
        grein_tui::app::WorktreeAutocompleteItem::CreateNew
    );
    // Candidates 1..=3 must belong to repo-a (active repo), candidate 4 to repo-b
    match &candidates[1] {
        grein_tui::app::WorktreeAutocompleteItem::Existing(e) => assert_eq!(e.repo_name, "repo-a"),
        other => panic!("expected Existing repo-a, got {other:?}"),
    }
    match &candidates[4] {
        grein_tui::app::WorktreeAutocompleteItem::Existing(e) => assert_eq!(e.repo_name, "repo-b"),
        other => panic!("expected Existing repo-b last, got {other:?}"),
    }

    // Render popup above Composer
    let mut terminal = Terminal::new(TestBackend::new(110, 26)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let popup_screen = render_screen(&terminal);
    assert!(
        popup_screen.contains("Worktrees (Tab complete · Enter attach)"),
        "{popup_screen}"
    );
    assert!(popup_screen.contains("+ New isolated worktree"), "{popup_screen}");
    assert!(popup_screen.contains("repo-a"), "{popup_screen}");
    assert!(popup_screen.contains("feat/auth"), "{popup_screen}");
    assert!(popup_screen.contains("*3"), "{popup_screen}");

    // Typing filter query 'feat' puts matching worktrees first and CreateNew fallback last (without re-queuing ListWorktrees!)
    type_str(&mut app, "feat");
    assert!(app.pending_actions.is_empty());
    let filtered = app.worktree_autocomplete_candidates();
    assert_eq!(filtered.len(), 3); // feat-auth, feat-auth-v2, CreateNew
    assert!(matches!(
        filtered[0],
        grein_tui::app::WorktreeAutocompleteItem::Existing(ref e) if e.path.ends_with("feat-auth")
    ));
    assert_eq!(
        filtered.last(),
        Some(&grein_tui::app::WorktreeAutocompleteItem::CreateNew)
    );

    // Tab completes selected candidate ahead of branch cycling (does not switch threads)
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.active_thread_id(), "t-auth");
    assert_eq!(
        app.composer_input(),
        "/worktree /repos/alpha/.grein/worktrees/feat-auth"
    );
    assert!(!app.worktree_autocomplete_open);

    // Esc closes popup without leaving Composer; Tab afterwards cycles branches normally
    app.input_buffer.clear();
    app.cursor_position = 0;
    type_str(&mut app, "/wt ");
    assert!(app.worktree_autocomplete_open);
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.worktree_autocomplete_open);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    app.handle_key_event(key(KeyCode::Tab));
    assert_ne!(app.active_thread_id(), "t-auth");

    // Enter on worktree candidate dispatches SwitchThreadWorktree and clears Composer
    app.switch_thread("t-auth");
    app.set_focus(FocusedPane::Composer);
    app.input_buffer.clear();
    app.cursor_position = 0;
    app.worktree_list_in_flight = true;
    type_str(&mut app, "/wt ");
    app.handle_key_event(key(KeyCode::Down)); // select first existing (main)
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.composer_input().is_empty());
    assert!(!app.worktree_autocomplete_open);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SwitchThreadWorktree {
            thread_id: "t-auth".to_string(),
            target_path: "/repos/alpha".to_string(),
        })
    );

    // Enter on '+ New isolated worktree' dispatches EnsureThreadWorktree and clears Composer
    type_str(&mut app, "/worktree ");
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.composer_input().is_empty());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::EnsureThreadWorktree {
            thread_id: "t-auth".to_string(),
        })
    );

    // Multi-line input does not open autocomplete
    app.input_buffer = "/worktree \nsecond line".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(key(KeyCode::Char('x')));
    assert!(!app.worktree_autocomplete_open);

    // Slash commands: /worktrees, /wt <unique>, /worktree <ambiguous>
    app.input_buffer.clear();
    app.pending_actions.clear();
    app.submit_composer_prompt("/worktrees");
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(!app.sidebar_hidden);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ListWorktrees)
    );

    app.set_focus(FocusedPane::Composer);
    app.submit_composer_prompt("/wt develop");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SwitchThreadWorktree {
            thread_id: "t-auth".to_string(),
            target_path: "/repos/beta".to_string(),
        })
    );

    // Ambiguous query 'auth' opens Worktrees sidebar filtered to 'auth'
    app.submit_composer_prompt("/worktree auth");
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert_eq!(app.worktree_filter_query, "auth");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ListWorktrees)
    );

    // Mouse scroll in Worktrees sidebar updates worktree_selected_idx
    app.worktree_filter_query.clear();
    app.worktree_selected_idx = 0;
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }),
    )
    .unwrap();
    assert_eq!(app.worktree_selected_idx, 1);
}

#[derive(Default)]
struct MockWorktreeState {
    worktrees: Vec<WorktreeEntry>,
    meta: Vec<ThreadMeta>,
    list_calls: usize,
    meta_calls: usize,
    switch_calls: Vec<(String, String)>,
    ensure_calls: Vec<String>,
    remove_calls: Vec<(String, bool)>,
    event_tx: Option<mpsc::UnboundedSender<TuiEngineEvent>>,
}

struct MockWorktreeEngine {
    state: Mutex<MockWorktreeState>,
}

#[async_trait::async_trait]
impl TuiEnginePort for MockWorktreeEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        Ok(HydratedSession {
            active_thread_id: "t-main".to_string(),
            threads: vec![HydratedThread {
                id: "t-main".to_string(),
                title: "Main".to_string(),
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
        let mut g = self.state.lock().unwrap();
        g.meta_calls += 1;
        Ok(g.meta.clone())
    }
    async fn list_worktrees(&self) -> io::Result<Vec<WorktreeEntry>> {
        let mut g = self.state.lock().unwrap();
        g.list_calls += 1;
        Ok(g.worktrees.clone())
    }
    async fn ensure_thread_worktree(&self, thread_id: &str) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.ensure_calls.push(thread_id.to_string());
        let new_path = "/repos/alpha/.grein/worktrees/new-wt".to_string();
        g.worktrees.push(WorktreeEntry {
            repo_root: "/repos/alpha".to_string(),
            repo_name: "repo-a".to_string(),
            path: new_path.clone(),
            branch: Some("grein/new-wt".to_string()),
            head_short: Some("9999999".to_string()),
            is_primary: false,
            is_detached: false,
            is_grein_managed: true,
            dirty_count: 0,
            attached_thread_ids: vec![thread_id.to_string()],
        });
        if let Some(m) = g.meta.iter_mut().find(|m| m.thread_id == thread_id) {
            m.cwd = Some(new_path.clone());
            m.git_branch = Some("grein/new-wt".to_string());
        }
        Ok(new_path)
    }
    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.switch_calls
            .push((thread_id.to_string(), target_path.to_string()));
        if let Some(m) = g.meta.iter_mut().find(|m| m.thread_id == thread_id) {
            m.cwd = Some(target_path.to_string());
        }
        Ok(target_path.to_string())
    }
    async fn remove_worktree(&self, path: &str, force: bool) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.remove_calls.push((path.to_string(), force));
        g.worktrees.retain(|w| w.path != path);
        Ok(())
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
async fn test_e2e_runner_worktree_switch_ensure_remove_refresh_list_and_metadata() {
    let engine = Arc::new(MockWorktreeEngine {
        state: Mutex::new(MockWorktreeState {
            worktrees: sample_worktrees(),
            meta: vec![ThreadMeta {
                thread_id: "t-main".to_string(),
                parent_thread_id: None,
                kind: ThreadKind::Agent,
                cwd: Some("/repos/alpha".to_string()),
                git_branch: Some("main".to_string()),
                ..ThreadMeta::default()
            }],
            ..Default::default()
        }),
    });
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(110, 26)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("subscribed", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    // 1. Ctrl+T lists worktrees
    key_tx.send(Event::Key(ctrl('t'))).unwrap();
    wait_for("list_worktrees on Ctrl+T", || {
        engine.state.lock().unwrap().list_calls >= 1
    })
    .await;
    let meta_after_boot = engine.state.lock().unwrap().meta_calls;

    // 2. Move down twice to feat/auth (dirty=3) and press Enter -> switch_thread_worktree + refreshes list_worktrees & thread_metadata
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    wait_for("switch_thread_worktree and refresh", || {
        let g = engine.state.lock().unwrap();
        !g.switch_calls.is_empty() && g.list_calls >= 2 && g.meta_calls > meta_after_boot
    })
    .await;
    let meta_after_switch = engine.state.lock().unwrap().meta_calls;

    // 3. Press 'w' -> ensure_thread_worktree + refreshes list_worktrees & thread_metadata
    key_tx.send(Event::Key(key(KeyCode::Char('w')))).unwrap();
    wait_for("ensure_thread_worktree and refresh", || {
        let g = engine.state.lock().unwrap();
        !g.ensure_calls.is_empty() && g.list_calls >= 3 && g.meta_calls > meta_after_switch
    })
    .await;
    let meta_after_ensure = engine.state.lock().unwrap().meta_calls;

    // 4. Press 'd' then 'y' on dirty feat/auth -> remove_worktree(force=true) + refreshes list_worktrees & thread_metadata
    key_tx.send(Event::Key(key(KeyCode::Char('d')))).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    key_tx.send(Event::Key(key(KeyCode::Char('y')))).unwrap();
    wait_for("remove_worktree and refresh", || {
        let g = engine.state.lock().unwrap();
        !g.remove_calls.is_empty() && g.list_calls >= 4 && g.meta_calls > meta_after_ensure
    })
    .await;

    key_tx.send(Event::Key(ctrl('q'))).unwrap();
    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("grein/new-wt"), "{screen}");
    assert!(!screen.contains("feat/auth"), "{screen}");

    let g = engine.state.lock().unwrap();
    assert_eq!(
        g.switch_calls,
        vec![(
            "t-main".to_string(),
            "/repos/alpha/.grein/worktrees/feat-auth".to_string()
        )]
    );
    assert_eq!(g.ensure_calls, vec!["t-main".to_string()]);
    assert_eq!(
        g.remove_calls,
        vec![(
            "/repos/alpha/.grein/worktrees/feat-auth".to_string(),
            true
        )]
    );
}
