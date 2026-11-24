use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{App, AppAction, FocusedPane, SidebarView};
use grein_tui::engine_port::{
    AgentSlashCommand, HydratedSession, HydratedThread, ThreadMeta, TuiEngineEvent, TuiEnginePort,
    UserPreferencesSnapshot, WorktreeEntry,
};
use grein_tui::keymap::{BindableAction, Keymap};
use grein_tui::render::fuzzy_match_positions;
use grein_tui::runner::{apply_engine_event, apply_hydrated_session, draw_frame_mut, run_with_engine_events};
use grein_tui::theme::{ThemePalette, ThemePreset};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
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
            dirty_count: 2,
            attached_thread_ids: vec!["t-agent".to_string(), "t-term".to_string()],
        },
    ]
}

fn setup_app() -> App {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-main".to_string(),
                    title: "Main Trunk".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-agent".to_string(),
                    title: "Auth Agent".to_string(),
                    custom_name: Some("Auth".to_string()),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Terminal".to_string(),
                    custom_name: None,
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
                    kind: ThreadKind::Agent,
                    cwd: Some("/repos/alpha".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-agent".to_string(),
                    parent_thread_id: Some("t-main".to_string()),
                    kind: ThreadKind::Agent,
                    cwd: Some("/repos/alpha/.grein/worktrees/feat-auth".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
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
fn test_composer_ghost_autocomplete_tiers_controls_toggle_and_terminal_isolation() {
    let mut app = setup_app();
    app.set_focus(FocusedPane::Composer);
    app.autocomplete_workspace_files = vec![
        "crates/grein-tui/src/keymap.rs".to_string(),
        "crates/grein-tui/src/engine_port.rs".to_string(),
    ];
    app.autocomplete_recent_prompts = vec![
        "refactor the autocomplete engine".to_string(),
        "implement tiered completion".to_string(),
    ];
    app.active_transcript_md =
        "## Agent (agy)\nCheck `sync_composer_completion` and `ActiveCompletion`.\n\n".to_string();
    apply_engine_event(
        &mut app,
        TuiEngineEvent::AgentCommandsUpdated {
            thread_id: "t-agent".to_string(),
            commands: vec![AgentSlashCommand {
                name: "compact".to_string(),
                description: "Compact transcript".to_string(),
            }],
        },
    );

    // 1. Keymap defaults keep Terminal PTY untouched via match_write_key vs match_composer_completion_key
    let km = Keymap::default();
    assert_eq!(km.match_write_key(key(KeyCode::Tab)), None);
    assert_eq!(km.match_write_key(ctrl('y')), None);
    assert_eq!(km.match_write_key(ctrl('n')), None);
    assert_eq!(km.match_write_key(ctrl('p')), None);
    assert_eq!(km.match_write_key(ctrl('e')), None);
    assert_eq!(
        km.match_composer_completion_key(key(KeyCode::Tab)),
        Some(BindableAction::AcceptCompletion)
    );
    assert_eq!(
        km.match_composer_completion_key(ctrl('y')),
        Some(BindableAction::AcceptCompletion)
    );
    assert_eq!(
        km.match_composer_completion_key(shift(KeyCode::BackTab)),
        Some(BindableAction::NextCompletion)
    );
    assert_eq!(
        km.match_composer_completion_key(ctrl('n')),
        Some(BindableAction::NextCompletion)
    );
    assert_eq!(
        km.match_composer_completion_key(ctrl('p')),
        Some(BindableAction::PrevCompletion)
    );
    assert_eq!(
        km.match_composer_completion_key(ctrl('e')),
        Some(BindableAction::DismissCompletion)
    );

    // 2. Tier 1 slash command + dynamic ACP command
    type_str(&mut app, "/comp");
    let ac = app.composer_completion.as_ref().expect("slash completion");
    assert_eq!(ac.active().unwrap().full_text, "/compact");
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.composer_input(), "/compact");

    // Tier 1 @file completion
    app.input_buffer.clear();
    app.cursor_position = 0;
    type_str(&mut app, "inspect @key");
    assert_eq!(
        app.composer_completion
            .as_ref()
            .and_then(|c| c.active())
            .map(|s| s.full_text.as_str()),
        Some("@keymap.rs")
    );
    app.handle_key_event(ctrl('y'));
    assert_eq!(app.composer_input(), "inspect @keymap.rs");

    // Tier 2 transcript identifier + recent prompt + Tier 3 dictionary word with cycling & ghost rendering
    app.input_buffer.clear();
    app.cursor_position = 0;
    type_str(&mut app, "sync_");
    assert_eq!(
        app.composer_completion
            .as_ref()
            .and_then(|c| c.active())
            .map(|s| s.full_text.as_str()),
        Some("sync_composer_completion")
    );
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.composer_input(), "sync_composer_completion");

    // Tier 3 dictionary capitalized prefix + Shift+Tab / Ctrl+N / Ctrl+P cycling + ghost render
    app.input_buffer.clear();
    app.cursor_position = 0;
    type_str(&mut app, "Imple");
    let initial = app.composer_completion.as_ref().unwrap();
    let total = initial.matches.len();
    assert!(total >= 2, "expected multiple matches for Imple: {:?}", initial.matches);
    let first_text = initial.active().unwrap().full_text.clone();
    assert_eq!(first_text, "Implement");

    // Render frame and check muted ghost text + top-right counter hint + cursor position preserved
    let mut terminal = Terminal::new(TestBackend::new(110, 26)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("Implement"), "ghost text missing:\n{screen}");
    assert!(
        screen.contains(&format!("Tab/C-y accept · 1/{total}")),
        "title counter hint missing:\n{screen}"
    );
    let palette = app.theme_palette.clone();
    let buf = terminal.backend().buffer();
    let mut found_muted_ghost = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(byte_idx) = row.find("Implement") {
            let ghost_col = row[..byte_idx].chars().count() as u16 + 5; // right after "Imple"
            if buf[(ghost_col, y)].fg == palette.muted {
                found_muted_ghost = true;
                break;
            }
        }
    }
    assert!(found_muted_ghost, "Ghost suffix must render in theme.muted");

    // Shift+Tab -> next, Ctrl+N -> next, Ctrl+P -> prev
    app.handle_key_event(shift(KeyCode::BackTab));
    assert_eq!(app.composer_completion.as_ref().unwrap().selected_idx, 1);
    app.handle_key_event(ctrl('p'));
    assert_eq!(app.composer_completion.as_ref().unwrap().selected_idx, 0);
    app.handle_key_event(ctrl('n'));
    assert_eq!(app.composer_completion.as_ref().unwrap().selected_idx, 1);
    let second_suffix = app
        .composer_completion
        .as_ref()
        .unwrap()
        .active()
        .unwrap()
        .insert_suffix
        .clone();
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.composer_input(), format!("Imple{second_suffix}"));

    // 3. Ctrl+E dismisses ghost text, second Ctrl+E acts as readline EOL, typing resets dismissed
    app.input_buffer = "Imple tail".to_string();
    app.cursor_position = 5;
    app.composer_completion_dismissed = false;
    app.sync_composer_completion();
    assert!(app.composer_completion.is_some());
    app.handle_key_event(ctrl('e'));
    assert!(app.composer_completion.is_none());
    assert!(app.composer_completion_dismissed);
    assert_eq!(app.cursor_position, 5, "first Ctrl+E only dismisses ghost");
    app.handle_key_event(ctrl('e'));
    assert_eq!(
        app.cursor_position,
        "Imple tail".len(),
        "second Ctrl+E falls through to EOL"
    );
    assert!(app.composer_completion.is_none());

    // Ctrl+Y yanks kill_ring when NO completion is active
    app.kill_ring = "!".to_string();
    app.handle_key_event(ctrl('y'));
    assert_eq!(app.composer_input(), "Imple tail!");

    // 4. Mid-line ghost does not shift existing characters on screen
    app.input_buffer = "Imple tail".to_string();
    app.cursor_position = 5;
    app.composer_completion_dismissed = false;
    app.sync_composer_completion();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let mid_screen = render_screen(&terminal);
    assert!(mid_screen.contains("Implement"), "{mid_screen}");

    // 5. :autocomplete off / on and preference persistence
    app.pending_actions.clear();
    app.execute_sidebar_command("autocomplete off");
    assert!(!app.composer_autocomplete_enabled);
    assert!(app.composer_completion.is_none());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SaveUiPreference {
            key: "composer_autocomplete".to_string(),
            value: "off".to_string(),
        })
    );
    app.set_focus(FocusedPane::Composer);
    app.input_buffer.clear();
    app.cursor_position = 0;
    type_str(&mut app, "Imple");
    assert!(app.composer_completion.is_none());

    app.submit_composer_prompt("/autocomplete on");
    assert!(app.composer_autocomplete_enabled);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SaveUiPreference {
            key: "composer_autocomplete".to_string(),
            value: "on".to_string(),
        })
    );

    let mut ui = BTreeMap::new();
    ui.insert("composer_autocomplete".to_string(), "false".to_string());
    app.apply_preferences(&UserPreferencesSnapshot {
        ui,
        ..Default::default()
    });
    assert!(!app.composer_autocomplete_enabled);

    // 6. FocusedPane::Terminal PTY keeps Tab, Shift+Tab, Ctrl+Y, Ctrl+N, Ctrl+P, Ctrl+E 100% untouched
    app.switch_thread("t-term");
    app.set_focus(FocusedPane::Terminal);
    app.pending_actions.clear();
    for (ev, expected_bytes) in [
        (key(KeyCode::Tab), vec![b'\t']),
        (shift(KeyCode::BackTab), b"\x1b[Z".to_vec()),
        (ctrl('y'), vec![0x19]),
        (ctrl('n'), vec![0x0e]),
        (ctrl('p'), vec![0x10]),
        (ctrl('e'), vec![0x05]),
    ] {
        assert!(app.handle_key_event(ev));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::PtyWrite {
                thread_id: "t-term".to_string(),
                bytes: expected_bytes,
            })
        );
    }
}

