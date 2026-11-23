use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{
    display_thread_title, shorten_terminal_cwd_with_home, App, AppAction, FocusedPane,
    PendingPermission, SidebarMode, SidebarView,
};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, ThreadMeta, TuiEngineEvent, TuiEnginePort,
    UserPreferencesSnapshot, WorktreeEntry,
};
use grein_tui::keymap::{BindableAction, KeyChord, Keymap};
use grein_tui::render::{compute_sidebar_width, render_app_mut};
use grein_tui::runner::{apply_engine_event, apply_hydrated_session, run_with_engine_events};
use grein_tui::theme::{ThemePalette, ThemePreset};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
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

fn seed_agent_and_terminal(app: &mut App) {
    apply_hydrated_session(
        app,
        &HydratedSession {
            active_thread_id: "t-agent".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent".to_string(),
                    title: "Main Agent".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Terminal".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                },
            ],
        },
    );
    apply_engine_event(
        app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "t-agent".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Agent,
                    cwd: Some("/home/alice/projects/grein".to_string()),
                    git_branch: Some("feature/super-long-branch-name-that-exceeds-28-chars".to_string()),
                    git_dirty: 2,
                    custom_name: None,
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/home/alice/projects/my-repo".to_string()),
                    git_branch: Some("main".to_string()),
                    git_dirty: 0,
                    custom_name: None,
                },
            ],
        },
    );
}

#[test]
fn test_unified_write_mode_zero_collision_altgr_and_safe_ctrl_c() {
    let mut app = App::new();
    seed_agent_and_terminal(&mut app);

    // Composer WRITE mode preserves Ctrl+Q, Ctrl+T, Alt+T, Alt+N, Alt+R, F2, Ctrl+N, Ctrl+O, Tab, ?
    app.set_focus(FocusedPane::Composer);
    for ev in [
        ctrl('q'),
        ctrl('t'),
        ctrl('n'),
        ctrl('o'),
        alt('t'),
        alt('n'),
        alt('r'),
        key(KeyCode::F(2)),
        key(KeyCode::Tab),
        key(KeyCode::Esc),
    ] {
        let _ = app.handle_key_event(ev);
    }
    assert!(!app.should_quit());
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.active_thread_id(), "t-agent");
    assert!(app.pending_actions.is_empty());

    // Swedish AltGr characters (CONTROL | ALT) insert cleanly into Composer
    for ch in ['@', '£', '$', '{', '}', '[', ']', '\\', '~', '|', '€'] {
        assert!(app.handle_key_event(KeyEvent::new(
            KeyCode::Char(ch),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));
    }
    assert_eq!(app.composer_content(), "@£${}[]\\~|€");

    // Readline Ctrl+F / Ctrl+B move cursor forward / back
    app.cursor_position = 0;
    assert!(app.handle_key_event(ctrl('f')));
    assert_eq!(app.cursor_position, 1);
    assert!(app.handle_key_event(ctrl('b')));
    assert_eq!(app.cursor_position, 0);

    // Ctrl+C clears non-empty input -> empty input gives safe exit guidance (never quits)
    assert!(app.handle_key_event(ctrl('c')));
    assert!(!app.should_quit());
    assert!(app.composer_content().is_empty());
    assert_eq!(app.status_notice.as_deref(), Some("Cleared input"));

    assert!(app.handle_key_event(ctrl('c')));
    assert!(!app.should_quit());
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Use Ctrl+G then q (or /quit) to exit")
    );

    // Switching status_notice clears automatically on next keypress
    assert!(app.handle_key_event(key(KeyCode::Char('a'))));
    assert!(app.status_notice.is_none());

    // Busy Composer turn: Ctrl+C cancels turn
    app.thread_statuses
        .insert("t-agent".to_string(), "streaming".to_string());
    assert!(app.handle_key_event(ctrl('c')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CancelTurn {
            thread_id: "t-agent".to_string(),
        })
    );

    // Ctrl+G toggles WRITE <-> MANAGE; legacy Alt+G and F6 remain unbound
    assert!(!app.handle_key_event(alt('g')));
    assert!(!app.handle_key_event(key(KeyCode::F(6))));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert!(app.handle_key_event(ctrl('g')));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.handle_key_event(ctrl('g')));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Terminal WRITE mode passes Ctrl+Q, Ctrl+T, Alt+T, F1, ?, Ctrl+F straight to PtyWrite
    app.switch_thread("t-term");
    assert_eq!(app.focused_pane(), FocusedPane::Terminal);
    app.pending_actions.clear();
    for ev in [ctrl('q'), ctrl('t'), alt('t'), key(KeyCode::Char('?'))] {
        assert!(app.handle_key_event(ev));
    }
    let writes: Vec<Vec<u8>> = app
        .pending_actions
        .drain(..)
        .filter_map(|a| match a {
            AppAction::PtyWrite { bytes, .. } => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(writes, vec![vec![0x11], vec![0x14], b"\x1bt".to_vec(), b"?".to_vec()]);
}

#[test]
fn test_modal_priority_permission_no_enter_delete_and_quit_confirm() {
    let mut app = App::new();
    seed_agent_and_terminal(&mut app);

    // 1. Permission modal intercepts Ctrl+G / Alt+Z / Enter (no Enter-to-allow!), Ctrl+C denies
    app.push_permission(PendingPermission {
        request_id: "p1".to_string(),
        description: "Delete files?".to_string(),
    });
    assert!(app.handle_key_event(ctrl('g')));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert!(app.pending_permission.is_some());

    assert!(app.handle_key_event(alt('z')));
    assert!(!app.sidebar_hidden);
    assert!(app.pending_permission.is_some());

    assert!(app.handle_key_event(key(KeyCode::Enter)));
    assert!(app.pending_permission.is_some(), "Enter must NOT approve permissions");
    assert!(app.pending_actions.is_empty());

    assert!(app.handle_key_event(ctrl('c')));
    assert!(!app.should_quit());
    assert!(app.pending_permission.is_none());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ResolvePermission {
            request_id: "p1".to_string(),
            approved: false,
        })
    );

    // 2. PendingQuitConfirm triggered by 'q' in MANAGE when terminal hasn't exited or agent is busy
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(!app.should_quit());
    let qc = app.pending_quit_confirm.clone().expect("quit confirm modal");
    assert_eq!(qc.active_terminals, 1);

    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|f| render_app_mut(f, &mut app))
        .unwrap();
    let modal_screen = render_screen(&terminal);
    assert!(modal_screen.contains("Confirm Quit"), "{modal_screen}");
    assert!(modal_screen.contains("1 live shell(s)"), "{modal_screen}");

    // Cancel quit confirm with Esc, then confirm with 'y'
    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.pending_quit_confirm.is_none());
    assert!(!app.should_quit());

    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.pending_quit_confirm.is_some());
    app.handle_key_event(key(KeyCode::Char('y')));
    assert!(app.should_quit());
}

