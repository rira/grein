use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{App, AppAction, FocusedPane, PendingPermission, SidebarView};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, PtyScreenSnapshot, ThreadMeta, TuiEngineEvent, WorktreeEntry,
};
use grein_tui::keymap::{BindableAction, KeyChord, Keymap};
use grein_tui::render::render_app_mut;
use grein_tui::runner::{apply_engine_event, apply_hydrated_session};
use grein_tui::ui::tree::TreeNode;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
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

fn build_tree_app() -> App {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "r1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "r1".to_string(),
                    title: "Root One".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "c1".to_string(),
                    title: "Child 1".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "g1".to_string(),
                    title: "Grand 1".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "c2".to_string(),
                    title: "Child 2".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t1".to_string(),
                    title: "Terminal".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "r2".to_string(),
                    title: "Root Two".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "r2c1".to_string(),
                    title: "Root Two Child".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "r3".to_string(),
                    title: "Root Three".to_string(),
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
                make_meta("r1", None, ThreadKind::Agent, None),
                make_meta("c1", Some("r1"), ThreadKind::Agent, None),
                make_meta("g1", Some("c1"), ThreadKind::Agent, None),
                make_meta("c2", Some("r1"), ThreadKind::Agent, None),
                make_meta("t1", Some("r1"), ThreadKind::Terminal, Some("/tmp")),
                make_meta("r2", None, ThreadKind::Agent, None),
                make_meta("r2c1", Some("r2"), ThreadKind::Agent, None),
                make_meta("r3", None, ThreadKind::Agent, None),
            ],
        },
    );
    app
}

#[test]
fn test_keymap_shift_arrow_variants_aliases_and_one_to_one_defaults() {
    let km = Keymap::default();
    assert_eq!(
        km.chords_for(BindableAction::PrevSibling),
        &[KeyChord::new(KeyCode::Char('h'), KeyModifiers::ALT)]
    );
    assert_eq!(
        km.chords_for(BindableAction::NextSibling),
        &[KeyChord::new(KeyCode::Char('l'), KeyModifiers::ALT)]
    );
    assert_eq!(
        km.chords_for(BindableAction::TreeUp),
        &[KeyChord::new(KeyCode::Up, KeyModifiers::SHIFT)]
    );
    assert_eq!(
        km.chords_for(BindableAction::TreeDown),
        &[KeyChord::new(KeyCode::Down, KeyModifiers::SHIFT)]
    );
    assert_eq!(
        km.chords_for(BindableAction::TreeParent),
        &[KeyChord::new(KeyCode::Left, KeyModifiers::SHIFT)]
    );
    assert_eq!(
        km.chords_for(BindableAction::TreeChild),
        &[KeyChord::new(KeyCode::Right, KeyModifiers::SHIFT)]
    );

    for (alias, expected) in [
        ("tree_up", BindableAction::TreeUp),
        ("tree_prev", BindableAction::TreeUp),
        ("sibling_up", BindableAction::TreeUp),
        ("tree_down", BindableAction::TreeDown),
        ("tree_next", BindableAction::TreeDown),
        ("sibling_down", BindableAction::TreeDown),
        ("tree_parent", BindableAction::TreeParent),
        ("tree_up_level", BindableAction::TreeParent),
        ("parent_thread", BindableAction::TreeParent),
        ("tree_child", BindableAction::TreeChild),
        ("tree_down_level", BindableAction::TreeChild),
        ("child_thread", BindableAction::TreeChild),
        ("sibling_prev", BindableAction::PrevSibling),
        ("sibling_next", BindableAction::NextSibling),
    ] {
        assert_eq!(alias.parse::<BindableAction>().unwrap(), expected, "{alias}");
        assert!(!expected.description().is_empty());
        assert!(BindableAction::all().contains(&expected));
        assert!(!expected.is_composer_completion());
    }

    assert_eq!(BindableAction::TreeUp.as_str(), "tree_up");
    assert_eq!(BindableAction::TreeDown.as_str(), "tree_down");
    assert_eq!(BindableAction::TreeParent.as_str(), "tree_parent");
    assert_eq!(BindableAction::TreeChild.as_str(), "tree_child");

    assert_eq!(
        km.match_write_key(shift(KeyCode::Up)),
        Some(BindableAction::TreeUp)
    );
    assert_eq!(
        km.match_write_key(shift(KeyCode::Down)),
        Some(BindableAction::TreeDown)
    );
    assert_eq!(
        km.match_write_key(shift(KeyCode::Left)),
        Some(BindableAction::TreeParent)
    );
    assert_eq!(
        km.match_write_key(shift(KeyCode::Right)),
        Some(BindableAction::TreeChild)
    );
    assert!(km.match_composer_completion_key(shift(KeyCode::Up)).is_none());
}