#[test]
fn test_help_modal_vim_slash_fuzzy_filter_highlighting_and_section_pruning() {
    let (score, pos) = fuzzy_match_positions("Toggle sidebar zoom (full width)", "zoom")
        .expect("contiguous zoom should match");
    assert!(score > 0);
    assert_eq!(pos, vec![15, 16, 17, 18]);

    let mut app = setup_app();
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('?')));
    assert!(app.show_help_modal);
    assert!(!app.help_filter_active);
    assert!(app.help_filter_query.is_empty());

    // '/' enters filter typing mode
    app.handle_key_event(key(KeyCode::Char('/')));
    assert!(app.help_filter_active);
    type_str(&mut app, "hide");
    assert_eq!(app.help_filter_query, "hide");

    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let filtered = render_screen(&terminal);
    assert!(filtered.contains("Filter: hide█"), "{filtered}");
    assert!(
        filtered.contains("Keybindings & Commands ("),
        "Filtered title must show (matched/total):\n{filtered}"
    );
    assert!(
        filtered.contains("Toggle sidebar zoom"),
        "Invisible keyword 'hide' must match zoom entry:\n{filtered}"
    );
    assert!(
        !filtered.contains("Worktrees View"),
        "Sections with zero matches must be pruned:\n{filtered}"
    );

    // Highlight check on a direct description match ("zoom")
    app.handle_key_event(ctrl('u'));
    assert!(app.help_filter_query.is_empty());
    type_str(&mut app, "zoom");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let zoom_screen = render_screen(&terminal);
    assert!(zoom_screen.contains("Filter: zoom█"), "{zoom_screen}");
    let palette = app.theme_palette.clone();
    let buf = terminal.backend().buffer();
    let mut found_highlighted_zoom = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(byte_idx) = row.find("Toggle sidebar zoom") {
            let z_col = row[..byte_idx].chars().count() as u16 + "Toggle sidebar ".len() as u16;
            let cell = &buf[(z_col, y)];
            if cell.fg == palette.accent && cell.modifier.contains(Modifier::BOLD) {
                found_highlighted_zoom = true;
                break;
            }
        }
    }
    assert!(
        found_highlighted_zoom,
        "Matched chars in description must use theme.accent + BOLD"
    );

    // Enter locks filter (keeps query, removes cursor █, allows j/k/Esc)
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.help_filter_active);
    assert_eq!(app.help_filter_query, "zoom");
    assert!(app.show_help_modal);
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let locked = render_screen(&terminal);
    assert!(locked.contains("Filter: zoom "), "{locked}");
    assert!(!locked.contains("Filter: zoom█"), "{locked}");

    // First Esc clears locked query without closing modal; second Esc closes modal
    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.show_help_modal);
    assert!(app.help_filter_query.is_empty());
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.show_help_modal);
}