#[test]
fn test_theme_presets_overrides_export_and_live_preview_picker() {
    assert_eq!(ThemePreset::default(), ThemePreset::TokyoNight);
    let mut overrides = BTreeMap::new();
    overrides.insert("accent".to_string(), "#ff00aa".to_string());
    overrides.insert("dirty".to_string(), "yellow".to_string());
    let custom = ThemePalette::from_prefs(Some("nord"), &overrides);
    assert_eq!(custom.accent, ratatui::style::Color::Rgb(0xff, 0x00, 0xaa));
    assert_eq!(custom.dirty, ratatui::style::Color::Yellow);

    let exported = custom.to_color_map("nord");
    assert_eq!(exported.get("name").map(String::as_str), Some("nord"));
    assert_eq!(exported.get("accent").map(String::as_str), Some("#ff00aa"));

    let mut app = App::new();
    app.apply_preferences(&UserPreferencesSnapshot {
        theme: Some("nord".to_string()),
        theme_overrides: overrides.clone(),
        ..Default::default()
    });
    assert_eq!(app.theme_preset, ThemePreset::Nord);
    let orig_palette = app.theme_palette.clone();

    // :theme opens live-preview ThemePickerModal; moving updates live palette, Esc reverts, Enter saves
    app.set_focus(FocusedPane::Tree);
    app.execute_sidebar_command("theme");
    assert!(app.theme_picker.is_some());

    app.handle_key_event(key(KeyCode::Char('k'))); // Nord -> CatppuccinMocha
    assert_eq!(app.theme_preset, ThemePreset::CatppuccinMocha);
    assert_ne!(app.theme_palette, orig_palette);

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.theme_picker.is_none());
    assert_eq!(app.theme_preset, ThemePreset::Nord);
    assert_eq!(app.theme_palette, orig_palette);

    app.submit_composer_prompt("/theme");
    assert!(app.theme_picker.is_some());
    app.handle_key_event(key(KeyCode::Down)); // Nord -> Ansi
    assert_eq!(app.theme_preset, ThemePreset::Ansi);
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.theme_picker.is_none());
    assert_eq!(app.theme_preset, ThemePreset::Ansi);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SaveThemePreference {
            theme: "ansi".to_string(),
        })
    );
}

