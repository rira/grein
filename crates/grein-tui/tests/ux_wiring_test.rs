use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, AppAction, FocusedPane, PendingPermission};
use grein_tui::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentOptionsSnapshot, AgentSlashCommand,
    HydratedSession, HydratedThread, HydratedTurn, ThreadMeta, TuiEngineEvent, TuiEnginePort,
};
use grein_tui::layout::{
    compute_layout, compute_layout_with_chrome, compute_layout_with_chrome_and_sidebar,
};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, draw_frame_mut, run_with_engine_events,
    TerminalGuard,
};
use ratatui::backend::{Backend, TestBackend};
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

fn render_screen(terminal: &Terminal<TestBackend>) -> String {
    render_rect(terminal, terminal.backend().buffer().area)
}

fn render_rect(terminal: &Terminal<TestBackend>, area: Rect) -> String {
    let buf = terminal.backend().buffer();
    let max_y = (area.y + area.height).min(buf.area.height);
    let max_x = (area.x + area.width).min(buf.area.width);
    let mut out = String::new();
    for y in area.y..max_y {
        for x in area.x..max_x {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn send_text(tx: &mpsc::UnboundedSender<Event>, text: &str) {
    for ch in text.chars() {
        tx.send(Event::Key(key(KeyCode::Char(ch)))).unwrap();
    }
}

fn send_key(tx: &mpsc::UnboundedSender<Event>, code: KeyCode, modifiers: KeyModifiers) {
    tx.send(Event::Key(KeyEvent::new(code, modifiers)))
        .unwrap();
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

fn sample_config_options() -> Vec<AgentConfigOption> {
    vec![
        AgentConfigOption {
            id: "model".to_string(),
            name: "Model".to_string(),
            category: Some("model".to_string()),
            current_value: "gemini-2.5-pro".to_string(),
            options: vec![
                AgentConfigChoice {
                    value: "gemini-2.5-pro".to_string(),
                    name: "Gemini 2.5 Pro".to_string(),
                    description: Some("Deep reasoning".to_string()),
                },
                AgentConfigChoice {
                    value: "claude-sonnet-4".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    description: Some("Balanced coding".to_string()),
                },
                AgentConfigChoice {
                    value: "gemini-2.5-flash".to_string(),
                    name: "Gemini 2.5 Flash".to_string(),
                    description: Some("Fast responses".to_string()),
                },
            ],
        },
        AgentConfigOption {
            id: "mode".to_string(),
            name: "Mode".to_string(),
            category: Some("mode".to_string()),
            current_value: "code".to_string(),
            options: vec![
                AgentConfigChoice {
                    value: "code".to_string(),
                    name: "Code".to_string(),
                    description: Some("Edit and execute".to_string()),
                },
                AgentConfigChoice {
                    value: "plan".to_string(),
                    name: "Plan".to_string(),
                    description: Some("Read-only planning".to_string()),
                },
            ],
        },
        AgentConfigOption {
            id: "thought_level".to_string(),
            name: "Thinking Level".to_string(),
            category: Some("thought_level".to_string()),
            current_value: "medium".to_string(),
            options: vec![
                AgentConfigChoice {
                    value: "low".to_string(),
                    name: "Low".to_string(),
                    description: Some("Fast".to_string()),
                },
                AgentConfigChoice {
                    value: "medium".to_string(),
                    name: "Medium".to_string(),
                    description: None,
                },
                AgentConfigChoice {
                    value: "high".to_string(),
                    name: "High".to_string(),
                    description: Some("Deep".to_string()),
                },
            ],
        },
    ]
}

fn sample_commands() -> Vec<AgentSlashCommand> {
    vec![
        AgentSlashCommand {
            name: "review".to_string(),
            description: "Review staged diff".to_string(),
        },
        AgentSlashCommand {
            name: "/compact".to_string(),
            description: "Summarize older turns".to_string(),
        },
    ]
}

#[derive(Default)]
struct ConfigMockState {
    options_by_thread: HashMap<String, Vec<AgentConfigOption>>,
    commands: Vec<AgentSlashCommand>,
    set_calls: Vec<(String, String, String, bool)>,
    agent_options_calls: usize,
    event_tx: Option<mpsc::UnboundedSender<TuiEngineEvent>>,
}

struct ConfigMockEngine {
    session: HydratedSession,
    state: Mutex<ConfigMockState>,
}

impl ConfigMockEngine {
    fn new(session: HydratedSession) -> Arc<Self> {
        let mut options_by_thread = HashMap::new();
        for t in &session.threads {
            options_by_thread.insert(t.id.clone(), sample_config_options());
        }
        Arc::new(Self {
            session,
            state: Mutex::new(ConfigMockState {
                options_by_thread,
                commands: sample_commands(),
                ..Default::default()
            }),
        })
    }
}

#[async_trait::async_trait]
impl TuiEnginePort for ConfigMockEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        Ok(self.session.clone())
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

    async fn agent_options(&self, thread_id: &str) -> io::Result<AgentOptionsSnapshot> {
        let mut guard = self.state.lock().unwrap();
        guard.agent_options_calls += 1;
        let opts = guard
            .options_by_thread
            .get(thread_id)
            .cloned()
            .unwrap_or_else(sample_config_options);
        Ok(AgentOptionsSnapshot {
            thread_id: thread_id.to_string(),
            config_options: opts,
            available_commands: guard.commands.clone(),
        })
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> io::Result<AgentOptionsSnapshot> {
        let mut guard = self.state.lock().unwrap();
        guard.set_calls.push((
            thread_id.to_string(),
            config_id.to_string(),
            value.to_string(),
            set_default,
        ));
        let opts = guard
            .options_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(sample_config_options);
        if let Some(opt) = opts.iter_mut().find(|o| o.id == config_id) {
            opt.current_value = value.to_string();
        }
        let updated = opts.clone();
        let cmds = guard.commands.clone();
        if let Some(tx) = guard.event_tx.as_ref() {
            let _ = tx.send(TuiEngineEvent::AgentConfigUpdated {
                thread_id: thread_id.to_string(),
                config_options: updated.clone(),
            });
        }
        Ok(AgentOptionsSnapshot {
            thread_id: thread_id.to_string(),
            config_options: updated,
            available_commands: cmds,
        })
    }
}

#[test]
fn test_structured_thread_blocks_wiring_ctrl_o_toggle_and_markdown_fallback() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-1".to_string(),
                    title: "Main".to_string(),
                    turns: vec![HydratedTurn {
                        id: "u1".to_string(),
                        role: "User".to_string(),
                        content: "inspect files".to_string(),
                    }],
                },
                HydratedThread {
                    id: "t-2".to_string(),
                    title: "Side".to_string(),
                    turns: Vec::new(),
                },
            ],
        },
    );

    // Concurrent tool calls with same name distinguished by tool_call_id, plus is_error & summary
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolInvoked {
            thread_id: "t-1".to_string(),
            tool_name: "read_file".to_string(),
            tool_call_id: Some("tc-1".to_string()),
            summary: Some("src/app.rs".to_string()),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolInvoked {
            thread_id: "t-1".to_string(),
            tool_name: "read_file".to_string(),
            tool_call_id: Some("tc-2".to_string()),
            summary: Some("missing.rs".to_string()),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolResult {
            thread_id: "t-1".to_string(),
            tool_name: "read_file".to_string(),
            output: "l1\nl2\nl3\nl4\nl5\nl6".to_string(),
            tool_call_id: Some("tc-1".to_string()),
            is_error: false,
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolResult {
            thread_id: "t-1".to_string(),
            tool_name: "read_file".to_string(),
            output: "ENOENT".to_string(),
            tool_call_id: Some("tc-2".to_string()),
            is_error: true,
        },
    );

    // ShellOutput with ANSI on t-1
    app.submit_composer_prompt("!ls --color");
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ShellOutput {
            thread_id: "t-1".to_string(),
            command: "ls --color".to_string(),
            stdout: "\x1b[32msrc\x1b[0m\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        },
    );

    // Render shows structured cards (collapsed >4 lines tool call shows [Ctrl+O])
    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let collapsed_screen = render_screen(&terminal);
    assert!(
        collapsed_screen.contains("▶ ✓ read_file (src/app.rs) · 6 lines  [Ctrl+O]"),
        "Expected structured collapsed tool card:\n{collapsed_screen}"
    );
    assert!(
        collapsed_screen.contains("▼ ✖ read_file (missing.rs)"),
        "Expected error tool card with summary:\n{collapsed_screen}"
    );
    assert!(
        collapsed_screen.contains("│ ENOENT"),
        "Expected short error tool output inside gutter:\n{collapsed_screen}"
    );
    assert!(
        !collapsed_screen.contains("│ l6"),
        "Collapsed 6-line tool output must be hidden initially:\n{collapsed_screen}"
    );

    // Global Ctrl+O expands all collapsible cards and sets concise status notice
    assert!(app.handle_key_event(ctrl('o')));
    assert!(app.status_notice.is_some());
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let expanded_screen = render_screen(&terminal);
    assert!(
        expanded_screen.contains("▼ ✓ read_file (src/app.rs)"),
        "Ctrl+O must expand collapsed tool card:\n{expanded_screen}"
    );
    assert!(
        expanded_screen.contains("│ l6"),
        "Expanded tool card body must be visible:\n{expanded_screen}"
    );

    // Ctrl+O again collapses multi-line cards
    assert!(app.handle_key_event(ctrl('o')));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let recollapsed_screen = render_screen(&terminal);
    assert!(
        recollapsed_screen.contains("▶ ✓ read_file (src/app.rs) · 6 lines  [Ctrl+O]"),
        "Second Ctrl+O must collapse multi-line tool card:\n{recollapsed_screen}"
    );

    // Ctrl+O on previewed branch in Tree targets previewed thread t-2
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolInvoked {
            thread_id: "t-2".to_string(),
            tool_name: "grep".to_string(),
            tool_call_id: Some("p-1".to_string()),
            summary: Some("preview".to_string()),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolResult {
            thread_id: "t-2".to_string(),
            tool_name: "grep".to_string(),
            output: "a\nb\nc\nd\ne".to_string(),
            tool_call_id: Some("p-1".to_string()),
            is_error: false,
        },
    );
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Down)); // select t-2 in Tree
    assert_eq!(app.active_thread_id(), "t-1");
    assert!(app.handle_key_event(ctrl('o')));
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let preview_expanded = render_screen(&terminal);
    assert!(
        preview_expanded.contains("▼ ✓ grep (preview)"),
        "Ctrl+O from Tree preview must expand previewed thread cards:\n{preview_expanded}"
    );

    // Fallback rule: directly mutating app.active_transcript_md must render mutated markdown
    app.set_focus(FocusedPane::Composer);
    app.active_transcript_md = "## Direct Test Override\nVisible fallback line".to_string();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let fallback_screen = render_screen(&terminal);
    assert!(
        fallback_screen.contains("Direct Test Override"),
        "Direct mutation to active_transcript_md must fall back to markdown renderer:\n{fallback_screen}"
    );
    assert!(fallback_screen.contains("Visible fallback line"));

    // ThreadCreated, ThreadMerged, Error, and load_state_from_sqlite populate thread_blocks cleanly
    let mut sync_app = App::new();
    apply_hydrated_session(
        &mut sync_app,
        &HydratedSession {
            active_thread_id: "r1".to_string(),
            threads: vec![HydratedThread {
                id: "r1".to_string(),
                title: "Root".to_string(),
                turns: Vec::new(),
            }],
        },
    );
    apply_engine_event(
        &mut sync_app,
        TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: "b1".to_string(),
                title: "Branch".to_string(),
                turns: vec![HydratedTurn {
                    id: "bu1".to_string(),
                    role: "User".to_string(),
                    content: "branch work".to_string(),
                }],
            },
        },
    );
    assert!(sync_app.thread_blocks.contains_key("b1"));
    apply_engine_event(
        &mut sync_app,
        TuiEngineEvent::ThreadMerged {
            thread_id: "b1".to_string(),
            target_thread_id: "r1".to_string(),
            summary: "verified auth".to_string(),
        },
    );
    apply_engine_event(
        &mut sync_app,
        TuiEngineEvent::Error {
            message: "stream dropped".to_string(),
        },
    );
    assert!(!sync_app.thread_blocks["r1"].is_empty());
    assert!(!sync_app.thread_blocks["b1"].is_empty());

    // SQLite roundtrip also populates thread_blocks
    let db_path = std::env::temp_dir().join(format!(
        "grein-blocks-sqlite-{}.sqlite3",
        grein_core::model::ThreadId::now_v7()
    ));
    let _ = std::fs::remove_file(&db_path);
    let mut saver = App::new();
    saver.submit_composer_prompt("sqlite prompt");
    saver.apply_agent_response("sqlite reply");
    saver.persist_state_to_sqlite(&db_path);
    let mut loader = App::new();
    loader.load_state_from_sqlite(&db_path);
    let loaded_id = loader.active_thread_id().to_string();
    assert!(
        loader
            .thread_blocks
            .get(&loaded_id)
            .is_some_and(|b| b.latest_assistant_text().as_deref() == Some("sqlite reply")),
        "load_state_from_sqlite must populate thread_blocks"
    );
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn test_exact_transcript_scroll_no_bottom_clipping_and_submit_auto_scroll() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![HydratedThread {
                id: "t-1".to_string(),
                title: "Main".to_string(),
                turns: Vec::new(),
            }],
        },
    );

    // Craft long lines with short words that force word-wrapping to produce MORE lines than naive div_ceil!
    let mut content = String::new();
    for i in 0..25 {
        content.push_str(&format!(
            "Turn {i:02}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda FINAL_TOKEN_{i:02}\n"
        ));
    }
    content.push_str("BOTTOM_SENTINEL_EXACT_LINE");
    apply_engine_event(
        &mut app,
        TuiEngineEvent::StreamChunk {
            thread_id: "t-1".to_string(),
            content,
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::TurnCompleted {
            thread_id: "t-1".to_string(),
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let layout = compute_layout_with_chrome(Rect::new(0, 0, 80, 18), 0);
    let transcript_screen = render_rect(&terminal, layout.transcript_area);
    assert!(
        transcript_screen.contains("BOTTOM_SENTINEL_EXACT_LINE"),
        "Last line must not be clipped at bottom of Transcript:\n{transcript_screen}"
    );

    // User scrolls up (disabling auto_scroll)
    app.handle_key_event(key(KeyCode::PageUp));
    assert!(!app.transcript_viewport("t-1").auto_scroll());
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let scrolled_up_screen = render_rect(&terminal, layout.transcript_area);
    assert!(!scrolled_up_screen.contains("BOTTOM_SENTINEL_EXACT_LINE"));

    // Normal prompt submit re-enables auto_scroll and jumps to bottom
    app.submit_composer_prompt("new prompt at bottom");
    assert!(
        app.transcript_viewport("t-1").auto_scroll(),
        "Normal submit must re-enable auto_scroll"
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let after_prompt_screen = render_rect(&terminal, layout.transcript_area);
    assert!(after_prompt_screen.contains("new prompt at bottom"));

    // Scroll up again, then !cmd shell submit re-enables auto_scroll
    app.handle_key_event(key(KeyCode::PageUp));
    assert!(!app.transcript_viewport("t-1").auto_scroll());
    app.submit_composer_prompt("!echo bottom_shell");
    assert!(
        app.transcript_viewport("t-1").auto_scroll(),
        "!cmd shell submit must re-enable auto_scroll"
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let after_shell_screen = render_rect(&terminal, layout.transcript_area);
    assert!(after_shell_screen.contains("echo bottom_shell"));
}

#[test]
fn test_composer_soft_wrap_unicode_auto_grow_and_cursor_tracking() {
    let mut app = App::new();
    // Single long line without '\n' on 60-col terminal (inner_width = 58)
    let long_single = "word ".repeat(28) + "TAIL_END";
    app.input_buffer = long_single.clone();
    app.cursor_position = app.input_buffer.len();

    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);
    assert!(
        screen.contains("TAIL_END"),
        "Wrapped single long line in Composer must not clip past right border:\n{screen}"
    );
    let cursor = terminal.backend_mut().get_cursor_position().unwrap();
    assert!(
        cursor.x >= 1 && cursor.x < 59,
        "Cursor x {} must be inside Composer inner width (1..59)",
        cursor.x
    );

    // Wide Unicode characters wrap cleanly without crossing right border
    app.input_buffer = "🦀".repeat(35); // 70 cells > 58 inner width
    app.cursor_position = app.input_buffer.len();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let unicode_cursor = terminal.backend_mut().get_cursor_position().unwrap();
    assert!(
        unicode_cursor.x >= 1 && unicode_cursor.x < 59,
        "Unicode wrapped cursor x {} must stay inside inner bounds",
        unicode_cursor.x
    );

    // Tall wrapped input (> 8 visual lines -> capped at height 10, inner_height = 8):
    // Cursor at start keeps top line visible; cursor at end keeps bottom line visible.
    app.input_buffer = (0..12)
        .map(|i| format!("ROW_{i:02}_CONTENT"))
        .collect::<Vec<_>>()
        .join("\n");
    app.cursor_position = 0; // top row!
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let top_screen = render_screen(&terminal);
    assert!(
        top_screen.contains("ROW_00_CONTENT"),
        "When cursor is at row 0, Composer scroll_y must show ROW_00_CONTENT:\n{top_screen}"
    );
    let top_cursor = terminal.backend_mut().get_cursor_position().unwrap();

    app.cursor_position = app.input_buffer.len(); // bottom row!
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let bottom_screen = render_screen(&terminal);
    assert!(
        bottom_screen.contains("ROW_11_CONTENT"),
        "When cursor is at bottom, Composer scroll_y must show ROW_11_CONTENT:\n{bottom_screen}"
    );
    let bottom_cursor = terminal.backend_mut().get_cursor_position().unwrap();
    assert!(
        bottom_cursor.y > top_cursor.y,
        "Bottom cursor y ({}) must be below top cursor y ({})",
        bottom_cursor.y,
        top_cursor.y
    );
}

#[test]
fn test_terminal_guard_non_tty_safety() {
    let guard = TerminalGuard::new();
    assert!(guard.is_ok());
    drop(guard);
}

#[tokio::test]
async fn test_config_picker_modal_arguments_commands_and_status_bar() {
    // 1. Unit-level modal priority, navigation, scope toggle, and cursor hiding
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![HydratedThread {
                id: "t-1".to_string(),
                title: "Main".to_string(),
                turns: Vec::new(),
            }],
        },
    );

    // Bare /model, /mode, /thinking, /effort emit OpenConfigPicker
    app.submit_composer_prompt("/model");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::OpenConfigPicker {
            thread_id: "t-1".to_string(),
            config_id: "model".to_string(),
        })
    );
    app.submit_composer_prompt("/mode");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::OpenConfigPicker {
            thread_id: "t-1".to_string(),
            config_id: "mode".to_string(),
        })
    );
    app.submit_composer_prompt("/thinking");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::OpenConfigPicker {
            thread_id: "t-1".to_string(),
            config_id: "thought_level".to_string(),
        })
    );
    app.submit_composer_prompt("/effort");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::OpenConfigPicker {
            thread_id: "t-1".to_string(),
            config_id: "thought_level".to_string(),
        })
    );

    // Cached AgentConfigUpdated populates status bar metadata & resolves /model sonnet / --default
    apply_engine_event(
        &mut app,
        TuiEngineEvent::AgentConfigUpdated {
            thread_id: "t-1".to_string(),
            config_options: sample_config_options(),
        },
    );
    assert_eq!(
        app.thread_agent_meta.get("t-1"),
        Some(&(
            Some("gemini-2.5-pro".to_string()),
            Some("code".to_string())
        ))
    );

    app.submit_composer_prompt("/model sonnet");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SetAgentConfig {
            thread_id: "t-1".to_string(),
            config_id: "model".to_string(),
            value: "claude-sonnet-4".to_string(),
            set_default: false,
        })
    );
    app.submit_composer_prompt("/mode --default plan");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SetAgentConfig {
            thread_id: "t-1".to_string(),
            config_id: "mode".to_string(),
            value: "plan".to_string(),
            set_default: true,
        })
    );
    app.submit_composer_prompt("/thinking default high");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::SetAgentConfig {
            thread_id: "t-1".to_string(),
            config_id: "thought_level".to_string(),
            value: "high".to_string(),
            set_default: true,
        })
    );

    // Cached /commands lists commands cleanly
    apply_engine_event(
        &mut app,
        TuiEngineEvent::AgentCommandsUpdated {
            thread_id: "t-1".to_string(),
            commands: sample_commands(),
        },
    );
    app.submit_composer_prompt("/commands");
    assert!(app
        .status_notice
        .as_deref()
        .is_some_and(|s| s.contains("/review") && s.contains("/compact")));

    // 2. Full async E2E with ConfigMockEngine
    let session = HydratedSession {
        active_thread_id: "t-main".to_string(),
        threads: vec![HydratedThread {
            id: "t-main".to_string(),
            title: "Main".to_string(),
            turns: Vec::new(),
        }],
    };
    let engine = ConfigMockEngine::new(session);
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let runner = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    wait_for("engine subscribed", || {
        engine.state.lock().unwrap().event_tx.is_some()
    })
    .await;

    // Bare /model opens ConfigPickerModal
    send_text(&key_tx, "/model");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("agent_options called for /model", || {
        engine.state.lock().unwrap().agent_options_calls >= 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Move down to Claude Sonnet 4, toggle scope with 'd' (Default for new branches), press Enter
    send_key(&key_tx, KeyCode::Char('j'), KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Char('d'), KeyModifiers::NONE);
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("set_agent_config called from picker", || {
        !engine.state.lock().unwrap().set_calls.is_empty()
    })
    .await;

    // Argument form with empty cache: /thinking high
    send_text(&key_tx, "/thinking high");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    wait_for("set_agent_config called for /thinking high", || {
        engine.state.lock().unwrap().set_calls.len() >= 2
    })
    .await;

    // /commands fetches and displays commands
    send_text(&key_tx, "/commands");
    send_key(&key_tx, KeyCode::Enter, KeyModifiers::NONE);
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_key(&key_tx, KeyCode::Char('q'), KeyModifiers::CONTROL);
    let terminal = timeout(Duration::from_secs(3), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let calls = engine.state.lock().unwrap().set_calls.clone();
    assert_eq!(
        calls[0],
        (
            "t-main".to_string(),
            "model".to_string(),
            "claude-sonnet-4".to_string(),
            true
        )
    );
    assert_eq!(
        calls[1],
        (
            "t-main".to_string(),
            "thought_level".to_string(),
            "high".to_string(),
            false
        )
    );

    let final_screen = render_screen(&terminal);
    assert!(
        final_screen.contains("claude-sonnet-4"),
        "Status bar must reflect updated model:\n{final_screen}"
    );
    assert!(
        final_screen.contains("/review") && final_screen.contains("Review staged diff"),
        "/commands output must render clearly:\n{final_screen}"
    );

    // 3. Direct rendering + key priority check for ConfigPickerModal (green active, reversed cyan selected)
    let mut modal_app = App::new();
    apply_engine_event(
        &mut modal_app,
        TuiEngineEvent::AgentConfigUpdated {
            thread_id: "trunk".to_string(),
            config_options: sample_config_options(),
        },
    );
    modal_app.open_config_picker_from_options("trunk", "model");
    assert!(modal_app.config_picker.is_some());

    // Priority: pending_permission captures keys before config_picker; config_picker captures before help/composer
    modal_app.push_permission(PendingPermission {
        request_id: "p1".to_string(),
        description: "Allow tool?".to_string(),
    });
    modal_app.handle_key_event(key(KeyCode::Char('n')));
    assert!(modal_app.pending_permission.is_none());
    assert!(modal_app.config_picker.is_some());

    // Move selection down to Claude Sonnet 4 while Gemini 2.5 Pro stays active
    modal_app.handle_key_event(key(KeyCode::Down));
    modal_app.handle_key_event(key(KeyCode::Tab)); // toggle scope to Default for new branches
    let mut modal_term = Terminal::new(TestBackend::new(100, 26)).unwrap();
    draw_frame_mut(&mut modal_term, &mut modal_app).unwrap();
    let modal_screen = render_screen(&modal_term);
    assert!(modal_screen.contains("Default for new branches"), "{modal_screen}");
    assert!(modal_screen.contains("Gemini 2.5 Pro"), "{modal_screen}");
    assert!(modal_screen.contains("Claude Sonnet 4"), "{modal_screen}");

    // Verify Green on active row ("Gemini 2.5 Pro") and Reversed Cyan on selected row ("Claude Sonnet 4")
    let buf = modal_term.backend().buffer();
    let mut found_green_active = false;
    let mut found_reversed_cyan = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(byte_idx) = row.find("Gemini 2.5 Pro") {
            let col = row[..byte_idx].chars().count() as u16;
            if buf[(col, y)].fg == Color::Green {
                found_green_active = true;
            }
        }
        if let Some(byte_idx) = row.find("Claude Sonnet 4") {
            let col = row[..byte_idx].chars().count() as u16;
            let cell = &buf[(col, y)];
            if cell.fg == Color::Cyan && cell.modifier.contains(Modifier::REVERSED) {
                found_reversed_cyan = true;
            }
        }
    }
    assert!(found_green_active, "Active choice must be highlighted in Green");
    assert!(
        found_reversed_cyan,
        "Selected row must be highlighted in Reversed Cyan"
    );

    // Esc closes modal cleanly
    modal_app.handle_key_event(key(KeyCode::Esc));
    assert!(modal_app.config_picker.is_none());
    assert!(!modal_app.should_quit());
}

#[test]
fn test_borderless_sides_zoom_layout_and_clipboard_copy_modes() {
    let area = Rect::new(0, 0, 100, 24);
    let legacy = compute_layout(area);
    let chrome = compute_layout_with_chrome(area, 0);
    assert_eq!(chrome.tree_area.width, legacy.tree_area.width);
    assert_eq!(chrome.tree_area.height + 1, legacy.tree_area.height);
    let unzoomed = compute_layout_with_chrome_and_sidebar(area, 0, false);
    assert_eq!(unzoomed, chrome);
    let zoomed = compute_layout_with_chrome_and_sidebar(area, 0, true);
    assert_eq!(zoomed.tree_area.width, 0);
    assert_eq!(zoomed.transcript_area.x, 0);
    assert_eq!(zoomed.transcript_area.width, 100);

    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![HydratedThread {
                id: "t-1".to_string(),
                title: "Main".to_string(),
                turns: vec![
                    HydratedTurn {
                        id: "u1".to_string(),
                        role: "User".to_string(),
                        content: "Show snippet".to_string(),
                    },
                    HydratedTurn {
                        id: "a1".to_string(),
                        role: "Assistant".to_string(),
                        content: "Explanation:\n```rust\nfn demo() {}\n```\nDone.".to_string(),
                    },
                ],
            }],
        },
    );

    // Toggle sidebar_hidden via Ctrl+T, /zoom, and Tree 'z'
    assert!(!app.sidebar_hidden);
    assert!(app.handle_key_event(ctrl('t')));
    assert!(app.sidebar_hidden);

    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let zoomed_transcript = render_rect(&terminal, zoomed.transcript_area);
    assert!(!render_screen(&terminal).contains("ᛇ Grein"));
    for (idx, row) in zoomed_transcript.lines().enumerate() {
        let first = row.chars().next().unwrap_or(' ');
        let last = row.chars().last().unwrap_or(' ');
        assert_ne!(
            first, '│',
            "Borderless zoomed Transcript row {idx} must not have left │ border: {row:?}"
        );
        assert_ne!(
            last, '│',
            "Borderless zoomed Transcript row {idx} must not have right │ border: {row:?}"
        );
    }
    assert!(zoomed_transcript.contains("✦ Explanation:"));
    assert!(!zoomed_transcript.contains("Assistant"));
    assert!(zoomed_transcript.contains("fn demo() {}"));

    app.submit_composer_prompt("/zoom");
    assert!(!app.sidebar_hidden);

    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('z')));
    assert!(app.sidebar_hidden);

    // Clipboard copy via 'y' in Tree, Alt+Y globally, /copy code, /copy all
    app.handle_key_event(key(KeyCode::Char('y')));
    assert_eq!(
        app.last_copied_text.as_deref(),
        Some("Explanation:\n```rust\nfn demo() {}\n```\nDone.")
    );
    assert!(app.status_notice.is_some());

    app.set_focus(FocusedPane::Composer);
    app.last_copied_text = None;
    app.handle_key_event(alt('y'));
    assert_eq!(
        app.last_copied_text.as_deref(),
        Some("Explanation:\n```rust\nfn demo() {}\n```\nDone.")
    );

    app.submit_composer_prompt("/copy code");
    assert_eq!(app.last_copied_text.as_deref(), Some("fn demo() {}"));

    app.submit_composer_prompt("/copy all");
    let copied_all = app.last_copied_text.as_deref().unwrap_or("");
    assert!(copied_all.contains("Show snippet"));
    assert!(copied_all.contains("✦ Explanation:"));
    assert!(!copied_all.contains("Assistant"));
    assert!(copied_all.contains("fn demo() {}"));
}