#[test]
fn test_worktrees_target_sync_exit_to_grein_attached_names_footer_and_new_agent() {
    let mut app = setup_app();

    // Start active on Terminal t-term, move Tree selection to Auth Agent t-agent, then press Tab -> syncs active_thread_id!
    app.switch_thread("t-term");
    assert_eq!(app.active_thread_id(), "t-term");
    app.set_focus(FocusedPane::Tree);
    let agent_idx = app
        .tree
        .nodes
        .iter()
        .position(|n| matches!(n, grein_tui::ui::tree::TreeNode::Thread(t) if t.id == "t-agent"))
        .unwrap();
    app.tree.select_index(agent_idx);
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(
        app.active_thread_id(),
        "t-agent",
        "Tab to Worktrees must sync active_thread_id to highlighted Grein thread"
    );

    // Render Worktrees view: check Target: footer and human-readable Attached: names
    let mut terminal = Terminal::new(TestBackend::new(120, 28)).unwrap();
    app.worktree_selected_idx = 2; // feat/auth (attached: t-agent, t-term)
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);
    assert!(
        screen.contains("Target: [Auth]") && screen.contains("· 1 repos · 2 wt"),
        "Worktrees footer missing Target: label:\n{screen}"
    );
    app.sidebar_layout.mode = grein_tui::app::SidebarMode::Fixed;
    app.sidebar_layout.fixed_cols = 46;
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let wide_wt_screen = render_screen(&terminal);
    assert!(
        wide_wt_screen.contains("Target: [Auth] Auth Agent · 1 repos · 2 wt"),
        "Wide Worktrees footer missing full Target: title:\n{wide_wt_screen}"
    );
    app.sidebar_layout.mode = grein_tui::app::SidebarMode::Dynamic;
    assert!(
        screen.contains("Attached: [Auth] Auth Agent, worktree: feat-auth"),
        "Attached: must show human-readable display titles instead of raw IDs:\n{screen}"
    );
    assert!(
        !screen.contains("Attached: t-agent"),
        "Raw IDs must not appear in Attached:\n{screen}"
    );
    assert!(
        screen.contains("[↵]attach [n]ew [s]hell [w]orktree [p]rune"),
        "Status bar must advertise [n]ew:\n{screen}"
    );

    // Enter attaches feat/auth to t-agent (NOT t-term!) and flips back to Grein + Composer
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SwitchThreadWorktree {
            thread_id: "t-agent".to_string(),
            target_path: "/repos/alpha/.grein/worktrees/feat-auth".to_string(),
        })
    );
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Ctrl+G and Esc exiting Worktrees also return sidebar_view to Grein
    app.handle_key_event(ctrl('g'));
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    app.handle_key_event(ctrl('g'));
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    app.handle_key_event(ctrl('g'));
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // 's' in Worktrees flips sidebar_view to Grein; Tab returns to Worktrees for 'n'
    app.handle_key_event(ctrl('g'));
    app.handle_key_event(key(KeyCode::Tab));
    app.worktree_selected_idx = 2;
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Char('s')));
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal { cwd: Some(ref p), .. }) if p == "/repos/alpha/.grein/worktrees/feat-auth"
    ));

    // 'n' on WorktreeRow::Entry queues CreateRoot with cwd and returns to Grein + Composer
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    app.worktree_selected_idx = 2;
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateRoot {
            title: None,
            cwd: Some("/repos/alpha/.grein/worktrees/feat-auth".to_string()),
        })
    );
    assert_eq!(app.sidebar_view, SidebarView::Grein);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Composer is NEVER hidden behind Diff Preview while in WRITE mode even if sidebar_view == Worktrees (e.g. Alt+Z zoom)
    app.handle_key_event(ctrl('g'));
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    app.handle_key_event(alt('z'));
    assert!(app.sidebar_hidden);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    app.handle_key_event(alt('z'));
    assert!(!app.sidebar_hidden);
    assert_eq!(app.sidebar_view, SidebarView::Worktrees);
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let write_screen = render_screen(&terminal);
    assert!(write_screen.contains("Composer"), "{write_screen}");
    assert!(!write_screen.contains("⎇ Diff ·"), "{write_screen}");
}

