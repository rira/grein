use std::io;
use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, AppAction, AppEvent, FocusedPane};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, HydratedTurn, SubagentSnapshot, SubagentStatus, ThreadMeta,
    TuiEngineEvent, TuiEnginePort,
};
use grein_tui::layout::{compute_layout, compute_layout_with_chrome, subagent_strip_height};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, draw_frame_mut, run_with_engine_events,
};
use grein_tui::ui::tree::TreeNode;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tokio::sync::mpsc;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
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

#[test]
fn test_tree_is_context_only_nested_and_scrollable() {
    let mut app = App::new();
    let session = HydratedSession {
        active_thread_id: "t-root".to_string(),
        threads: vec![
            HydratedThread {
                id: "t-root".to_string(),
                title: "Main".to_string(),
                turns: vec![
                    HydratedTurn {
                        id: "u1".to_string(),
                        role: "User".to_string(),
                        content: "How does auth work?".to_string(),
                    },
                    HydratedTurn {
                        id: "a1".to_string(),
                        role: "Assistant".to_string(),
                        content: "### Auth Pipeline\n1. **Token Check**: verifies JWT".to_string(),
                    },
                ],
            },
            HydratedThread {
                id: "t-child".to_string(),
                title: "Child Context".to_string(),
                turns: Vec::new(),
            },
            HydratedThread {
                id: "t-grandchild".to_string(),
                title: "Grandchild".to_string(),
                turns: Vec::new(),
            },
            HydratedThread {
                id: "t-sibling".to_string(),
                title: "Second Child".to_string(),
                turns: Vec::new(),
            },
        ],
        blackboard: Vec::new(),
    };
    apply_hydrated_session(&mut app, &session);
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "t-root".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "t-child".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    inherited_turn_count: 2,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "t-grandchild".to_string(),
                    parent_thread_id: Some("t-child".to_string()),
                    inherited_turn_count: 2,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "t-sibling".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    inherited_turn_count: 2,
                    merged: false,
                },
            ],
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadStatusChanged {
            thread_id: "t-child".to_string(),
            status: "streaming".to_string(),
        },
    );

    // Tree nodes must contain ONLY Thread context entries (zero Anchor nodes)
    assert_eq!(app.tree.nodes.len(), 4);
    assert!(app
        .tree
        .nodes
        .iter()
        .all(|n| matches!(n, TreeNode::Thread(_))));
    assert!(app.threads[0].anchors.iter().all(|a| {
        !a.label.contains("User:")
            && !a.label.contains("Agent (agy)")
            && !a.label.contains("Assistant")
    }));

    let mut terminal = Terminal::new(TestBackend::new(120, 28)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);

    // Tree pane has no anchors, no message previews, and shows nested contexts
    assert!(!screen.contains("⚓"), "Tree must not render ⚓ anchors:\n{screen}");
    assert!(
        !screen.contains("↳ How does auth work?"),
        "Tree must not render user question previews:\n{screen}"
    );
    assert!(screen.contains("ᛉ Main"), "Expected root context:\n{screen}");
    assert!(
        screen.contains("├─ ● Child Context"),
        "Expected depth-1 box-connected streaming child context:\n{screen}"
    );
    assert!(
        screen.contains("│  └─ Grandchild"),
        "Expected depth-2 box-connected idle grandchild without ᛉ:\n{screen}"
    );
    assert!(
        screen.contains("└─ Second Child"),
        "Expected depth-1 last idle sibling without ᛉ:\n{screen}"
    );
    assert!(
        !screen.contains("ᛉ Grandchild") && !screen.contains("ᛉ Second Child"),
        "Idle sub-branches must omit ᛉ:\n{screen}"
    );

    // User and agent messages remain visible in Transcript
    assert!(screen.contains("How does auth work?"));
    assert!(screen.contains("Auth Pipeline"));
    assert!(screen.contains("verifies JWT"));
}

