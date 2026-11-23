use super::*;
pub use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::engine_port::ThreadMeta;

fn multi_root_app() -> App {
    let mut app = App {
        active_thread_id: "r1".to_string(),
        threads: vec![
            ThreadItem {
                id: "r1".to_string(),
                title: "Root 1".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
            ThreadItem {
                id: "b1".to_string(),
                title: "Branch 1A".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
            ThreadItem {
                id: "g1".to_string(),
                title: "Grand 1A1".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
            ThreadItem {
                id: "b2".to_string(),
                title: "Branch 1B".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
            ThreadItem {
                id: "r2".to_string(),
                title: "Root 2".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
            ThreadItem {
                id: "r2b".to_string(),
                title: "Root 2 Branch".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
            ThreadItem {
                id: "r3".to_string(),
                title: "Lone Root 3".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
        ],
        ..App::default()
    };
    app.thread_meta.insert(
        "r1".to_string(),
        ThreadMeta {
            thread_id: "r1".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
            kind: grein_core::ThreadKind::Agent,
            cwd: None,
            git_branch: None,
            git_dirty: 0,
            custom_name: None,
        },
    );
    app.thread_meta.insert(
        "b1".to_string(),
        ThreadMeta {
            thread_id: "b1".to_string(),
            parent_thread_id: Some("r1".to_string()),
            inherited_turn_count: 0,
            merged: false,
            kind: grein_core::ThreadKind::Agent,
            cwd: None,
            git_branch: None,
            git_dirty: 0,
            custom_name: None,
        },
    );
    app.thread_meta.insert(
        "g1".to_string(),
        ThreadMeta {
            thread_id: "g1".to_string(),
            parent_thread_id: Some("b1".to_string()),
            inherited_turn_count: 0,
            merged: false,
            kind: grein_core::ThreadKind::Agent,
            cwd: None,
            git_branch: None,
            git_dirty: 0,
            custom_name: None,
        },
    );
    app.thread_meta.insert(
        "b2".to_string(),
        ThreadMeta {
            thread_id: "b2".to_string(),
            parent_thread_id: Some("r1".to_string()),
            inherited_turn_count: 0,
            merged: false,
            kind: grein_core::ThreadKind::Agent,
            cwd: None,
            git_branch: None,
            git_dirty: 0,
            custom_name: None,
        },
    );
    app.thread_meta.insert(
        "r2".to_string(),
        ThreadMeta {
            thread_id: "r2".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
            kind: grein_core::ThreadKind::Agent,
            cwd: None,
            git_branch: None,
            git_dirty: 0,
            custom_name: None,
        },
    );
    app.thread_meta.insert(
        "r2b".to_string(),
        ThreadMeta {
            thread_id: "r2b".to_string(),
            parent_thread_id: Some("r2".to_string()),
            inherited_turn_count: 0,
            merged: false,
            kind: grein_core::ThreadKind::Agent,
            cwd: None,
            git_branch: None,
            git_dirty: 0,
            custom_name: None,
        },
    );
    app.thread_meta.insert(
        "r3".to_string(),
        ThreadMeta {
            thread_id: "r3".to_string(),
            parent_thread_id: None,
            inherited_turn_count: 0,
            merged: false,
            kind: grein_core::ThreadKind::Agent,
            cwd: None,
            git_branch: None,
            git_dirty: 0,
            custom_name: None,
        },
    );
    app.sync_anchors();
    app.select_active_thread_in_tree();
    app
}

#[test]
fn test_initial_focus_is_composer() {
    let app = App::default();
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.focus, FocusedPane::Composer);
}

#[test]
fn test_focus_next_prev_ctrl_g_and_esc_toggle_composer_tree() {
    let mut app = App::default();
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    app.focus_next();
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.focus_prev();
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Idle Esc in Composer stays in WRITE mode; Ctrl+G enters MANAGE (F6/Alt+G dropped),
    // and Esc in MANAGE returns to WRITE mode.
    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    assert!(!app.handle_key_event(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE)));
    assert!(!app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT)));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    app.thread_statuses
        .insert(app.active_thread_id.clone(), "streaming".to_string());
    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.pending_actions.is_empty());

    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CancelTurn {
            thread_id: app.active_thread_id.clone(),
        })
    );
}

