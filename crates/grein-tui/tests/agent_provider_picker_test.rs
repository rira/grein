use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{App, AppAction, FocusedPane};
use grein_tui::engine_port::{
    AgentProviderInfo, HydratedSession, HydratedThread, PreferencesPort, ThreadMeta,
    TuiEngineEvent, TuiEnginePort, UserPreferencesSnapshot,
};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, draw_frame_mut, run_with_engine_events_and_services,
};
use grein_tui::TuiServices;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::SHIFT)
}

fn ctrl(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
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

fn sample_providers() -> Vec<AgentProviderInfo> {
    vec![
        AgentProviderInfo {
            id: "alpha".to_string(),
            plugin_id: "agent.alpha".to_string(),
            display_name: "Alpha Agent".to_string(),
            kind: "alpha".to_string(),
            available: true,
            executable: Some("/usr/bin/alpha".to_string()),
            is_default: true,
            install_hint: None,
        },
        AgentProviderInfo {
            id: "beta".to_string(),
            plugin_id: "agent.beta".to_string(),
            display_name: "Beta Code".to_string(),
            kind: "beta".to_string(),
            available: true,
            executable: Some("/usr/bin/beta".to_string()),
            is_default: false,
            install_hint: None,
        },
        AgentProviderInfo {
            id: "gamma".to_string(),
            plugin_id: "agent.gamma".to_string(),
            display_name: "Gamma CLI".to_string(),
            kind: "gamma".to_string(),
            available: false,
            executable: None,
            is_default: false,
            install_hint: Some("nix shell --impure nixpkgs#gamma-acp".to_string()),
        },
    ]
}

fn setup_tree_app() -> App {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-child".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-root".to_string(),
                    title: "Main".to_string(),
                    custom_name: None,
                    agent_provider: Some("alpha".to_string()),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-child".to_string(),
                    title: "Child Branch".to_string(),
                    custom_name: None,
                    agent_provider: Some("alpha".to_string()),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-grand".to_string(),
                    title: "Grandchild".to_string(),
                    custom_name: None,
                    agent_provider: None,
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-term".to_string(),
                    title: "Shell".to_string(),
                    custom_name: None,
                    agent_provider: None,
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
                    thread_id: "t-root".to_string(),
                    parent_thread_id: None,
                    kind: ThreadKind::Agent,
                    cwd: Some("/repos/root-dir".to_string()),
                    agent_provider: Some("alpha".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-child".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    kind: ThreadKind::Agent,
                    cwd: Some("/repos/child-wt".to_string()),
                    agent_provider: Some("alpha".to_string()),
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-grand".to_string(),
                    parent_thread_id: Some("t-child".to_string()),
                    kind: ThreadKind::Agent,
                    cwd: Some("/repos/grand-wt".to_string()),
                    agent_provider: None,
                    ..ThreadMeta::default()
                },
                ThreadMeta {
                    thread_id: "t-term".to_string(),
                    parent_thread_id: Some("t-child".to_string()),
                    kind: ThreadKind::Terminal,
                    cwd: Some("/repos/child-wt".to_string()),
                    agent_provider: None,
                    ..ThreadMeta::default()
                },
            ],
        },
    );
    app.set_agent_providers(sample_providers());
    app
}