#[test]
fn test_tree_vertical_scrolling_keeps_selected_context_visible() {
    let mut app = App::new();
    let threads: Vec<HydratedThread> = (0..14)
        .map(|i| HydratedThread {
            id: format!("t-{i}"),
            title: format!("Branch-{i:02}"),
            turns: Vec::new(),
        })
        .collect();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-0".to_string(),
            threads,
            blackboard: Vec::new(),
        },
    );

    // Small height: 10 rows total -> top panes height = 6, inner height = 4
    let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
    app.set_focus(FocusedPane::Tree);
    for _ in 0..13 {
        app.handle_key_event(key(KeyCode::Down));
    }
    assert_eq!(app.tree.selected_index(), Some(13));

    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(
        app.tree.scroll_offset > 0,
        "scroll_offset must advance when selection moves below viewport"
    );
    let screen = render_screen(&terminal);
    assert!(
        screen.contains("Branch-13"),
        "Selected bottom branch must be scrolled into view:\n{screen}"
    );
    assert!(
        !screen.contains("Branch-00"),
        "Top branch should have scrolled out of view:\n{screen}"
    );

    // Navigate back up to top
    for _ in 0..13 {
        app.handle_key_event(key(KeyCode::Up));
    }
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert_eq!(app.tree.scroll_offset, 0);
    let screen_top = render_screen(&terminal);
    assert!(screen_top.contains("Branch-00"));
}

#[test]
fn test_branch_deletion_key_command_and_sole_thread_guard() {
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
            blackboard: Vec::new(),
        },
    );

    // Sole-thread guard via 'd' in Tree
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('d')));
    assert!(
        app.pending_actions.is_empty(),
        "Sole thread must not emit DeleteThread"
    );
    assert!(app.status_notice.is_some());
    let mut guard_terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    draw_frame_mut(&mut guard_terminal, &mut app).unwrap();
    let guard_screen = render_screen(&guard_terminal);
    assert!(guard_screen.contains("Cannot delete the only context"));

    // Sole-thread guard via /delete in Composer
    app.status_notice = None;
    app.set_focus(FocusedPane::Composer);
    app.submit_composer_prompt("/delete");
    assert!(app.pending_actions.is_empty());
    assert!(app.status_notice.is_some());

    // Add second branch -> 'd' in Tree and /delete in Composer both emit DeleteThread
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: "t-2".to_string(),
                title: "Feature".to_string(),
                turns: Vec::new(),
            },
        },
    );
    assert_eq!(app.active_thread_id(), "t-2");

    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('d')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::DeleteThread("t-2".to_string()))
    );

    app.handle_key_event(key(KeyCode::Char('x')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::DeleteThread("t-2".to_string()))
    );

    app.set_focus(FocusedPane::Composer);
    app.submit_composer_prompt("/delete");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::DeleteThread("t-2".to_string()))
    );

    app.submit_composer_prompt("/close");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::DeleteThread("t-2".to_string()))
    );

    app.submit_composer_prompt("/grein alt-idea");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch {
            source_thread_id: "t-2".to_string(),
            title: Some("alt-idea".to_string()),
        })
    );

    // Handling ThreadDeleted removes branch state, switches active thread, rebuilds tree
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadDeleted {
            thread_id: "t-2".to_string(),
            next_active_thread_id: "t-1".to_string(),
        },
    );
    assert_eq!(app.threads.len(), 1);
    assert_eq!(app.active_thread_id(), "t-1");
    assert_eq!(app.tree.nodes.len(), 1);
}