#[test]
fn test_shift_arrows_depth0_and_depth1_sibling_cycling_and_boundary_noops() {
    let mut app = build_tree_app();
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Depth-0 cycling: r1 -> r2 -> r3 -> r1
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "r2");
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "r3");
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "r1");

    // Reverse depth-0 cycling: r1 -> r3 -> r2 -> r1
    assert!(app.handle_key_event(shift(KeyCode::Up)));
    assert_eq!(app.active_thread_id(), "r3");
    assert!(app.handle_key_event(shift(KeyCode::Up)));
    assert_eq!(app.active_thread_id(), "r2");
    assert!(app.handle_key_event(shift(KeyCode::Up)));
    assert_eq!(app.active_thread_id(), "r1");

    // Depth-1 cycling under r1: c1 -> c2 -> t1 -> c1 (never crosses into r2c1 or g1)
    app.switch_thread("c1");
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "c2");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Stepping into Terminal child auto-switches focus to Terminal (WRITE mode preserved)
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "t1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Wrap from t1 -> c1 auto-switches focus back to Composer
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "c1");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Only-child under r2 stays a no-op
    app.switch_thread("r2c1");
    assert!(!app.step_sibling_at_current_depth(true));
    assert!(!app.step_sibling_at_current_depth(false));
    assert_eq!(app.active_thread_id(), "r2c1");
}

#[test]
fn test_shift_left_right_ascend_descend_memory_unfold_and_deletion_fallback() {
    let mut app = build_tree_app();
    assert_eq!(app.active_thread_id(), "r1");

    // Root boundary: Shift+Left is a no-op
    assert!(!app.ascend_to_parent_thread());
    assert_eq!(app.active_thread_id(), "r1");

    // Descend first time -> first direct child c1
    assert!(app.handle_key_event(shift(KeyCode::Right)));
    assert_eq!(app.active_thread_id(), "c1");

    // Move to sibling c2, then ascend -> records c2 as last_visited_child for r1
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "c2");
    assert!(app.handle_key_event(shift(KeyCode::Left)));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.last_visited_child.get("r1").map(String::as_str), Some("c2"));

    // Collapse r1 in tree, then descend via Shift+Right -> auto-unfolds r1 and picks remembered c2
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('H')));
    assert!(app.collapsed_threads.contains("r1"));
    app.set_focus(FocusedPane::Composer);

    assert!(app.handle_key_event(shift(KeyCode::Right)));
    assert_eq!(app.active_thread_id(), "c2");
    assert!(!app.collapsed_threads.contains("r1"));
    assert!(
        app.tree
            .nodes
            .iter()
            .any(|n| matches!(n, TreeNode::Thread(t) if t.id == "c2")),
        "Unfolded child c2 must appear in tree nodes"
    );

    // Leaf boundary: c2 has no children -> Shift+Right is a no-op
    assert!(!app.descend_to_child_thread());
    assert_eq!(app.active_thread_id(), "c2");

    // Remembered Terminal child t1 auto-focuses Terminal on descend
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "t1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert!(app.handle_key_event(shift(KeyCode::Left)));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert!(app.handle_key_event(shift(KeyCode::Right)));
    assert_eq!(app.active_thread_id(), "t1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // If remembered child t1 is deleted, descending from r1 falls back to first live child c1
    assert!(app.handle_key_event(shift(KeyCode::Left)));
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadDeleted {
            thread_id: "t1".to_string(),
            next_active_thread_id: "r1".to_string(),
        },
    );
    assert!(app.handle_key_event(shift(KeyCode::Right)));
    assert_eq!(app.active_thread_id(), "c1");
}