#[test]
fn test_os_safe_keymap_ctrl_b_tree_keys_and_scrollable_help_modal() {
    // 1. Ctrl+B backward-char and uppercase Ctrl+C quit
    let mut app = App::new();
    app.input_buffer = "héllo".to_string();
    app.cursor_position = app.input_buffer.len();
    assert!(app.handle_key_event(ctrl('b')));
    assert_eq!(&app.input_buffer[..app.cursor_position], "héll");
    assert!(app.handle_key_event(ctrl('b')));
    assert!(app.handle_key_event(ctrl('b')));
    assert!(app.handle_key_event(ctrl('b')));
    assert_eq!(&app.input_buffer[..app.cursor_position], "h");

    // Alt+1..9 remain unbound in all panes
    for pane in [
        FocusedPane::Composer,
        FocusedPane::Tree,
        FocusedPane::Transcript,
    ] {
        app.set_focus(pane);
        for d in '1'..='9' {
            assert!(!app.handle_key_event(alt(d)));
        }
    }

    let mut quit_upper = App::new();
    quit_upper.handle_key_event(ctrl('C'));
    assert!(quit_upper.should_quit());

    // 2. Zero-modifier Tree keys: J/K, Ctrl+U/D, g/G, f, z, y
    let mut tree_app = App::new();
    apply_hydrated_session(
        &mut tree_app,
        &HydratedSession {
            active_thread_id: "r1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "r1".to_string(),
                    title: "Root 1".to_string(),
                    turns: vec![HydratedTurn {
                        id: "a1".to_string(),
                        role: "Assistant".to_string(),
                        content: "root1 reply".to_string(),
                    }],
                },
                HydratedThread {
                    id: "b1".to_string(),
                    title: "Branch 1".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "r2".to_string(),
                    title: "Root 2".to_string(),
                    turns: Vec::new(),
                },
            ],
        },
    );
    apply_engine_event(
        &mut tree_app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "r1".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: grein_core::ThreadKind::Agent,
                    cwd: None,
                    git_branch: None,
                    git_dirty: 0,
                },
                ThreadMeta {
                    thread_id: "b1".to_string(),
                    parent_thread_id: Some("r1".to_string()),
                    inherited_turn_count: 1,
                    merged: false,
                    kind: grein_core::ThreadKind::Agent,
                    cwd: None,
                    git_branch: None,
                    git_dirty: 0,
                },
                ThreadMeta {
                    thread_id: "r2".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: grein_core::ThreadKind::Agent,
                    cwd: None,
                    git_branch: None,
                    git_dirty: 0,
                },
            ],
        },
    );
    tree_app.transcript_viewport_mut("r1").set_max_scroll(80);
    tree_app.transcript_viewport_mut("r1").scroll_to_bottom();
    tree_app.set_focus(FocusedPane::Tree);

    // Ctrl+U / Ctrl+D half-page scroll, g / G top / bottom
    assert!(tree_app.handle_key_event(ctrl('u')));
    assert!(tree_app.transcript_viewport("r1").scroll_offset() < 80);
    assert!(tree_app.handle_key_event(ctrl('d')));
    assert_eq!(tree_app.transcript_viewport("r1").scroll_offset(), 80);
    assert!(tree_app.handle_key_event(key(KeyCode::Char('g'))));
    assert_eq!(tree_app.transcript_viewport("r1").scroll_offset(), 0);
    assert!(tree_app.handle_key_event(key(KeyCode::Char('G'))));
    assert_eq!(tree_app.transcript_viewport("r1").scroll_offset(), 80);

    // 'f' forks selected thread, 'J'/'K' switch roots
    tree_app.handle_key_event(key(KeyCode::Down)); // select b1
    tree_app.handle_key_event(key(KeyCode::Char('f')));
    assert_eq!(
        tree_app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch {
            source_thread_id: "b1".to_string(),
            title: None,
        })
    );
    tree_app.handle_key_event(key(KeyCode::Char('J')));
    assert_eq!(tree_app.active_thread_id(), "r2");
    tree_app.handle_key_event(key(KeyCode::Char('K')));
    assert_eq!(tree_app.active_thread_id(), "r1");

    // 3. Complete scrollable Help modal on standard and tiny terminals
    tree_app.show_help_modal = true;
    let mut full_term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut full_term, &mut tree_app).unwrap();
    let full_help = render_screen(&full_term);
    for expected in [
        "Global",
        "ᛇ Grein Navigation",
        "Composer & Slash Commands",
        "Ctrl+J",
        "Ctrl+T",
        "Ctrl+O",
        "Ctrl+B",
        "Ctrl+F",
        "/model",
        "/mode",
        "/thinking",
        "/commands",
        "/copy",
        "/zoom",
        "/quit",
        "Press Esc, q, ?, or Enter to close",
    ] {
        assert!(
            full_help.contains(expected),
            "Help modal missing {expected:?}:\n{full_help}"
        );
    }

    // On tiny 80x12 terminal, footer remains visible and j/k scroll reveals bottom slash commands
    let mut tiny_term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    draw_frame_mut(&mut tiny_term, &mut tree_app).unwrap();
    let tiny_top = render_screen(&tiny_term);
    assert!(
        tiny_top.contains("Press Esc, q, ?, or Enter to close"),
        "Footer must remain pinned on tiny terminal:\n{tiny_top}"
    );
    for _ in 0..15 {
        tree_app.handle_key_event(key(KeyCode::Char('j')));
    }
    assert!(tree_app.help_scroll_offset > 0);
    draw_frame_mut(&mut tiny_term, &mut tree_app).unwrap();
    let tiny_scrolled = render_screen(&tiny_term);
    assert!(
        tiny_scrolled.contains("/model") || tiny_scrolled.contains("/copy"),
        "Scrolling down in help modal on tiny terminal must reveal slash commands:\n{tiny_scrolled}"
    );
    assert!(tiny_scrolled.contains("Press Esc, q, ?, or Enter to close"));
}