#[tokio::test]
async fn test_engine_port_delete_thread_and_thread_metadata_roundtrip() {
    struct DeleteCapableEngine {
        deleted: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl TuiEnginePort for DeleteCapableEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-2".to_string(),
                threads: vec![
                    HydratedThread {
                        id: "t-1".to_string(),
                        title: "Main".to_string(),
                        turns: Vec::new(),
                    },
                    HydratedThread {
                        id: "t-2".to_string(),
                        title: "Side".to_string(),
                        turns: Vec::new(),
                    },
                ],
                blackboard: Vec::new(),
            })
        }
        async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
            Ok(())
        }
        async fn fork_thread(&self, _thread_id: &str, _anchor_id: &str) -> io::Result<String> {
            Ok("t-3".to_string())
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
            Ok(vec![
                ThreadMeta {
                    thread_id: "t-1".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "t-2".to_string(),
                    parent_thread_id: Some("t-1".to_string()),
                    inherited_turn_count: 3,
                    merged: false,
                },
            ])
        }
        async fn delete_thread(&self, thread_id: &str) -> Result<String, String> {
            self.deleted.lock().unwrap().push(thread_id.to_string());
            Ok("t-1".to_string())
        }
    }

    let engine = Arc::new(DeleteCapableEngine {
        deleted: Mutex::new(Vec::new()),
    });
    let engine_dyn: Arc<dyn TuiEnginePort> = engine.clone();
    let mut terminal = Terminal::new(TestBackend::new(110, 28)).unwrap();
    let app = App::new();

    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, app, engine_dyn, key_rx).await?;
        Ok::<_, io::Error>(terminal)
    });

    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    for ch in "/delete".chars() {
        key_tx
            .send(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            )))
            .unwrap();
    }
    key_tx
        .send(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    key_tx
        .send(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
        )))
        .unwrap();

    let terminal = handle.await.unwrap().unwrap();
    assert_eq!(engine.deleted.lock().unwrap().as_slice(), &["t-2"]);
    let screen = render_screen(&terminal);
    assert!(screen.contains("Main"));
    assert!(!screen.contains("Side"));
}

#[test]
fn test_fork_banner_renders_in_forked_transcript_only() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-child".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-root".to_string(),
                    title: "Main Trunk".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-child".to_string(),
                    title: "Bugfix".to_string(),
                    turns: vec![HydratedTurn {
                        id: "u1".to_string(),
                        role: "User".to_string(),
                        content: "branch local question".to_string(),
                    }],
                },
            ],
            blackboard: Vec::new(),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "t-root".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "t-child".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    inherited_turn_count: 4,
                    merged: false,
                },
            ],
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(150, 28)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let child_screen = render_screen(&terminal);
    assert!(
        child_screen.contains("> ↳ Forked from Main Trunk (4 ancestor turns in LLM context)"),
        "Expected fork banner in forked branch transcript:\n{child_screen}"
    );
    assert!(child_screen.contains("branch local question"));

    app.switch_thread("t-root");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let root_screen = render_screen(&terminal);
    assert!(!root_screen.contains("Forked from"));
}