#[test]
fn test_composer_agent_autocomplete_and_direct_sibling_spawn() {
    let mut app = setup_tree_app();
    app.set_focus(FocusedPane::Composer);

    // /a ranks /agent high, /ag uniquely completes /agent
    type_str(&mut app, "/a");
    let first_match = app
        .composer_completion
        .as_ref()
        .and_then(|ac| ac.active())
        .map(|s| s.full_text.as_str());
    assert_eq!(first_match, Some("/agent"));

    type_str(&mut app, "g");
    let ac = app.composer_completion.as_ref().expect("/ag completion");
    assert_eq!(ac.matches.len(), 1);
    assert_eq!(ac.active().unwrap().full_text, "/agent");
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.composer_input(), "/agent");

    // Bare /agent opens AgentPickerModal
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.agent_picker.is_some());
    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.agent_picker.is_none());

    // /agent <provider> [title] at depth >= 1 spawns sibling under same parent & inherited cwd
    app.pending_actions.clear();
    app.submit_composer_prompt("/agent BETA Fix auth flow");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateAgentThread {
            title: Some("Fix auth flow".to_string()),
            cwd: Some("/repos/child-wt".to_string()),
            parent_thread_id: Some("t-root".to_string()),
            agent_provider: Some("beta".to_string()),
        })
    );

    // /agent matching by display_name on depth-0 root yields parent_thread_id: None
    app.switch_thread("t-root");
    app.pending_actions.clear();
    app.submit_composer_prompt("/agent Beta Code Root sibling");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateAgentThread {
            title: Some("Root sibling".to_string()),
            cwd: Some("/repos/root-dir".to_string()),
            parent_thread_id: None,
            agent_provider: Some("beta".to_string()),
        })
    );

    // /agent on unavailable provider blocks spawn and shows install_hint
    app.submit_composer_prompt("/agent gamma Should fail");
    assert!(app.pending_actions.is_empty());
    let notice = app.status_notice.as_deref().unwrap_or("");
    assert!(
        notice.contains("nix shell --impure nixpkgs#gamma-acp"),
        "expected install_hint in notice: {notice}"
    );
}

#[test]
fn test_manage_a_key_palette_agent_and_dynamic_provider_commands() {
    let mut app = setup_tree_app();
    app.switch_thread("t-root");
    app.set_focus(FocusedPane::Tree);

    // Select t-child in tree and press 'A' -> opens AgentPickerModal targeting t-child
    let child_idx = app
        .tree
        .nodes
        .iter()
        .position(|n| matches!(n, grein_tui::ui::tree::TreeNode::Thread(t) if t.id == "t-child"))
        .unwrap();
    app.tree.select_index(child_idx);
    app.handle_key_event(shift('A'));
    let picker = app.agent_picker.as_ref().expect("A opens AgentPickerModal");
    assert_eq!(picker.target_thread_id, "t-child");
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.agent_picker.is_none());

    // :agent opens AgentPickerModal
    app.open_command_palette("agent");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.agent_picker.is_some());
    app.handle_key_event(ctrl('c'));
    assert!(app.agent_picker.is_none());

    // Dynamic palette suggestions include :<provider> and [not found]
    app.open_command_palette("");
    let all_suggs = app.palette_suggestions();
    assert!(all_suggs.iter().any(|(c, _)| c == "agent"));
    assert!(all_suggs
        .iter()
        .any(|(c, d)| c == "beta" && d == "New Beta Code sibling thread"));
    assert!(all_suggs
        .iter()
        .any(|(c, d)| c == "gamma" && d == "New Gamma CLI sibling thread [not found]"));

    // Windowed scrolling in palette popup when suggestions.len() > 5
    let mut terminal = Terminal::new(TestBackend::new(110, 26)).unwrap();
    for _ in 0..8 {
        app.handle_key_event(key(KeyCode::Down));
    }
    assert!(app.palette_match_idx >= 5);
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let scrolled_palette = render_screen(&terminal);
    let expected_cmd = format!(":{}", all_suggs[app.palette_match_idx].0);
    assert!(
        scrolled_palette.contains(&expected_cmd),
        "Selected palette row {expected_cmd} must scroll into view:\n{scrolled_palette}"
    );

    // Tab completion on dynamic provider suggestion
    app.handle_key_event(key(KeyCode::Esc));
    app.open_command_palette("be");
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.palette_input, "beta");

    // :<provider> [title] at depth >= 1 spawns sibling under t-root with child cwd
    app.palette_input = "beta Fix parser".to_string();
    app.palette_cursor = app.palette_input.len();
    app.pending_actions.clear();
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateAgentThread {
            title: Some("Fix parser".to_string()),
            cwd: Some("/repos/child-wt".to_string()),
            parent_thread_id: Some("t-root".to_string()),
            agent_provider: Some("beta".to_string()),
        })
    );

    // :<provider> on depth-0 root spawns sibling root (parent_thread_id: None)
    app.tree.select_index(0);
    app.pending_actions.clear();
    app.execute_sidebar_command("beta");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateAgentThread {
            title: None,
            cwd: Some("/repos/root-dir".to_string()),
            parent_thread_id: None,
            agent_provider: Some("beta".to_string()),
        })
    );

    // :<unavailable> sets status_notice with install_hint
    app.execute_sidebar_command("gamma broken");
    assert!(app.pending_actions.is_empty());
    assert!(app
        .status_notice
        .as_deref()
        .is_some_and(|n| n.contains("nix shell --impure nixpkgs#gamma-acp")));
}