#[test]
fn test_terminal_alt_screen_passthrough_and_primary_screen_navigation() {
    let mut app = build_tree_app();
    app.switch_thread("t1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    // Alternate screen active -> Shift+Arrows pass \x1b[1;2A..D to PtyWrite without switching threads
    app.pty_screens.insert(
        "t1".to_string(),
        PtyScreenSnapshot {
            alternate_screen: true,
            ..PtyScreenSnapshot::default()
        },
    );
    app.pending_actions.clear();

    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Right, KeyCode::Left] {
        assert!(app.handle_key_event(shift(code)));
    }
    assert_eq!(app.active_thread_id(), "t1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);

    let writes: Vec<Vec<u8>> = app
        .pending_actions
        .drain(..)
        .filter_map(|a| match a {
            AppAction::PtyWrite { bytes, .. } => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(
        writes,
        vec![
            b"\x1b[1;2A".to_vec(),
            b"\x1b[1;2B".to_vec(),
            b"\x1b[1;2C".to_vec(),
            b"\x1b[1;2D".to_vec(),
        ]
    );

    // Primary screen -> Shift+Up steps to sibling c2 and switches focus to Composer
    app.pty_screens.get_mut("t1").unwrap().alternate_screen = false;
    assert!(app.handle_key_event(shift(KeyCode::Up)));
    assert_eq!(app.active_thread_id(), "c2");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
}

#[test]
fn test_composer_popups_single_line_and_unbound_shift_arrow_guards() {
    let mut app = build_tree_app();
    app.worktrees = vec![
        WorktreeEntry {
            path: "/repo/wt-a".to_string(),
            repo_root: "/repo".to_string(),
            repo_name: "repo".to_string(),
            branch: Some("feat-a".to_string()),
            head_short: Some("abc".to_string()),
            is_primary: false,
            is_detached: false,
            is_grein_managed: true,
            dirty_count: 0,
            attached_thread_ids: Vec::new(),
        },
        WorktreeEntry {
            path: "/repo/wt-b".to_string(),
            repo_root: "/repo".to_string(),
            repo_name: "repo".to_string(),
            branch: Some("feat-b".to_string()),
            head_short: Some("def".to_string()),
            is_primary: false,
            is_detached: false,
            is_grein_managed: true,
            dirty_count: 0,
            attached_thread_ids: Vec::new(),
        },
    ];

    // Single-line Composer: plain Up/Down stay false
    app.input_buffer = "single line".to_string();
    app.cursor_position = app.input_buffer.len();
    assert!(!app.handle_key_event(key(KeyCode::Up)));
    assert!(!app.handle_key_event(key(KeyCode::Down)));

    // Active /wt popup does NOT block Shift+Down tree navigation
    app.input_buffer = "/wt ".to_string();
    app.cursor_position = app.input_buffer.len();
    app.sync_worktree_autocomplete(true);
    assert!(app.worktree_autocomplete_open);
    assert!(app.handle_key_event(key(KeyCode::Down)));
    assert_eq!(app.worktree_autocomplete_idx, 1);
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "r2");

    // Active inline ghost-text autocomplete does NOT block Shift+Up tree navigation
    app.autocomplete_recent_prompts = vec!["hello_world_prompt".to_string()];
    app.input_buffer = "hello".to_string();
    app.cursor_position = app.input_buffer.len();
    app.sync_composer_completion();
    assert!(app.composer_completion.is_some());
    assert!(app.handle_key_event(shift(KeyCode::Up)));
    assert_eq!(app.active_thread_id(), "r1");

    // In MANAGE Worktrees view, Shift+Down switches sidebar back to Grein and steps root
    app.set_focus(FocusedPane::Tree);
    app.sidebar_view = SidebarView::Worktrees;
    assert!(app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.active_thread_id(), "r2");
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    // Unbinding tree_up/down/parent/child must NOT fall back to plain arrow movement anywhere
    for action in [
        BindableAction::TreeUp,
        BindableAction::TreeDown,
        BindableAction::TreeParent,
        BindableAction::TreeChild,
    ] {
        app.keymap.bind(action, "none").unwrap();
    }

    // Composer: unbound Shift+Arrows never move cursor
    app.set_focus(FocusedPane::Composer);
    app.input_buffer = "line1\nline2".to_string();
    app.cursor_position = 8; // middle of line2
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right] {
        assert!(!app.handle_key_event(shift(code)));
        assert_eq!(app.cursor_position, 8);
    }

    // /wt popup: unbound Shift+Up/Down never cycle popup
    app.input_buffer = "/wt ".to_string();
    app.cursor_position = 4;
    app.sync_worktree_autocomplete(true);
    assert!(app.worktree_autocomplete_open);
    assert_eq!(app.worktree_autocomplete_idx, 0);
    assert!(!app.handle_key_event(shift(KeyCode::Down)));
    assert!(!app.handle_key_event(shift(KeyCode::Up)));
    assert_eq!(app.worktree_autocomplete_idx, 0);

    // Grein Tree & Worktrees Tree: unbound Shift+Arrows do not move tree selection
    app.set_focus(FocusedPane::Tree);
    app.sidebar_view = SidebarView::Grein;
    let sel_before = app.tree.selected_index();
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right] {
        app.handle_key_event(shift(code));
        assert_eq!(app.tree.selected_index(), sel_before);
    }

    app.sidebar_view = SidebarView::Worktrees;
    app.worktree_selected_idx = 1;
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right] {
        app.handle_key_event(shift(code));
        assert_eq!(app.worktree_selected_idx, 1);
    }
}