#[test]
fn test_layout_with_chrome_and_subagent_strip_auto_hide_and_overflow() {
    let area = Rect::new(0, 0, 120, 30);
    // Keep legacy compute_layout intact
    let legacy = compute_layout(area);
    assert_eq!(legacy.composer_area.height, 3);

    // Zero subagents -> strip hidden (None)
    assert_eq!(subagent_strip_height(0), 0);
    let hidden = compute_layout_with_chrome(area, 0);
    assert!(hidden.subagent_strip_area.is_none());
    assert_eq!(hidden.status_bar_area, Rect::new(0, 29, 120, 1));
    assert_eq!(hidden.composer_area.y + hidden.composer_area.height, 29);

    // 7 subagents -> capped at 5 + 1 overflow + 2 border = 8 rows
    assert_eq!(subagent_strip_height(7), 8);
    let shown = compute_layout_with_chrome(area, subagent_strip_height(7));
    assert_eq!(shown.subagent_strip_area, Some(Rect::new(0, 21, 120, 8)));
    assert_eq!(shown.status_bar_area, Rect::new(0, 29, 120, 1));
    assert_eq!(shown.composer_area.y + shown.composer_area.height, 21);

    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-1".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-2".to_string(),
                    title: "Other".to_string(),
                    turns: Vec::new(),
                },
            ],
            blackboard: Vec::new(),
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let empty_screen = render_screen(&terminal);
    assert!(
        !empty_screen.contains("Subagents"),
        "Subagent strip must auto-hide when active thread has 0 subagents"
    );

    for i in 1..=7 {
        let status = match i {
            6 => SubagentStatus::Completed,
            7 => SubagentStatus::Failed,
            _ => SubagentStatus::Running,
        };
        apply_engine_event(
            &mut app,
            TuiEngineEvent::SubagentUpdated {
                thread_id: "t-1".to_string(),
                subagent: SubagentSnapshot {
                    id: format!("sa-{i}"),
                    role: format!("Worker {i}"),
                    agent_type: "research".to_string(),
                    model: Some("gemini-2.5-pro".to_string()),
                    activity: Some(format!("task {i}")),
                    status,
                    started_at_ms: 0,
                },
            },
        );
    }

    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let populated_screen = render_screen(&terminal);
    assert!(populated_screen.contains("Subagents"));
    assert!(populated_screen.contains("Agent(research: Worker 1)"));
    assert!(populated_screen.contains("gemini-2.5-pro"));
    assert!(populated_screen.contains("task 1"));
    assert!(populated_screen.contains("+2 more"));
    assert!(populated_screen.contains("7 subagents"));

    // Switching to t-2 (0 subagents) auto-hides the strip
    app.switch_thread("t-2");
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let other_screen = render_screen(&terminal);
    assert!(!other_screen.contains("Subagents"));
    assert!(other_screen.contains("0 subagents"));

    // Switching back and removing subagents eventually auto-hides the strip
    app.switch_thread("t-1");
    for i in 1..=7 {
        apply_engine_event(
            &mut app,
            TuiEngineEvent::SubagentRemoved {
                thread_id: "t-1".to_string(),
                subagent_id: format!("sa-{i}"),
            },
        );
    }
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let cleared_screen = render_screen(&terminal);
    assert!(!cleared_screen.contains("Subagents"));
}

