use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{
    palette_command_specs, App, AppAction, FocusedPane, SidebarView,
};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, PtyScreenSnapshot, ThreadMeta, TuiEngineEvent,
    UserPreferencesSnapshot, WorktreeEntry,
};
use grein_tui::keymap::{BindableAction, Keymap, KeymapPreset};
use grein_tui::render::render_app_mut;
use grein_tui::runner::{apply_engine_event, apply_hydrated_session};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
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

fn render_status_row(terminal: &Terminal<TestBackend>, height: u16, width: u16) -> String {
    let buf = terminal.backend().buffer();
    let y = height - 1;
    let mut row = String::new();
    for x in 0..width {
        row.push_str(buf[(x, y)].symbol());
    }
    row
}

fn build_multi_root_app() -> App {
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
                    title: "Child One".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "c2".to_string(),
                    title: "Child Two".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                    agent_provider: None,
                },
                HydratedThread {
                    id: "t1".to_string(),
                    title: "Child Shell".to_string(),
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
                    id: "r3".to_string(),
                    title: "Root Shell".to_string(),
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
                make_meta("c2", Some("r1"), ThreadKind::Agent, None),
                make_meta("t1", Some("r1"), ThreadKind::Terminal, Some("/tmp")),
                make_meta("r2", None, ThreadKind::Agent, None),
                make_meta("r3", None, ThreadKind::Terminal, Some("/tmp")),
            ],
        },
    );
    app.worktrees = vec![WorktreeEntry {
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
    }];
    app
}

#[test]
fn test_manage_plain_digits_jump_to_root_and_enter_write_with_out_of_range_noop() {
    let mut app = build_multi_root_app();
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Ctrl+G then '2' jumps to root 2 (Agent) and returns to WRITE Composer
    assert!(app.handle_key_event(ctrl('g')));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.handle_key_event(key(KeyCode::Char('2'))));
    assert_eq!(app.active_thread_id(), "r2");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.sidebar_view, SidebarView::Grein);

    // Ctrl+G then '3' jumps to root 3 (Terminal) and enters WRITE Terminal
    assert!(app.handle_key_event(ctrl('g')));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.handle_key_event(key(KeyCode::Char('3'))));
    assert_eq!(app.active_thread_id(), "r3");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    assert_eq!(app.sidebar_view, SidebarView::Grein);

    // In Worktrees view, plain '1' jumps back to root 1, switches to Grein, and enters WRITE Composer
    assert!(app.handle_key_event(ctrl('g')));
    assert!(app.handle_key_event(key(KeyCode::Tab)));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert!(app.handle_key_event(key(KeyCode::Char('1'))));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Out-of-range digit '9' stays in MANAGE without changing thread
    assert!(app.handle_key_event(ctrl('g')));
    assert!(app.handle_key_event(key(KeyCode::Char('9'))));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    // Active rename prompt / palette / worktree filter / search editing receive '2' as text instead of jumping
    assert!(app.handle_key_event(key(KeyCode::Char('r'))));
    assert!(app.rename_prompt.is_some());
    assert!(app.handle_key_event(key(KeyCode::Char('2'))));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.rename_prompt.as_ref().unwrap().buffer, "2");
    assert!(app.handle_key_event(key(KeyCode::Esc)));

    assert!(app.handle_key_event(key(KeyCode::Char('/'))));
    assert!(app.transcript_search.editing);
    assert!(app.handle_key_event(key(KeyCode::Char('2'))));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.transcript_search.query, "2");
    assert!(app.handle_key_event(key(KeyCode::Esc)));

    assert!(app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.handle_key_event(key(KeyCode::Char('/'))));
    assert!(app.worktree_filter_active);
    assert!(app.handle_key_event(key(KeyCode::Char('2'))));
    assert_eq!(app.active_thread_id(), "r1");
    assert_eq!(app.worktree_filter_query, "2");
}