#[test]
fn test_agent_picker_modal_navigation_enter_switch_default_and_render() {
    let mut app = setup_tree_app();
    app.open_agent_picker();
    assert_eq!(app.agent_picker.as_ref().unwrap().selected_idx, 0);

    // Navigation clamped at bounds
    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.agent_picker.as_ref().unwrap().selected_idx, 0);
    app.handle_key_event(key(KeyCode::Char('j')));
    assert_eq!(app.agent_picker.as_ref().unwrap().selected_idx, 1);
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.agent_picker.as_ref().unwrap().selected_idx, 2);
    app.handle_key_event(key(KeyCode::Char('j')));
    assert_eq!(app.agent_picker.as_ref().unwrap().selected_idx, 2);

    // Render unavailable row selected: shows install hint + selection bg across row + footer
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let unavail_screen = render_screen(&terminal);
    assert!(unavail_screen.contains("Select Agent"), "{unavail_screen}");
    assert!(
        unavail_screen.contains("Active: Alpha Agent · Default: Alpha Agent"),
        "{unavail_screen}"
    );
    assert!(unavail_screen.contains("(active, default)"), "{unavail_screen}");
    assert!(unavail_screen.contains("ready"), "{unavail_screen}");
    assert!(unavail_screen.contains("not found"), "{unavail_screen}");
    assert!(
        unavail_screen.contains("Install: nix shell --impure nixpkgs#gamma-acp"),
        "{unavail_screen}"
    );
    assert!(
        unavail_screen.contains("↑/↓/j/k Select · Enter New sibling · s Switch · d Default · Esc/q Close"),
        "{unavail_screen}"
    );

    // Enter / s / d on unavailable row keep modal open and show install_hint
    for k in [
        key(KeyCode::Enter),
        key(KeyCode::Char('s')),
        key(KeyCode::Char('d')),
    ] {
        app.handle_key_event(k);
        assert!(app.agent_picker.is_some());
        assert!(app.pending_actions.is_empty());
        assert!(app
            .status_notice
            .as_deref()
            .is_some_and(|n| n.contains("nix shell --impure nixpkgs#gamma-acp")));
    }

    // Move to Beta (idx 1) and press 'd' -> updates is_default & emits SaveDefaultAgentProvider
    app.handle_key_event(key(KeyCode::Char('k')));
    assert_eq!(app.agent_picker.as_ref().unwrap().selected_idx, 1);
    app.handle_key_event(key(KeyCode::Char('d')));
    assert!(!app.agent_providers[0].is_default);
    assert!(app.agent_providers[1].is_default);
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SaveDefaultAgentProvider {
            provider: "beta".to_string(),
        })
    );
    assert!(app
        .status_notice
        .as_deref()
        .is_some_and(|n| n.contains("Beta Code")));

    // Check full selection_bg span coverage on selected row
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let palette = app.theme_palette.clone();
    let buf = terminal.backend().buffer();
    let mut found_full_bg = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(byte_idx) = row.find("▌ ● Beta Code (default)") {
            let start_col = row[..byte_idx].chars().count() as u16;
            let ready_byte = row.find("ready ").expect("ready label on selected row");
            let end_col = row[..ready_byte].chars().count() as u16 + 5;
            assert!((start_col..=end_col).all(|x| buf[(x, y)].bg == palette.selection_bg));
            found_full_bg = true;
            break;
        }
    }
    assert!(found_full_bg, "selected row must fill selection_bg across row");

    // Press 's' -> closes modal and emits SwitchThreadProvider
    app.handle_key_event(key(KeyCode::Char('s')));
    assert!(app.agent_picker.is_none());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SwitchThreadProvider {
            thread_id: "t-child".to_string(),
            provider: "beta".to_string(),
        })
    );

    // 's' on a terminal thread refuses and sets status_notice
    app.switch_thread("t-term");
    app.pending_actions.clear();
    app.open_agent_picker();
    app.handle_key_event(key(KeyCode::Home));
    app.handle_key_event(key(KeyCode::Char('s')));
    assert!(app.pending_actions.is_empty());
    assert!(app
        .status_notice
        .as_deref()
        .is_some_and(|n| n.contains("shell")));

    // Enter on Beta while targeting t-child spawns sibling under t-root
    app.switch_thread("t-child");
    app.open_agent_picker();
    app.handle_key_event(key(KeyCode::End));
    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.agent_picker.as_ref().unwrap().selected_idx, 1);
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.agent_picker.is_none());
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateAgentThread {
            title: None,
            cwd: Some("/repos/child-wt".to_string()),
            parent_thread_id: Some("t-root".to_string()),
            agent_provider: Some("beta".to_string()),
        })
    );
}