#[test]
fn test_bottom_status_bar_minimal_composer_and_live_activity() {
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
            blackboard: Vec::new(),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::AgentMetaUpdated {
            thread_id: "t-1".to_string(),
            model: Some("claude-sonnet".to_string()),
            mode: Some("code".to_string()),
        },
    );

    let mut terminal = Terminal::new(TestBackend::new(120, 26)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let idle_screen = render_screen(&terminal);
    assert!(!idle_screen.contains("Enter: Send"));
    assert!(!idle_screen.contains("!cmd: Shell"));
    assert!(idle_screen.contains("? for help"));
    assert!(idle_screen.contains("code · claude-sonnet"));
    assert!(idle_screen.contains("0 subagents"));

    // ThoughtChunk shows thinking and esc to cancel
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThoughtChunk {
            thread_id: "t-1".to_string(),
            content: "analyzing diff".to_string(),
        },
    );
    app.handle_event(AppEvent::Tick);
    assert_eq!(app.tick_count, 1);
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let thinking_screen = render_screen(&terminal);
    assert!(thinking_screen.contains("esc to cancel"));
    assert!(thinking_screen.contains("thinking"));

    // ToolActivityUpdated sets tool name, clearing restores streaming
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolActivityUpdated {
            thread_id: "t-1".to_string(),
            tool_name: Some("read_file".to_string()),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let tool_screen = render_screen(&terminal);
    assert!(tool_screen.contains("read_file"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolActivityUpdated {
            thread_id: "t-1".to_string(),
            tool_name: None,
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let after_tool_activity_screen = render_screen(&terminal);
    assert!(after_tool_activity_screen.contains("streaming"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolInvoked {
            thread_id: "t-1".to_string(),
            tool_name: "grep_search".to_string(),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    assert!(render_screen(&terminal).contains("grep_search"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::ToolResult {
            thread_id: "t-1".to_string(),
            tool_name: "grep_search".to_string(),
            output: "ok".to_string(),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let streaming_screen = render_screen(&terminal);
    assert!(streaming_screen.contains("streaming"));

    apply_engine_event(
        &mut app,
        TuiEngineEvent::TurnCompleted {
            thread_id: "t-1".to_string(),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let done_screen = render_screen(&terminal);
    assert!(done_screen.contains("? for help"));
    assert!(done_screen.contains("idle"));
}

#[test]
fn test_help_modal_triggers_rendering_and_dismissal() {
    let mut app = App::new();

    // Pressing '?' with empty Composer opens modal without inserting '?'
    app.set_focus(FocusedPane::Composer);
    app.handle_key_event(key(KeyCode::Char('?')));
    assert!(app.show_help_modal);
    assert_eq!(app.composer_input(), "");

    // Render modal and check keybind contents
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let modal_screen = render_screen(&terminal);
    assert!(modal_screen.contains("/fork"));
    assert!(modal_screen.contains("/merge"));
    assert!(modal_screen.contains("/delete"));
    assert!(modal_screen.contains("/help"));
    assert!(modal_screen.contains("!cmd"));
    assert!(modal_screen.contains("Tab / Shift+Tab"));
    assert!(modal_screen.contains("Ctrl+C"));

    // Dismiss with Esc without quitting or changing focus
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.show_help_modal);
    assert!(!app.should_quit());
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    // Mid-sentence '?' in non-empty Composer inserts '?' normally
    app.handle_key_event(key(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('h')));
    app.handle_key_event(key(KeyCode::Char('y')));
    app.handle_key_event(key(KeyCode::Char('?')));
    assert!(!app.show_help_modal);
    assert_eq!(app.composer_input(), "why?");
    app.input_buffer.clear();
    app.cursor_position = 0;

    // Submit /? and /help in Composer -> opens modal; dismiss with q, ?, Enter
    app.submit_composer_prompt("/?");
    assert!(app.show_help_modal);
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(!app.show_help_modal);
    assert!(!app.should_quit());

    app.submit_composer_prompt("/help");
    assert!(app.show_help_modal);
    app.handle_key_event(key(KeyCode::Char('?')));
    assert!(!app.show_help_modal);
    assert_eq!(app.composer_input(), "");

    // Open from Tree and Transcript and dismiss with Enter
    for pane in [FocusedPane::Tree, FocusedPane::Transcript] {
        app.set_focus(pane);
        app.handle_key_event(key(KeyCode::Char('?')));
        assert!(app.show_help_modal, "Expected '?' to open modal from {pane:?}");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(!app.show_help_modal);
        assert_eq!(app.focused_pane(), pane);
    }
}

#[test]
fn test_live_branch_preview_and_rune_lore() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-main".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-main".to_string(),
                    title: "Main".to_string(),
                    turns: vec![HydratedTurn {
                        id: "u1".to_string(),
                        role: "User".to_string(),
                        content: "main branch prompt".to_string(),
                    }],
                },
                HydratedThread {
                    id: "t-alt".to_string(),
                    title: "Feature Alt".to_string(),
                    turns: vec![HydratedTurn {
                        id: "u2".to_string(),
                        role: "User".to_string(),
                        content: "feature preview prompt".to_string(),
                    }],
                },
            ],
            blackboard: Vec::new(),
        },
    );
    app.input_buffer = "keep-my-draft".to_string();
    app.cursor_position = app.input_buffer.len();

    let mut terminal = Terminal::new(TestBackend::new(120, 26)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let initial_screen = render_screen(&terminal);
    assert!(
        initial_screen.contains("ᛇ Grein"),
        "Expected 'ᛇ Grein' left panel title:\n{initial_screen}"
    );
    assert!(initial_screen.contains("ᛉ Main"));
    assert!(initial_screen.contains("❯ main branch prompt"));
    assert!(!initial_screen.contains("Transcript (Preview:"));

    // Focus left panel and move selection to Feature Alt -> live preview without switching
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.active_thread_id(), "t-main");
    assert_eq!(app.composer_input(), "keep-my-draft");

    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let preview_screen = render_screen(&terminal);
    assert!(
        preview_screen.contains("Transcript (Preview: Feature Alt)"),
        "Expected preview title in Transcript:\n{preview_screen}"
    );
    assert!(
        preview_screen.contains("❯ feature preview prompt"),
        "Expected previewed branch prompt in Transcript:\n{preview_screen}"
    );

    // Enter commits the branch swap, restores non-preview Transcript title, and focuses Composer
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.active_thread_id(), "t-alt");
    assert_eq!(app.focused_pane(), FocusedPane::Composer);

    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let swapped_screen = render_screen(&terminal);
    assert!(!swapped_screen.contains("Transcript (Preview:"));
    assert!(swapped_screen.contains("❯ feature preview prompt"));
}

#[test]
fn test_multi_root_three_level_connectors_and_joined_badge_rendering() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "r1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "r1".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "b1".to_string(),
                    title: "Branch A".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "b2".to_string(),
                    title: "Branch B".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "g1".to_string(),
                    title: "Grandchild".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "gg1".to_string(),
                    title: "Great-Grand".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "r2".to_string(),
                    title: "Root 2".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "r2c".to_string(),
                    title: "Root2 Child".to_string(),
                    turns: Vec::new(),
                },
            ],
            blackboard: Vec::new(),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "r1".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "b1".to_string(),
                    parent_thread_id: Some("r1".to_string()),
                    inherited_turn_count: 0,
                    merged: true,
                },
                ThreadMeta {
                    thread_id: "b2".to_string(),
                    parent_thread_id: Some("r1".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "g1".to_string(),
                    parent_thread_id: Some("b2".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "gg1".to_string(),
                    parent_thread_id: Some("g1".to_string()),
                    inherited_turn_count: 0,
                    merged: true,
                },
                ThreadMeta {
                    thread_id: "r2".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "r2c".to_string(),
                    parent_thread_id: Some("r2".to_string()),
                    inherited_turn_count: 0,
                    merged: false,
                },
            ],
        },
    );

    assert_eq!(app.tree.prefix_for("r1"), "");
    assert_eq!(app.tree.prefix_for("b1"), "├─ ");
    assert_eq!(app.tree.prefix_for("b2"), "└─ ");
    assert_eq!(app.tree.prefix_for("g1"), "   └─ ");
    assert_eq!(app.tree.prefix_for("gg1"), "      └─ ");
    assert_eq!(app.tree.prefix_for("r2"), "");
    assert_eq!(app.tree.prefix_for("r2c"), "└─ ");

    let mut terminal = Terminal::new(TestBackend::new(130, 28)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);

    assert!(screen.contains("ᛉ Main"), "{screen}");
    assert!(screen.contains("├─ Branch A [joined]"), "{screen}");
    assert!(screen.contains("└─ Branch B"), "{screen}");
    assert!(screen.contains("   └─ Grandchild"), "{screen}");
    assert!(screen.contains("      └─ Great-Grand [joined]"), "{screen}");
    assert!(screen.contains("ᛉ Root 2"), "{screen}");
    assert!(screen.contains("└─ Root2 Child"), "{screen}");
    assert!(
        !screen.contains("ᛉ Branch")
            && !screen.contains("ᛉ Grandchild")
            && !screen.contains("ᛉ Great-Grand")
            && !screen.contains("ᛉ Root2 Child"),
        "Idle sub-branches must omit ᛉ:\n{screen}"
    );

    // Verify [joined] badge uses DarkGray styling
    let buf = terminal.backend().buffer();
    let mut found_dark_gray_joined = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(byte_idx) = row.find("[joined]") {
            let col_idx = row[..byte_idx].chars().count() as u16;
            assert_eq!(buf[(col_idx, y)].fg, ratatui::style::Color::DarkGray);
            found_dark_gray_joined = true;
            break;
        }
    }
    assert!(found_dark_gray_joined, "Expected [joined] styled with Color::DarkGray");

    // Fallback when tree.nodes is empty still computes box connectors on the fly
    app.tree.nodes.clear();
    app.tree.prefixes.clear();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let fallback_screen = render_screen(&terminal);
    assert!(fallback_screen.contains("├─ Branch A [joined]"), "{fallback_screen}");
    assert!(fallback_screen.contains("      └─ Great-Grand [joined]"), "{fallback_screen}");
    assert!(!fallback_screen.contains("ᛉ Branch"), "{fallback_screen}");
}