#[test]
fn test_ctrl_g_z_symmetric_zoom_toggle_and_non_z_action_resets_flag() {
    let mut app = build_multi_root_app();
    assert!(!app.sidebar_hidden);

    // 1. Ctrl+G then z hides sidebar and returns to WRITE
    assert!(app.handle_key_event(ctrl('g')));
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // 2. While hidden, Ctrl+G unhides sidebar, then z returns to WRITE keeping sidebar visible
    assert!(app.handle_key_event(ctrl('g')));
    assert!(!app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(!app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // 3. After another Ctrl+G z hides it, navigating in tree (non-z action) clears the flag so z hides again
    assert!(app.handle_key_event(ctrl('g')));
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(app.sidebar_hidden);

    assert!(app.handle_key_event(ctrl('g')));
    assert!(!app.sidebar_hidden);
    assert!(app.handle_key_event(key(KeyCode::Char('j'))));
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // 4. Preserved Worktrees view: hidden -> Ctrl+G (Worktrees) -> immediate z stays visible & returns to WRITE
    app.set_focus(FocusedPane::Tree);
    assert!(app.handle_key_event(key(KeyCode::Tab)));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(app.sidebar_hidden);
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);

    assert!(app.handle_key_event(ctrl('g')));
    assert!(!app.sidebar_hidden);
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(!app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // 5. Leaving via Esc clears flag so subsequent Ctrl+G z hides again
    assert!(app.handle_key_event(ctrl('g')));
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(app.sidebar_hidden);
    assert!(app.handle_key_event(ctrl('g')));
    assert!(!app.sidebar_hidden);
    assert!(app.handle_key_event(key(KeyCode::Esc)));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert!(app.handle_key_event(ctrl('g')));
    assert!(app.handle_key_event(key(KeyCode::Char('z'))));
    assert!(app.sidebar_hidden);
}

#[test]
fn test_pageup_pagedown_scroll_in_manage_write_agent_and_pty_passthrough() {
    let mut app = build_multi_root_app();
    app.transcript_viewport_mut("r1").set_max_scroll(100);
    app.transcript_viewport_mut("r1").scroll_to_bottom();
    app.transcript_viewport_mut("c1").set_max_scroll(80);
    app.transcript_viewport_mut("c1").scroll_to_bottom();

    // 1. WRITE · AGENT (Composer): PageUp / PageDown scroll active agent transcript by 10; Shift+PageUp ignored
    app.set_focus(FocusedPane::Composer);
    app.input_buffer = "keep text".to_string();
    app.cursor_position = 4;
    assert!(!app.handle_key_event(shift(KeyCode::PageUp)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);
    assert!(app.handle_key_event(key(KeyCode::PageUp)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 90);
    assert!(app.handle_key_event(key(KeyCode::PageDown)));
    assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);
    assert_eq!(app.input_buffer, "keep text");
    assert_eq!(app.cursor_position, 4);

    // 2. MANAGE · GREIN preview scrolling with PageUp / PageDown
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Down)); // preview c1
    assert!(app.handle_key_event(key(KeyCode::PageUp)));
    assert_eq!(app.transcript_viewport("c1").scroll_offset(), 70);
    assert!(app.handle_key_event(key(KeyCode::PageDown)));
    assert_eq!(app.transcript_viewport("c1").scroll_offset(), 80);

    // 3. MANAGE · WORKTREES diff preview scrolling with PageUp / PageDown
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    app.worktree_diff_max_scroll = 50;
    app.worktree_diff_scroll = 25;
    assert!(app.handle_key_event(key(KeyCode::PageUp)));
    assert_eq!(app.worktree_diff_scroll, 15);
    assert!(app.handle_key_event(key(KeyCode::PageDown)));
    assert_eq!(app.worktree_diff_scroll, 25);

    // 4. WRITE · SHELL PTY passthrough (\x1b[5~ / \x1b[6~)
    app.handle_key_event(key(KeyCode::Char('3'))); // jump to Terminal root r3
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    app.pty_screens.insert(
        "r3".to_string(),
        PtyScreenSnapshot {
            alternate_screen: false,
            ..PtyScreenSnapshot::default()
        },
    );
    app.pending_actions.clear();
    assert!(app.handle_key_event(key(KeyCode::PageUp)));
    assert!(app.handle_key_event(key(KeyCode::PageDown)));
    let writes: Vec<Vec<u8>> = app
        .pending_actions
        .drain(..)
        .filter_map(|a| match a {
            AppAction::PtyWrite { bytes, .. } => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(writes, vec![b"\x1b[5~".to_vec(), b"\x1b[6~".to_vec()]);
}

#[test]
fn test_manage_grein_ctrl_pn_and_bracket_sibling_stepping() {
    let mut app = build_multi_root_app();
    app.switch_thread("c1");
    app.set_focus(FocusedPane::Tree);

    // Step forward with ']' and Ctrl+N: c1 -> c2 -> t1 -> c1 (stays in MANAGE)
    assert!(app.handle_key_event(key(KeyCode::Char(']'))));
    assert_eq!(app.active_thread_id(), "c2");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    assert!(app.handle_key_event(ctrl('n')));
    assert_eq!(app.active_thread_id(), "t1");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    assert!(app.handle_key_event(key(KeyCode::Char(']'))));
    assert_eq!(app.active_thread_id(), "c1");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    // Step backward with '[' and Ctrl+P: c1 -> t1 -> c2 -> c1
    assert!(app.handle_key_event(key(KeyCode::Char('['))));
    assert_eq!(app.active_thread_id(), "t1");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    assert!(app.handle_key_event(ctrl('p')));
    assert_eq!(app.active_thread_id(), "c2");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    assert!(app.handle_key_event(key(KeyCode::Char('['))));
    assert_eq!(app.active_thread_id(), "c1");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);

    // Active search / rename must receive '[' / ']' instead of stepping siblings
    assert!(app.handle_key_event(key(KeyCode::Char('r'))));
    assert!(app.handle_key_event(key(KeyCode::Char('['))));
    assert_eq!(app.rename_prompt.as_ref().unwrap().buffer, "[");
    assert_eq!(app.active_thread_id(), "c1");
    assert!(app.handle_key_event(key(KeyCode::Esc)));

    // Worktrees view ignores '[' / ']' for Grein sibling stepping
    assert!(app.handle_key_event(key(KeyCode::Tab)));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert!(app.handle_key_event(key(KeyCode::Char(']'))));
    assert_eq!(app.active_thread_id(), "c1");
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
}

#[test]
fn test_keymap_preset_commands_persistence_prefs_roundtrip_and_chrome_hints() {
    assert_eq!(Keymap::default().preset, KeymapPreset::Standard);
    assert_eq!("standard".parse::<KeymapPreset>().unwrap(), KeymapPreset::Standard);
    assert_eq!("default".parse::<KeymapPreset>().unwrap(), KeymapPreset::Standard);
    assert_eq!("linux".parse::<KeymapPreset>().unwrap(), KeymapPreset::Standard);
    assert_eq!("macos".parse::<KeymapPreset>().unwrap(), KeymapPreset::MacOs);
    assert_eq!("mac".parse::<KeymapPreset>().unwrap(), KeymapPreset::MacOs);
    assert_eq!("leader".parse::<KeymapPreset>().unwrap(), KeymapPreset::MacOs);
    assert!("invalid".parse::<KeymapPreset>().is_err());

    assert!(palette_command_specs().iter().any(|(cmd, _)| *cmd == "keymap"));

    let mut app = build_multi_root_app();
    // Composer slash autocomplete exposes /keymap
    app.set_focus(FocusedPane::Composer);
    for ch in "/keym".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    assert_eq!(
        app.composer_completion
            .as_ref()
            .and_then(|c| c.active())
            .map(|s| s.full_text.as_str()),
        Some("/keymap")
    );
    app.input_buffer.clear();
    app.cursor_position = 0;

    // Palette :keymap macos -> updates preset, emits SaveKeybindingPreference
    app.set_focus(FocusedPane::Tree);
    app.open_command_palette("keymap macos");
    assert!(app.handle_key_event(key(KeyCode::Enter)));
    assert_eq!(app.keymap.preset, KeymapPreset::MacOs);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SaveKeybindingPreference {
            action: "preset".to_string(),
            binding: "macos".to_string(),
        })
    );

    // Round-trip through BTreeMap & tuple slice via from_prefs and from_prefs_with_default
    let mut prefs_map = BTreeMap::new();
    prefs_map.insert("preset".to_string(), "macos".to_string());
    let km_map = Keymap::from_prefs(&prefs_map);
    assert_eq!(km_map.preset, KeymapPreset::MacOs);
    assert_eq!(
        km_map.match_write_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT)),
        Some(BindableAction::ToggleZoom)
    );

    let slice_prefs = [("preset".to_string(), "standard".to_string())];
    let km_slice = Keymap::from_prefs_with_default(&slice_prefs[..], KeymapPreset::MacOs);
    assert_eq!(km_slice.preset, KeymapPreset::Standard);

    let empty_slice: &[(String, String)] = &[];
    assert_eq!(
        Keymap::from_prefs_with_default(empty_slice, KeymapPreset::MacOs).preset,
        KeymapPreset::MacOs
    );

    app.apply_preferences(&UserPreferencesSnapshot {
        keybindings: prefs_map,
        ..UserPreferencesSnapshot::default()
    });
    assert_eq!(app.keymap.preset, KeymapPreset::MacOs);

    // Chrome status bar in MacOs preset: WRITE · AGENT idle, WRITE · AGENT busy, WRITE · SHELL live
    app.set_focus(FocusedPane::Composer);
    app.status_notice = None;
    let mut terminal = Terminal::new(TestBackend::new(140, 26)).unwrap();
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let agent_idle = render_status_row(&terminal, 26, 140);
    assert!(
        agent_idle.contains("Ctrl+G manage · Ctrl+G z sidebar · PgUp/PgDn scroll · Ctrl+G 1..9 root · /help"),
        "Unexpected MacOs WRITE·AGENT idle hints:\n{agent_idle}"
    );
    assert!(!agent_idle.contains("Alt+"), "{agent_idle}");

    app.thread_statuses.insert("r1".to_string(), "streaming".to_string());
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let agent_busy = render_status_row(&terminal, 26, 140);
    assert!(
        agent_busy.contains("Esc cancel · Ctrl+G manage · Ctrl+G z sidebar"),
        "Unexpected MacOs WRITE·AGENT busy hints:\n{agent_busy}"
    );
    assert!(!agent_busy.contains("Alt+"), "{agent_busy}");
    app.thread_statuses.insert("r1".to_string(), "idle".to_string());

    app.switch_thread("r3"); // Terminal
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let shell_live = render_status_row(&terminal, 26, 140);
    assert!(
        shell_live.contains("Ctrl+G manage · Ctrl+G z sidebar · Ctrl+G 1..9 root"),
        "Unexpected MacOs WRITE·SHELL live hints:\n{shell_live}"
    );
    assert!(!shell_live.contains("Alt+"), "{shell_live}");

    // Custom EnterManageMode (F6) and ToggleZoom (F9) update MacOs chrome hints dynamically
    app.keymap.bind(BindableAction::EnterManageMode, "f6").unwrap();
    app.keymap.bind(BindableAction::ToggleZoom, "f9").unwrap();
    app.switch_thread("r1");
    app.set_focus(FocusedPane::Composer);
    app.status_notice = None;
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let custom_macos = render_status_row(&terminal, 26, 140);
    assert!(
        custom_macos.contains("F6 manage · F9 sidebar · PgUp/PgDn scroll · F6 1..9 root · /help"),
        "Custom EnterManageMode/ToggleZoom must adjust MacOs chrome hints:\n{custom_macos}"
    );
    app.keymap.unbind(BindableAction::EnterManageMode);
    app.keymap.unbind(BindableAction::ToggleZoom);

    // Slash command /keymap standard restores Alt+ hints and saves preference
    app.switch_thread("r1");
    app.pending_actions.clear();
    app.submit_composer_prompt("/keymap standard");
    assert_eq!(app.keymap.preset, KeymapPreset::Standard);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SaveKeybindingPreference {
            action: "preset".to_string(),
            binding: "standard".to_string(),
        })
    );
    app.status_notice = None;
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let std_idle = render_status_row(&terminal, 26, 140);
    assert!(
        std_idle.contains("Ctrl+G manage · Alt+Z sidebar · Alt+K/Alt+J scroll · Alt+H/Alt+L sibling · /help"),
        "Unexpected Standard WRITE·AGENT idle hints:\n{std_idle}"
    );
}