#[test]
fn test_tab_switches_manage_views_and_cycle_branch_within_active_root() {
    let mut app = multi_root_app();
    assert_eq!(app.active_thread_id(), "r1");

    // In WRITE (Composer), Tab does not hijack branches across threads
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // cycle_branch_in_active_root still traverses [r1, b1, g1, b2] within r1 subtree
    app.cycle_branch_in_active_root(true);
    assert_eq!(app.active_thread_id(), "b1");
    app.cycle_branch_in_active_root(true);
    assert_eq!(app.active_thread_id(), "g1");
    app.cycle_branch_in_active_root(true);
    assert_eq!(app.active_thread_id(), "b2");
    app.cycle_branch_in_active_root(true);
    assert_eq!(app.active_thread_id(), "r1");

    app.cycle_branch_in_active_root(false);
    assert_eq!(app.active_thread_id(), "b2");

    // Lone root with no children stays a no-op
    app.switch_thread("r3");
    app.cycle_branch_in_active_root(true);
    assert_eq!(app.active_thread_id(), "r3");

    // In MANAGE (Tree), Tab toggles Grein <-> Worktrees
    app.set_focus(FocusedPane::Tree);
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
}

#[test]
fn test_manage_j_k_and_alt_number_switch_roots_with_altgr_guard() {
    let mut app = multi_root_app();
    app.switch_thread("g1"); // nested grandchild under r1
    app.set_focus(FocusedPane::Tree);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT));
    assert_eq!(app.active_thread_id(), "r2");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE));
    assert_eq!(app.active_thread_id(), "r3");

    app.handle_key_event(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE));
    assert_eq!(app.active_thread_id(), "r1");

    app.handle_key_event(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
    assert_eq!(app.active_thread_id(), "r3");

    app.handle_key_event(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::NONE));
    assert_eq!(app.active_thread_id(), "r2");

    // Alt+1..3 jumps to roots 1..3 in both MANAGE and WRITE; AltGr (Ctrl+Alt) never hijacks
    app.set_focus(FocusedPane::Composer);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::ALT)));
    assert_eq!(app.active_thread_id(), "r3");
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT)));
    assert_eq!(app.active_thread_id(), "r1");
    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('2'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.composer_input(), "2");
}

#[test]
fn test_global_transcript_scroll_from_composer_and_tree_preview() {
    let mut app = multi_root_app();
    app.transcript_viewport_mut("r1").set_max_scroll(100);
    app.transcript_viewport_mut("r1").scroll_to_bottom();
    app.transcript_viewport_mut("b1").set_max_scroll(80);
    app.transcript_viewport_mut("b1").scroll_to_bottom();

    // From Composer (WRITE): Alt+K/Alt+J scroll active thread r1; plain Home/End still operate
    app.set_focus(FocusedPane::Composer);
    app.input_buffer = "draft".to_string();
    app.cursor_position = 2;
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 90);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);

    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 0);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 5);
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);

    // From Tree previewing b1 (MANAGE): Ctrl+U/D half-page and Ctrl+B/F full-page scroll b1
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.active_thread_id(), "r1");
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)));
    assert_eq!(app.transcript_viewport("b1").scroll_offset(), 70);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)));
    assert_eq!(app.transcript_viewport("b1").scroll_offset(), 50);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)));
    assert_eq!(app.transcript_viewport("b1").scroll_offset(), 70);
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);
}

#[test]
fn test_composer_typing_immunity_for_q_and_brackets() {
    let mut app = App::new();
    app.set_focus(FocusPane::Composer);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));

    assert!(!app.should_quit());
    assert_eq!(app.composer_content(), "q[]");
}

#[test]
fn test_ctrl_q_and_ctrl_c_safe_in_composer_and_q_quits_in_manage() {
    let mut app1 = App::new();
    app1.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(!app1.should_quit());

    app1.input_buffer = "unsent draft".to_string();
    app1.cursor_position = app1.input_buffer.len();
    app1.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app1.should_quit());
    assert_eq!(app1.composer_content(), "");
    assert_eq!(app1.status_notice.as_deref(), Some("Cleared input"));

    app1.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app1.should_quit());
    assert_eq!(
        app1.status_notice.as_deref(),
        Some("Use Ctrl+G then q (or /quit) to exit")
    );

    app1.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app1.focused_pane(), FocusedPane::Tree);
    app1.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app1.should_quit());
}