#[test]
fn test_help_modal_at_110x30_fits_without_scroll_and_displays_shift_arrows() {
    let mut app = App::new();
    app.show_help_modal = true;
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    assert_eq!(app.help_max_scroll, 0);

    let screen = render_screen(&terminal);
    assert!(
        screen.contains("Alt+H / Alt+L"),
        "Help modal must preserve Alt+H / Alt+L:\n{screen}"
    );
    assert!(
        screen.contains("Shift+↑/↓/←/→"),
        "Help modal must display Shift+↑/↓/←/→ hint:\n{screen}"
    );
    assert!(
        screen.contains("Esc, q, ?, or Enter to close"),
        "Help modal must fit cleanly without scrolling at 110x30:\n{screen}"
    );
}

#[test]
fn test_unbound_shift_arrows_in_legacy_transcript_do_not_scroll() {
    let mut app = build_tree_app();
    app.transcript_viewport_mut("r1").set_max_scroll(50);
    app.transcript_viewport_mut("r1").scroll_up(25);
    app.set_focus(FocusedPane::Transcript);

    // Plain Up/Down scroll the legacy Transcript pane when unbound
    assert!(app.handle_key_event(key(KeyCode::Up)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 24);
    assert!(app.handle_key_event(key(KeyCode::Down)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 25);

    // Unbind tree_up/down/parent/child; Shift+Up/Down must NOT fall through to plain Up/Down
    for action in [
        BindableAction::TreeUp,
        BindableAction::TreeDown,
        BindableAction::TreeParent,
        BindableAction::TreeChild,
    ] {
        app.keymap.bind(action, "none").unwrap();
    }

    assert!(!app.handle_key_event(shift(KeyCode::Up)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 25);
    assert!(!app.handle_key_event(shift(KeyCode::Down)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 25);
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.focused_pane(), FocusedPane::Transcript);
}

#[test]
fn test_super_hyper_meta_modifiers_never_alias_shift_or_plain_arrows() {
    let km = Keymap::default();
    let shift_up_chord = KeyChord::new(KeyCode::Up, KeyModifiers::SHIFT);

    for extra in [KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META] {
        let mod_shift_up = KeyEvent::new(KeyCode::Up, extra | KeyModifiers::SHIFT);
        let mod_up = KeyEvent::new(KeyCode::Up, extra);
        let mod_down = KeyEvent::new(KeyCode::Down, extra);
        let mod_shift_down = KeyEvent::new(KeyCode::Down, extra | KeyModifiers::SHIFT);
        let mod_tab = KeyEvent::new(KeyCode::Tab, extra);
        let mod_shift_tab = KeyEvent::new(KeyCode::Tab, extra | KeyModifiers::SHIFT);
        let mod_ctrl_g = KeyEvent::new(KeyCode::Char('g'), extra | KeyModifiers::CONTROL);
        let mod_alt_z = KeyEvent::new(KeyCode::Char('z'), extra | KeyModifiers::ALT);

        assert!(!shift_up_chord.matches(mod_shift_up));
        assert_eq!(km.match_write_key(mod_shift_up), None);
        assert_eq!(km.match_write_key(mod_up), None);
        assert_eq!(km.match_write_key(mod_shift_down), None);
        assert_eq!(km.match_write_key(mod_down), None);
        assert_eq!(km.match_write_key(mod_ctrl_g), None);
        assert_eq!(km.match_write_key(mod_alt_z), None);
        assert_eq!(km.match_composer_completion_key(mod_tab), None);
        assert_eq!(km.match_composer_completion_key(mod_shift_tab), None);
    }

    // Valid CONTROL, ALT, SHIFT, and AltGr chords continue to match/ignore properly
    assert_eq!(
        km.match_write_key(shift(KeyCode::Up)),
        Some(BindableAction::TreeUp)
    );
    assert_eq!(
        km.match_write_key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        )),
        None
    );

    // Central key router must reject SUPER/HYPER/META across all panes
    let mut app = build_tree_app();
    app.input_buffer = "line1\nline2".to_string();
    app.cursor_position = 8;
    app.transcript_viewport_mut("r1").set_max_scroll(50);
    app.transcript_viewport_mut("r1").scroll_up(25);

    // 1. Composer
    app.set_focus(FocusedPane::Composer);
    for ev in [
        KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER | KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER),
        KeyEvent::new(KeyCode::Down, KeyModifiers::SUPER | KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Down, KeyModifiers::HYPER),
        KeyEvent::new(KeyCode::Up, KeyModifiers::META),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER),
    ] {
        assert!(!app.handle_key_event(ev), "Composer should ignore {ev:?}");
    }
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.cursor_position, 8);
    assert_eq!(app.input_buffer, "line1\nline2");

    // AltGr remains intact in Composer
    assert!(app.handle_key_event(KeyEvent::new(
        KeyCode::Char('@'),
        KeyModifiers::CONTROL | KeyModifiers::ALT
    )));
    assert_eq!(app.input_buffer, "line1\nli@ne2");

    // 2. Tree (Grein & Worktrees)
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1);
    for ev in [
        KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER | KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER),
        KeyEvent::new(KeyCode::Down, KeyModifiers::SUPER),
    ] {
        assert!(!app.handle_key_event(ev), "Tree should ignore {ev:?}");
        assert!(app.handle_key_tree(ev).is_none());
    }
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.tree.selected_index(), Some(1));

    // 3. Legacy Transcript
    app.set_focus(FocusedPane::Transcript);
    for ev in [
        KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER | KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER),
        KeyEvent::new(KeyCode::Down, KeyModifiers::SUPER | KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Down, KeyModifiers::SUPER),
    ] {
        assert!(!app.handle_key_event(ev), "Transcript should ignore {ev:?}");
    }
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 25);

    // 4. Terminal (primary & alternate screen)
    app.switch_thread("t1");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    app.pending_actions.clear();
    assert!(!app.handle_key_event(KeyEvent::new(
        KeyCode::Up,
        KeyModifiers::SUPER | KeyModifiers::SHIFT
    )));
    app.pty_screens.insert(
        "t1".to_string(),
        PtyScreenSnapshot {
            alternate_screen: true,
            ..PtyScreenSnapshot::default()
        },
    );
    assert!(!app.handle_key_event(KeyEvent::new(
        KeyCode::Up,
        KeyModifiers::SUPER | KeyModifiers::SHIFT
    )));
    assert_eq!(app.active_thread_id(), "t1");
    assert!(app.pending_actions.is_empty());

    // 5. Worktrees Tree view and active modals also ignore SUPER/HYPER/META
    app.set_focus(FocusedPane::Tree);
    app.sidebar_view = SidebarView::Worktrees;
    app.worktree_selected_idx = 0;
    assert!(!app.handle_key_event(KeyEvent::new(
        KeyCode::Down,
        KeyModifiers::SUPER | KeyModifiers::SHIFT
    )));
    assert!(!app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::SUPER)));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.worktree_selected_idx, 0);

    app.push_permission(PendingPermission {
        request_id: "perm-1".to_string(),
        description: "Test".to_string(),
    });
    assert!(!app.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::SUPER)));
    assert!(app.pending_permission.is_some());
    assert!(app.pending_actions.is_empty());
}