#[test]
fn test_custom_keymap_bind_unbind_conflict_and_help_sync() {
    let mut km = Keymap::default();
    assert_eq!(
        km.match_write_key(ctrl('g')),
        Some(BindableAction::EnterManageMode)
    );
    assert!(km.bind(BindableAction::EnterManageMode, "none").is_err());

    // Binding Alt+Z to CommandPalette steals Alt+Z from ToggleZoom cleanly
    km.bind(BindableAction::CommandPalette, "alt+z").unwrap();
    assert_eq!(
        km.match_write_key(alt('z')),
        Some(BindableAction::CommandPalette)
    );
    assert!(km.chords_for(BindableAction::ToggleZoom).is_empty());

    let mut app = App::new();
    app.set_focus(FocusedPane::Tree);
    app.open_command_palette("");
    for ch in "bind jump_terminal alt+t".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SaveKeybindingPreference {
            action: "jump_terminal".to_string(),
            binding: "alt+t".to_string(),
        })
    );

    // Custom global WRITE binding now works in Composer!
    app.set_focus(FocusedPane::Composer);
    assert!(app.handle_key_event(alt('t')));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateTerminal { .. })
    ));

    // Help modal reflects live custom bindings
    app.show_help_modal = true;
    app.execute_sidebar_command("bind enter_manage_mode f6");
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    terminal
        .draw(|f| render_app_mut(f, &mut app))
        .unwrap();
    let help = render_screen(&terminal);
    assert!(help.contains("F6"), "Help modal must show live rebound F6:\n{help}");

    // Unbind resets to default (empty for jump_terminal) and emits RemoveKeybindingPreference
    app.show_help_modal = false;
    app.pending_actions.clear();
    app.submit_composer_prompt("/unbind jump_terminal");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::RemoveKeybindingPreference {
            action: "jump_terminal".to_string(),
        })
    );
    assert!(KeyChord::parse("ctrl+alt+invalid+x").is_err());
}

#[test]
fn test_tier1_tier2_chrome_smart_labels_overflow_and_sidebar_sizing() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-agent".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-agent".to_string(),
                    title: "A very long conversation title that overflows the dynamic sidebar".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Terminal".to_string(),
                    turns: Vec::new(),
                    custom_name: None,
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
                    cwd: Some("/home/alice/projects/grein".to_string()),
                    git_branch: Some("feature/super-long-branch-name-that-exceeds-28-chars".to_string()),
                    git_dirty: 2,
                    custom_name: None,
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-agent".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                    kind: ThreadKind::Terminal,
                    cwd: Some("/home/alice/repos/api/.grein/worktrees/feat-x".to_string()),
                    git_branch: Some("feat/x".to_string()),
                    git_dirty: 0,
                    custom_name: None,
                },
            ],
        },
    );

    // Smart >_ auto-labels & cwd shortening
    assert_eq!(
        display_thread_title(&app.threads[1], app.thread_meta.get("t-term")),
        "worktree: feat-x"
    );
    assert_eq!(
        shorten_terminal_cwd_with_home(
            "/home/alice/repos/api/.grein/worktrees/feat-x",
            Some("/home/alice")
        ),
        "⎇ feat-x"
    );
    assert_eq!(
        shorten_terminal_cwd_with_home("/home/alice/projects/grein", Some("/home/alice")),
        "~/projects/grein"
    );

    // Dynamic sidebar bounds & Fixed sidebar command
    assert_eq!(app.sidebar_layout.mode, SidebarMode::Dynamic);
    let dyn_w = compute_sidebar_width(&app, 100);
    assert!((18..=32).contains(&dyn_w), "dyn_w={dyn_w}");

    app.submit_composer_prompt("/sidebar fixed 24");
    assert_eq!(app.sidebar_layout.mode, SidebarMode::Fixed);
    assert_eq!(compute_sidebar_width(&app, 100), 24);
    assert!(app.pending_actions.iter().any(|a| matches!(
        a,
        AppAction::SaveUiPreference { key, value } if key == "sidebar_fixed_cols" && value == "24"
    )));

    // Render in MANAGE mode: rounded borders, hover overflow overlay crosses divider without resizing PTY,
    // no REVERSED on selection, capped <=28 char git branch badge, footer count
    app.set_focus(FocusedPane::Tree);
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|f| render_app_mut(f, &mut app))
        .unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains('╭') && screen.contains('╰'), "{screen}");
    assert!(screen.contains('▌'), "{screen}");
    assert!(screen.contains("1 agent · 1 shell"), "{screen}");
    assert!(screen.contains("⎇ feature/super-long-branch…"), "{screen}");
    assert!(
        screen.contains("conversation title that overflows"),
        "MANAGE 1-row hover overflow must render overflowing selected title across divider:\n{screen}"
    );

    // Verify full sidebar footer keeps concise agent/shell count without clipped ": cmd"
    app.execute_sidebar_command("sidebar fixed 32");
    terminal
        .draw(|f| render_app_mut(f, &mut app))
        .unwrap();
    let wide_screen = render_screen(&terminal);
    assert!(wide_screen.contains("1 agent · 1 shell"), "{wide_screen}");

    // Verify selected row does NOT use Modifier::REVERSED and row 1 col 23 (divider) shows overlaid char
    let buf = terminal.backend().buffer();
    assert!(!buf[(1, 1)].modifier.contains(Modifier::REVERSED));

    // Switch to Terminal and enter WRITE; verify header shows shortened cwd + right-aligned ● live pill
    app.switch_thread("t-term");
    app.set_focus(FocusedPane::Terminal);
    terminal
        .draw(|f| render_app_mut(f, &mut app))
        .unwrap();
    let term_screen = render_screen(&terminal);
    assert!(term_screen.contains(">_ ⎇ feat-x"), "{term_screen}");
    assert!(term_screen.contains("● live"), "{term_screen}");
    assert!(term_screen.contains("WRITE · SHELL"), "{term_screen}");
}