#[test]
fn test_16_theme_scrollable_picker_renders_swatches_and_tone_tags_at_short_height() {
    let mut app = setup_app();
    app.execute_sidebar_command("theme");
    assert!(app.theme_picker.is_some());

    let mut short_term = Terminal::new(TestBackend::new(80, 14)).unwrap();
    draw_frame_mut(&mut short_term, &mut app).unwrap();
    let top_screen = render_screen(&short_term);
    assert!(top_screen.contains("Tokyo Night (saved)"), "{top_screen}");
    assert!(top_screen.contains("● ● ● ● ●"), "{top_screen}");
    assert!(top_screen.contains("dark"), "{top_screen}");

    // Verify swatch dot fg colors match ThemePalette::from_preset(TokyoNight).swatch_colors()
    let expected_swatch = ThemePalette::from_preset(ThemePreset::TokyoNight).swatch_colors();
    let buf = short_term.backend().buffer();
    let mut dot_colors = Vec::new();
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if row.contains("Tokyo Night (saved)") {
            for x in 0..buf.area.width {
                if buf[(x, y)].symbol() == "●" {
                    dot_colors.push(buf[(x, y)].fg);
                }
            }
            break;
        }
    }
    assert_eq!(dot_colors.as_slice(), &expected_swatch);

    // Scroll to bottom (index 15 = Rose Pine Dawn, light tone)
    app.handle_key_event(key(KeyCode::End));
    assert_eq!(app.theme_preset, ThemePreset::RosePineDawn);
    draw_frame_mut(&mut short_term, &mut app).unwrap();
    let bottom_screen = render_screen(&short_term);
    assert!(
        bottom_screen.contains("Rose Pine Dawn") && bottom_screen.contains("light"),
        "Scrolling to bottom of 16 themes at 80x14 must reveal Rose Pine Dawn + light tone:\n{bottom_screen}"
    );
    assert!(
        !bottom_screen.contains("Tokyo Night (saved)"),
        "Top item must scroll out at 80x14 when bottom selected:\n{bottom_screen}"
    );

    // Check ANSI 'term' tone tag is visible when scrolled up slightly
    app.handle_key_event(key(KeyCode::Home));
    for _ in 0..3 {
        app.handle_key_event(key(KeyCode::Down));
    }
    assert_eq!(app.theme_preset, ThemePreset::Ansi);
    draw_frame_mut(&mut short_term, &mut app).unwrap();
    let ansi_screen = render_screen(&short_term);
    assert!(ansi_screen.contains("ANSI") && ansi_screen.contains("term"), "{ansi_screen}");
}