#[test]
fn test_sidebar_chrome_provider_badges_and_help_modal_budget() {
    let mut app = setup_tree_app();
    apply_engine_event(
        &mut app,
        TuiEngineEvent::AgentMetaUpdated {
            thread_id: "t-child".to_string(),
            model: Some("claude-sonnet".to_string()),
            mode: Some("code".to_string()),
        },
    );
    app.collapsed_threads.insert("t-root".to_string());
    grein_tui::runner::rebuild_tree_nodes(&mut app);

    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let folded_screen = render_screen(&terminal);
    // Existing prefixes preserved even when provider badge is appended
    assert!(
        folded_screen.contains("1 ▸ ᛉ Main (+3) [alpha]"),
        "expected folded root prefix + provider badge:\n{folded_screen}"
    );

    // Unfold to verify child badge + status bar badge while keeping WRITE · AGENT and code · claude-sonnet
    app.collapsed_threads.clear();
    grein_tui::runner::rebuild_tree_nodes(&mut app);
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("1 ᛉ Main [alpha]"), "{screen}");
    assert!(screen.contains("└─ Child Branch [alpha]"), "{screen}");
    assert!(screen.contains("WRITE · AGENT"), "{screen}");
    assert!(screen.contains("alpha · code · claude-sonnet"), "{screen}");

    // Thread without agent_provider leaves prefix & status clean
    app.switch_thread("t-grand");
    apply_engine_event(
        &mut app,
        TuiEngineEvent::AgentMetaUpdated {
            thread_id: "t-grand".to_string(),
            model: Some("claude-sonnet".to_string()),
            mode: Some("code".to_string()),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let grand_screen = render_screen(&terminal);
    assert!(grand_screen.contains("├─ Grandchild"), "{grand_screen}");
    assert!(!grand_screen.contains("Grandchild ["), "{grand_screen}");
    assert!(grand_screen.contains("code · claude-sonnet"), "{grand_screen}");
    assert!(!grand_screen.contains("alpha · code · claude-sonnet"), "{grand_screen}");

    // Help modal at 110x30 fits without scroll and preserves tested labels
    app.show_help_modal = true;
    let mut help_term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut help_term, &mut app).unwrap();
    assert_eq!(app.help_max_scroll, 0, "help modal must fit 110x30 without scroll");
    let help_screen = render_screen(&help_term);
    for expected in [
        "Alt+H / Alt+L",
        "Toggle sidebar zoom",
        "/model",
        "/agent",
        "n / f / r / d",
        "A, :, /, ?, z, q",
        "Esc, q, ?, or Enter to close",
    ] {
        assert!(
            help_screen.contains(expected),
            "help modal missing '{expected}':\n{help_screen}"
        );
    }
}