#[test]
fn test_root_create_rename_and_global_alt_n_alt_r_f2_shortcuts() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-main".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-main".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-child".to_string(),
                    title: "Side Work".to_string(),
                    turns: Vec::new(),
                },
            ],
            blackboard: Vec::new(),
        },
    );

    // /root and /new in Composer
    app.set_focus(FocusedPane::Composer);
    app.submit_composer_prompt("/root Fresh Trunk");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateRoot {
            title: Some("Fresh Trunk".to_string()),
        })
    );
    app.submit_composer_prompt("/new");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateRoot { title: None })
    );

    // 'n' in Tree emits CreateRoot { title: None }
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateRoot { title: None })
    );

    // Global Alt+N from Composer and Tree emits CreateRoot { title: None }
    for pane in [FocusedPane::Composer, FocusedPane::Tree, FocusedPane::Transcript] {
        app.set_focus(pane);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::CreateRoot { title: None })
        );
    }

    // Alt+1..9 remain unbound
    app.set_focus(FocusedPane::Composer);
    for digit in '1'..='9' {
        assert!(!app.handle_key_event(KeyEvent::new(KeyCode::Char(digit), KeyModifiers::ALT)));
    }

    // Empty /rename sets usage status_notice without emitting RenameThread
    app.submit_composer_prompt("/rename   ");
    assert!(app.pending_actions.is_empty());
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Usage: /rename <new title>")
    );

    // 'r' in Tree on unselected child prefills "/rename Side Work" and targets child
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1);
    app.handle_key_event(key(KeyCode::Char('r')));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.composer_input(), "/rename Side Work");
    assert_eq!(app.cursor_position, "/rename Side Work".len());
    assert_eq!(app.pending_rename_thread_id.as_deref(), Some("t-child"));

    // Submitting edited /rename targets pending_rename_thread_id (t-child) while active stays t-main
    app.input_buffer = "/rename Auth Branch".to_string();
    app.cursor_position = app.input_buffer.len();
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::RenameThread {
            thread_id: "t-child".to_string(),
            title: "Auth Branch".to_string(),
        })
    );
    assert!(app.pending_rename_thread_id.is_none());

    // Global Alt+R from Composer targets active_thread_id (t-main)
    app.set_focus(FocusedPane::Composer);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT)));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.composer_input(), "/rename Main");
    assert_eq!(app.pending_rename_thread_id.as_deref(), Some("t-main"));
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::RenameThread {
            thread_id: "t-main".to_string(),
            title: "Main".to_string(),
        })
    );

    // Global F2 from Tree targets selected thread, and from Transcript targets active_thread_id
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1);
    assert!(app.handle_key_event(key(KeyCode::F(2))));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.composer_input(), "/rename Side Work");
    assert_eq!(app.pending_rename_thread_id.as_deref(), Some("t-child"));

    app.switch_thread("t-main");
    assert!(app.pending_rename_thread_id.is_none());
    app.set_focus(FocusedPane::Transcript);
    assert!(app.handle_key_event(key(KeyCode::F(2))));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.composer_input(), "/rename Main");
    assert_eq!(app.pending_rename_thread_id.as_deref(), Some("t-main"));
}