#[test]
fn test_chrome_and_help_modal_omit_unbound_keys_cleanly_and_fit_110x30() {
    let mut app = build_multi_root_app();
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();

    // 1. Help modal in Standard and MacOs presets fits 110x30 with help_max_scroll == 0
    for preset in [KeymapPreset::Standard, KeymapPreset::MacOs] {
        app.keymap.preset = preset;
        app.show_help_modal = true;
        terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
        assert_eq!(app.help_max_scroll, 0, "Help modal overflowed for {preset:?}");
        let screen = render_screen(&terminal);
        match preset {
            KeymapPreset::Standard => {
                assert!(screen.contains("gg / G, 1..9 / Alt+1..9"), "{screen}");
                assert!(screen.contains("PgUp/PgDn"), "{screen}");
                assert!(screen.contains("Alt+H / Alt+L"), "{screen}");
            }
            KeymapPreset::MacOs => {
                assert!(screen.contains("gg / G, 1..9"), "{screen}");
                assert!(!screen.contains("Alt+1..9"), "{screen}");
                assert!(!screen.contains("Alt+Z"), "{screen}");
                assert!(screen.contains("PgUp/PgDn"), "{screen}");
                assert!(screen.contains("Ctrl+G z"), "{screen}");
            }
        }
    }

    // 2. Unbinding ToggleZoom/ScrollUp/ScrollDown/PrevSibling/NextSibling omits dead segments cleanly
    app.show_help_modal = false;
    app.keymap.preset = KeymapPreset::Standard;
    for action in [
        BindableAction::ToggleZoom,
        BindableAction::ScrollUp,
        BindableAction::ScrollDown,
        BindableAction::PrevSibling,
        BindableAction::NextSibling,
    ] {
        app.keymap.bind(action, "none").unwrap();
    }
    app.status_notice = None;
    app.set_focus(FocusedPane::Composer);
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let unbound_status = render_status_row(&terminal, 30, 110);
    assert!(unbound_status.contains("Ctrl+G manage · /help"), "{unbound_status}");
    for dead in ["Alt+Z", "Alt+K", "Alt+J", "Alt+H", "Alt+L", "·  ·"] {
        assert!(!unbound_status.contains(dead), "Dead segment {dead} in:\n{unbound_status}");
    }

    // Also unbind Tree* and completion actions; Help modal must not overflow with "unbound/unbound"
    for action in [
        BindableAction::TreeUp,
        BindableAction::TreeDown,
        BindableAction::TreeParent,
        BindableAction::TreeChild,
        BindableAction::AcceptCompletion,
        BindableAction::NextCompletion,
        BindableAction::PrevCompletion,
        BindableAction::DismissCompletion,
    ] {
        app.keymap.bind(action, "none").unwrap();
    }
    app.show_help_modal = true;
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    assert_eq!(app.help_max_scroll, 0);
    let help_unbound = render_screen(&terminal);
    assert!(!help_unbound.contains("unbound/unbound"), "{help_unbound}");

    let _ = Rect::default();
}