#[tokio::test]
async fn test_runner_hydrates_providers_spawns_sibling_switches_and_persists_default_via_prefs() {
    struct TestPrefsPort {
        saved_ui: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl PreferencesPort for TestPrefsPort {
        async fn load_preferences(&self) -> io::Result<UserPreferencesSnapshot> {
            Ok(UserPreferencesSnapshot::default())
        }
        async fn save_theme_preference(&self, _theme: &str) -> io::Result<()> {
            Ok(())
        }
        async fn save_keybinding_preference(&self, _action: &str, _binding: &str) -> io::Result<()> {
            Ok(())
        }
        async fn remove_keybinding_preference(&self, _action: &str) -> io::Result<()> {
            Ok(())
        }
        async fn save_ui_pref(&self, key: &str, value: &str) -> io::Result<()> {
            self.saved_ui
                .lock()
                .unwrap()
                .push((key.to_string(), value.to_string()));
            Ok(())
        }
    }

    type CreateThreadCall = (Option<String>, Option<String>, Option<String>, Option<String>);

    struct ProviderEngine {
        created: Mutex<Vec<CreateThreadCall>>,
        switched: Mutex<Vec<(String, String)>>,
        list_calls: Mutex<usize>,
        engine_ui_saves: Mutex<Vec<(String, String)>>,
        meta: Mutex<Vec<ThreadMeta>>,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for ProviderEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-child".to_string(),
                threads: vec![
                    HydratedThread {
                        id: "t-root".to_string(),
                        title: "Main".to_string(),
                        custom_name: None,
                        agent_provider: Some("alpha".to_string()),
                        turns: Vec::new(),
                    },
                    HydratedThread {
                        id: "t-child".to_string(),
                        title: "Work".to_string(),
                        custom_name: None,
                        agent_provider: Some("alpha".to_string()),
                        turns: Vec::new(),
                    },
                ],
            })
        }
        async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
            Ok(())
        }
        async fn fork_thread(&self, _thread_id: &str, _anchor_id: &str) -> io::Result<String> {
            Ok("t-fork".to_string())
        }
        async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
            Ok(())
        }
        async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
            Ok(())
        }
        async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
            let (_tx, rx) = mpsc::unbounded_channel();
            Ok(rx)
        }
        async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
            Ok(self.meta.lock().unwrap().clone())
        }
        async fn save_ui_preference(&self, key: &str, value: &str) -> io::Result<()> {
            self.engine_ui_saves
                .lock()
                .unwrap()
                .push((key.to_string(), value.to_string()));
            Ok(())
        }
        async fn list_agent_providers(&self) -> io::Result<Vec<AgentProviderInfo>> {
            *self.list_calls.lock().unwrap() += 1;
            Ok(sample_providers())
        }
        async fn create_agent_thread(
            &self,
            title: Option<&str>,
            cwd: Option<&str>,
            parent_thread_id: Option<&str>,
            agent_provider: Option<&str>,
        ) -> io::Result<HydratedThread> {
            self.created.lock().unwrap().push((
                title.map(str::to_string),
                cwd.map(str::to_string),
                parent_thread_id.map(str::to_string),
                agent_provider.map(str::to_string),
            ));
            let new_meta = ThreadMeta {
                thread_id: "t-sibling".to_string(),
                parent_thread_id: parent_thread_id.map(str::to_string),
                kind: ThreadKind::Agent,
                cwd: cwd.map(str::to_string),
                agent_provider: agent_provider.map(str::to_string),
                ..ThreadMeta::default()
            };
            self.meta.lock().unwrap().push(new_meta);
            Ok(HydratedThread {
                id: "t-sibling".to_string(),
                title: title.unwrap_or("Beta Sibling").to_string(),
                custom_name: None,
                agent_provider: agent_provider.map(str::to_string),
                turns: Vec::new(),
            })
        }
        async fn switch_thread_provider(&self, thread_id: &str, provider: &str) -> io::Result<String> {
            self.switched
                .lock()
                .unwrap()
                .push((thread_id.to_string(), provider.to_string()));
            let mut g = self.meta.lock().unwrap();
            if let Some(m) = g.iter_mut().find(|m| m.thread_id == thread_id) {
                m.agent_provider = Some(provider.to_string());
            }
            Ok(provider.to_string())
        }
    }

    let prefs = Arc::new(TestPrefsPort {
        saved_ui: Mutex::new(Vec::new()),
    });
    let engine = Arc::new(ProviderEngine {
        created: Mutex::new(Vec::new()),
        switched: Mutex::new(Vec::new()),
        list_calls: Mutex::new(0),
        engine_ui_saves: Mutex::new(Vec::new()),
        meta: Mutex::new(vec![
            ThreadMeta {
                thread_id: "t-root".to_string(),
                parent_thread_id: None,
                kind: ThreadKind::Agent,
                cwd: Some("/repos/root".to_string()),
                agent_provider: Some("alpha".to_string()),
                ..ThreadMeta::default()
            },
            ThreadMeta {
                thread_id: "t-child".to_string(),
                parent_thread_id: Some("t-root".to_string()),
                kind: ThreadKind::Agent,
                cwd: Some("/repos/child-wt".to_string()),
                agent_provider: Some("alpha".to_string()),
                ..ThreadMeta::default()
            },
        ]),
    });

    let services = TuiServices::default().with_preferences(prefs.clone());
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();

    let runner = tokio::spawn(async move {
        run_with_engine_events_and_services(&mut terminal, App::new(), engine_dyn, services, key_rx)
            .await?;
        Ok::<_, io::Error>(terminal)
    });

    for _ in 0..80 {
        if *engine.list_calls.lock().unwrap() >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*engine.list_calls.lock().unwrap(), 1);

    // 1. Open /agent in Composer, move to Beta, press 'd' (persist default via TuiServices::preferences + refresh), then Enter (spawn sibling)
    for ch in "/agent".chars() {
        key_tx.send(Event::Key(key(KeyCode::Char(ch)))).unwrap();
    }
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Down))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('d')))).unwrap();

    for _ in 0..80 {
        if !prefs.saved_ui.lock().unwrap().is_empty()
            && *engine.list_calls.lock().unwrap() >= 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    for _ in 0..80 {
        if !engine.created.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 2. Now on t-sibling in Composer (WRITE · AGENT), open /agent and switch provider via 's' to alpha
    for ch in "/agent".chars() {
        key_tx.send(Event::Key(key(KeyCode::Char(ch)))).unwrap();
    }
    key_tx.send(Event::Key(key(KeyCode::Enter))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Home))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('s')))).unwrap();

    for _ in 0..80 {
        if !engine.switched.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(40)).await;

    key_tx.send(Event::Key(ctrl('g'))).unwrap();
    key_tx.send(Event::Key(key(KeyCode::Char('q')))).unwrap();

    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let screen = render_screen(&terminal);
    assert!(
        screen.contains("Beta Sibling [alpha]"),
        "expected created sibling with switched [alpha] badge in sidebar:\n{screen}"
    );
    assert!(screen.contains("alpha · agent · default"), "{screen}");
    assert_eq!(
        prefs.saved_ui.lock().unwrap().as_slice(),
        &[(
            "default_agent_provider".to_string(),
            "beta".to_string()
        )]
    );
    assert!(
        engine.engine_ui_saves.lock().unwrap().is_empty(),
        "default_agent_provider must persist solely via TuiServices::preferences"
    );
    assert_eq!(
        engine.created.lock().unwrap().as_slice(),
        &[(
            None,
            Some("/repos/child-wt".to_string()),
            Some("t-root".to_string()),
            Some("beta".to_string()),
        )]
    );
    assert_eq!(
        engine.switched.lock().unwrap().as_slice(),
        &[("t-sibling".to_string(), "alpha".to_string())]
    );
}