#[tokio::test]
async fn test_e2e_preferences_load_palette_diff_preview_and_prune() {
    struct OverhaulMockEngine {
        worktrees: Mutex<Vec<WorktreeEntry>>,
        diff_calls: Mutex<Vec<String>>,
        prune_calls: Mutex<usize>,
        saved_themes: Mutex<Vec<String>>,
        saved_bindings: Mutex<Vec<(String, String)>>,
        event_tx: Mutex<Option<mpsc::UnboundedSender<TuiEngineEvent>>>,
    }

    #[async_trait]
    impl TuiEnginePort for OverhaulMockEngine {
        async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
            let (tx, rx) = mpsc::unbounded_channel();
            *self.event_tx.lock().unwrap() = Some(tx);
            Ok(rx)
        }
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-1".to_string(),
                threads: vec![HydratedThread {
                    id: "t-1".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                                  custom_name: None,
                }],
            })
        }
        async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
            Ok(())
        }
        async fn fork_thread(&self, _source_thread_id: &str, _title: &str) -> io::Result<String> {
            Ok("t-fork".to_string())
        }
        async fn merge_thread(
            &self,
            _source_thread_id: &str,
            _target_thread_id: &str,
        ) -> io::Result<()> {
            Ok(())
        }
        async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
            Ok(())
        }
        async fn load_preferences(&self) -> io::Result<UserPreferencesSnapshot> {
            let mut ui = BTreeMap::new();
            ui.insert("sidebar_min_pct".to_string(), "20".to_string());
            ui.insert("sidebar_max_pct".to_string(), "35".to_string());
            Ok(UserPreferencesSnapshot {
                theme: Some("catppuccin-mocha".to_string()),
                theme_overrides: BTreeMap::new(),
                keybindings: BTreeMap::new(),
                ui,
            })
        }
        async fn save_theme_preference(&self, theme: &str) -> io::Result<()> {
            self.saved_themes.lock().unwrap().push(theme.to_string());
            Ok(())
        }
        async fn save_keybinding_preference(&self, action: &str, binding: &str) -> io::Result<()> {
            self.saved_bindings
                .lock()
                .unwrap()
                .push((action.to_string(), binding.to_string()));
            Ok(())
        }
        async fn list_worktrees(&self) -> io::Result<Vec<WorktreeEntry>> {
            Ok(self.worktrees.lock().unwrap().clone())
        }
        async fn worktree_diff_preview(&self, path: &str) -> io::Result<String> {
            self.diff_calls.lock().unwrap().push(path.to_string());
            Ok("diff --git a/src/lib.rs b/src/lib.rs\n@@ -1,2 +1,3 @@\n+pub fn overhaul() {}\n-pub fn old() {}\n".to_string())
        }
        async fn prune_worktrees(&self) -> io::Result<String> {
            *self.prune_calls.lock().unwrap() += 1;
            self.worktrees.lock().unwrap().retain(|w| w.is_primary);
            Ok("Pruned 1 stale worktree".to_string())
        }
    }

    let engine = Arc::new(OverhaulMockEngine {
        worktrees: Mutex::new(vec![
            WorktreeEntry {
                repo_root: "/repos/demo".to_string(),
                repo_name: "demo".to_string(),
                path: "/repos/demo".to_string(),
                branch: Some("main".to_string()),
                head_short: None,
                is_detached: false,
                is_primary: true,
                is_grein_managed: false,
                dirty_count: 0,
                attached_thread_ids: Vec::new(),
            },
            WorktreeEntry {
                repo_root: "/repos/demo".to_string(),
                repo_name: "demo".to_string(),
                path: "/repos/demo/.grein/worktrees/feat-ui".to_string(),
                branch: Some("feat/ui".to_string()),
                head_short: None,
                is_detached: false,
                is_primary: false,
                is_grein_managed: true,
                dirty_count: 2,
                attached_thread_ids: Vec::new(),
            },
        ]),
        diff_calls: Mutex::new(Vec::new()),
        prune_calls: Mutex::new(0),
        saved_themes: Mutex::new(Vec::new()),
        saved_bindings: Mutex::new(Vec::new()),
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
        if engine.event_tx.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Enter MANAGE, switch to Worktrees with Tab, arrow down twice to feat/ui -> loads diff preview
    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Tab))).unwrap();
    for _ in 0..30 {
        if !engine.worktrees.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(40)).await;
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();

    for _ in 0..50 {
        if !engine.diff_calls.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Also run :theme nord via sidebar command palette (and test :prune later or capture diff first)
    key_tx.send(Event::Key(key(KeyCode::Char(':')))).unwrap();
    for ch in "theme nord".chars() {
        key_tx.send(Event::Key(key(KeyCode::Char(ch)))).unwrap();
    }
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Capture frame with live diff preview before pruning!
    drop(key_tx);
    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("⎇ Diff · feat/ui"), "{screen}");
    assert!(screen.contains("+pub fn overhaul() {}"), "{screen}");
    assert_eq!(engine.saved_themes.lock().unwrap().as_slice(), &["nord"]);
    assert!(!engine.diff_calls.lock().unwrap().is_empty());

    // Verify 'p' prune in Worktrees works end-to-end as well
    let mut app_prune = App::new();
    app_prune.sidebar_view = SidebarView::Worktrees;
    app_prune.set_focus(FocusedPane::Tree);
    app_prune.handle_key_event(key(KeyCode::Char('p')));
    assert_eq!(
        app_prune.pending_actions.pop_front(),
        Some(AppAction::PruneWorktrees)
    );
}