#[test]
fn test_join_merge_commands_hotkey_event_and_help_modal() {
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-child".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-root".to_string(),
                    title: "Main".to_string(),
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-child".to_string(),
                    title: "Auth Fix".to_string(),
                    turns: Vec::new(),
                },
            ],
            blackboard: Vec::new(),
        },
    );
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![
                ThreadMeta {
                    thread_id: "t-root".to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                },
                ThreadMeta {
                    thread_id: "t-child".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    inherited_turn_count: 1,
                    merged: false,
                },
            ],
        },
    );

    // /join with summary and /merge without summary on child thread
    app.set_focus(FocusedPane::Composer);
    app.submit_composer_prompt("/join JWT check works");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::MergeBranch {
            source_thread_id: "t-child".to_string(),
            target_thread_id: "t-root".to_string(),
            summary: Some("JWT check works".to_string()),
        })
    );
    app.submit_composer_prompt("/merge");
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::MergeBranch {
            source_thread_id: "t-child".to_string(),
            target_thread_id: "t-root".to_string(),
            summary: None,
        })
    );

    // 'm' in Tree on child -> target t-root; on root -> target ""
    app.set_focus(FocusedPane::Tree);
    app.tree.select_index(1);
    app.handle_key_event(key(KeyCode::Char('m')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::MergeBranch {
            source_thread_id: "t-child".to_string(),
            target_thread_id: "t-root".to_string(),
            summary: None,
        })
    );
    app.tree.select_index(0);
    app.handle_key_event(key(KeyCode::Char('m')));
    assert!(app.pending_actions.is_empty());
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Cannot join a root context (no parent branch)")
    );

    // ThreadMerged marks .merged = true, appends "> ↳ Joined from ..." into target transcript, sets status_notice
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMerged {
            thread_id: "t-child".to_string(),
            target_thread_id: "t-root".to_string(),
            summary: "JWT check works".to_string(),
        },
    );
    assert!(app.thread_meta.get("t-child").is_some_and(|m| m.merged));
    assert!(app
        .thread_transcripts
        .get("t-root")
        .is_some_and(|md| md.contains("> ↳ Joined from Auth Fix: JWT check works")));
    assert!(app
        .status_notice
        .as_deref()
        .is_some_and(|n| n.contains("Joined") && n.contains("Auth Fix")));

    // Root ThreadMerged sets status_notice without modifying target transcript
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadMerged {
            thread_id: "t-root".to_string(),
            target_thread_id: String::new(),
            summary: "Promoted summary".to_string(),
        },
    );
    assert!(app.thread_meta.get("t-root").is_some_and(|m| m.merged));
    assert!(app
        .status_notice
        .as_deref()
        .is_some_and(|n| n.contains("Shared Context")));

    // Help modal renders /root, /rename, /join, n / r / m, Alt+N, Alt+R / F2 cleanly without clipping
    app.show_help_modal = true;
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let help_screen = render_screen(&terminal);
    assert!(help_screen.contains("/root, /new"), "{help_screen}");
    assert!(help_screen.contains("/rename"), "{help_screen}");
    assert!(help_screen.contains("/join, /merge"), "{help_screen}");
    assert!(help_screen.contains("n / r / m (ᛇ Grein)"), "{help_screen}");
    assert!(help_screen.contains("Alt+N"), "{help_screen}");
    assert!(help_screen.contains("Alt+R / F2"), "{help_screen}");
    assert!(help_screen.contains("Press Esc, q, ?, or Enter to close"), "{help_screen}");

    // Also fits cleanly on a standard 80x24 terminal without clipping footer
    let mut small_term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    draw_frame_mut(&mut small_term, &mut app).unwrap();
    let small_help = render_screen(&small_term);
    assert!(small_help.contains("Press Esc, q, ?, or Enter to close"), "{small_help}");

    // Non-idle error sub-branch retains ⚠ glyph with tree connector
    app.show_help_modal = false;
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadStatusChanged {
            thread_id: "t-child".to_string(),
            status: "error".to_string(),
        },
    );
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let err_screen = render_screen(&terminal);
    assert!(err_screen.contains("└─ ⚠ Auth Fix [joined]"), "{err_screen}");
}