#[tokio::test]
async fn test_runner_hydrates_autocomplete_context_and_creates_root_in_cwd() {
    struct Wave3MockEngine {
        prompt_limits: Mutex<Vec<usize>>,
        file_roots: Mutex<Vec<Option<String>>>,
        create_root_calls: Mutex<Vec<(Option<String>, Option<String>)>>,
        event_tx: Mutex<Option<mpsc::UnboundedSender<TuiEngineEvent>>>,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for Wave3MockEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-main".to_string(),
                threads: vec![HydratedThread {
                    id: "t-main".to_string(),
                    title: "Main".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                }],
            })
        }
        async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
            Ok(())
        }
        async fn fork_thread(&self, _source_thread_id: &str, _anchor_id: &str) -> io::Result<String> {
            Ok("t-fork".to_string())
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
        async fn list_recent_user_prompts(&self, limit: usize) -> io::Result<Vec<String>> {
            self.prompt_limits.lock().unwrap().push(limit);
            Ok(vec!["refactor workspace scanner".to_string()])
        }
        async fn list_workspace_files(&self, root: Option<&str>) -> io::Result<Vec<String>> {
            self.file_roots.lock().unwrap().push(root.map(str::to_string));
            Ok(vec!["crates/grein-tui/src/app/completion.rs".to_string()])
        }
        async fn list_worktrees(&self) -> io::Result<Vec<WorktreeEntry>> {
            Ok(vec![WorktreeEntry {
                repo_root: "/repos/alpha".to_string(),
                repo_name: "repo-a".to_string(),
                path: "/repos/alpha/.grein/worktrees/feat-new".to_string(),
                branch: Some("feat/new".to_string()),
                head_short: None,
                is_primary: false,
                is_detached: false,
                is_grein_managed: true,
                dirty_count: 0,
                attached_thread_ids: Vec::new(),
            }])
        }
        async fn switch_thread_worktree(&self, _tid: &str, target: &str) -> io::Result<String> {
            Ok(target.to_string())
        }
        async fn create_root_in_cwd(
            &self,
            title: Option<&str>,
            cwd: Option<&str>,
        ) -> io::Result<HydratedThread> {
            self.create_root_calls
                .lock()
                .unwrap()
                .push((title.map(str::to_string), cwd.map(str::to_string)));
            Ok(HydratedThread {
                id: "t-wt-root".to_string(),
                title: "Root".to_string(),
                custom_name: None,
                turns: Vec::new(),
            })
        }
    }

    let engine = Arc::new(Wave3MockEngine {
        prompt_limits: Mutex::new(Vec::new()),
        file_roots: Mutex::new(Vec::new()),
        create_root_calls: Mutex::new(Vec::new()),
        event_tx: Mutex::new(None),
    });
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(110, 26)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    for _ in 0..50 {
        if !engine.prompt_limits.lock().unwrap().is_empty()
            && !engine.file_roots.lock().unwrap().is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(engine.prompt_limits.lock().unwrap().as_slice(), &[200]);

    // Startup hydrated prompt & file should autocomplete with Tab in Composer
    for ch in "ref".chars() {
        key_tx.send(Event::Key(key(KeyCode::Char(ch)))).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    key_tx.send(Event::Key(key(KeyCode::Tab))).unwrap();
    key_tx.send(Event::Key(ctrl('c'))).unwrap(); // clear input

    // Ctrl+G -> Tab to Worktrees -> Down twice to feat/new -> 'n' creates root in cwd
    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Tab))).unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('n')))).unwrap();

    for _ in 0..50 {
        if !engine.create_root_calls.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(40)).await;

    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('q')))).unwrap();
    let _terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(
        engine.create_root_calls.lock().unwrap().as_slice(),
        &[(None, Some("/repos/alpha/.grein/worktrees/feat-new".to_string()))]
    );
}