#[test]
fn test_composer_input_routing() {
    let mut app = App::default();
    app.set_focus(FocusedPane::Tree);

    // Input ignored when not in Composer
    app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert_eq!(app.composer_input(), "");

    // Focus Composer
    app.set_focus(FocusedPane::Composer);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(app.composer_input(), "hi");
    assert_eq!(app.cursor_position, 2);

    app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.composer_input(), "h");
    assert_eq!(app.cursor_position, 1);
}

#[test]
fn test_composer_command_parsing_and_root_join_guard() {
    let mut app = App::default();
    app.active_thread_id = "t-main".to_string();

    app.input_buffer = "!cargo test".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.active_transcript_md.contains("cargo test"));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::RunShell {
            thread_id: "t-main".to_string(),
            command: "cargo test".to_string(),
        })
    );

    app.input_buffer = "/branch feature-x".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch {
            source_thread_id: "t-main".to_string(),
            title: Some("feature-x".to_string()),
        })
    );

    app.input_buffer = "/fork".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch {
            source_thread_id: "t-main".to_string(),
            title: None,
        })
    );

    // /join and Tree 'm' on a root context set status_notice without emitting MergeBranch
    app.input_buffer = "/join".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.pending_actions.is_empty());
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Cannot join a root context (no parent branch)")
    );

    app.status_notice = None;
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    assert!(app.pending_actions.is_empty());
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Cannot join a root context (no parent branch)")
    );

    app.set_focus(FocusedPane::Composer);
    app.input_buffer = "explain tree".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.active_transcript_md.contains("# User: explain tree"));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SendPrompt {
            thread_id: "t-main".to_string(),
            prompt: "explain tree".to_string(),
        })
    );

    app.input_buffer = "/quit".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.should_quit());
}

#[test]
fn test_permission_modal_intercepts_keys_and_queues_multiple() {
    let mut app = App::default();
    app.push_permission(PendingPermission {
        request_id: "req-1".to_string(),
        description: "Run git status".to_string(),
    });
    app.push_permission(PendingPermission {
        request_id: "req-2".to_string(),
        description: "Write file".to_string(),
    });

    app.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.composer_input(), "");
    assert!(app.pending_actions.is_empty());

    app.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "req-1".to_string(),
            approved: true,
        })
    );
    assert_eq!(
        app.pending_permission.as_ref().map(|p| p.request_id.as_str()),
        Some("req-2")
    );

    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "req-2".to_string(),
            approved: false,
        })
    );
    assert!(app.pending_permission.is_none());
}

#[test]
fn test_tree_fork_and_enter_switches_thread_to_composer() {
    let mut app = App {
        active_thread_id: "t-1".to_string(),
        active_transcript_md: "# User: root\n\n".to_string(),
        threads: vec![
            ThreadItem {
                id: "t-1".to_string(),
                title: "Root".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
            ThreadItem {
                id: "t-2".to_string(),
                title: "Branch".to_string(),
                custom_name: None,
                anchors: Vec::new(),
            },
        ],
        ..App::default()
    };
    app.thread_transcripts
        .insert("t-1".to_string(), "# User: root\n\n".to_string());
    app.thread_transcripts
        .insert("t-2".to_string(), "# User: branch\n\n".to_string());
    app.sync_anchors();

    // Ctrl+N no longer hijacks Composer in WRITE mode
    assert!(!app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)));
    assert!(app.pending_actions.is_empty());

    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch {
            source_thread_id: "t-1".to_string(),
            title: None,
        })
    );

    let t2_idx = app
        .tree
        .nodes
        .iter()
        .position(|n| matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == "t-2"))
        .unwrap();
    app.tree.select_index(t2_idx);
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.active_thread_id(), "t-2");
    assert!(app.active_transcript_md.contains("branch"));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
}