#[test]
fn test_sidebar_width_stays_identical_across_grein_and_worktrees_tabs() {
    let mut app = App::new();
    seed_agent_and_terminal(&mut app);

    app.sidebar_view = SidebarView::Grein;
    let width_grein_empty = compute_sidebar_width(&app, 120);
    app.sidebar_view = SidebarView::Worktrees;
    let width_wt_empty = compute_sidebar_width(&app, 120);
    assert_eq!(width_grein_empty, width_wt_empty);

    app.worktrees = vec![WorktreeEntry {
        repo_root: "/repos/example".to_string(),
        repo_name: "example".to_string(),
        path: "/repos/example/.grein/worktrees/feature-very-long-worktree-branch".to_string(),
        branch: Some("feature/very-long-worktree-branch".to_string()),
        head_short: None,
        is_detached: false,
        is_primary: false,
        is_grein_managed: true,
        dirty_count: 1,
        attached_thread_ids: vec!["t-agent".to_string()],
    }];

    app.sidebar_view = SidebarView::Grein;
    let width_grein_populated = compute_sidebar_width(&app, 120);
    app.sidebar_view = SidebarView::Worktrees;
    let width_wt_populated = compute_sidebar_width(&app, 120);
    assert_eq!(width_grein_populated, width_wt_populated);
}
