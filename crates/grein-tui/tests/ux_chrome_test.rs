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
                },
                ThreadMeta {
                    thread_id: "t-child".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    inherited_turn_count: 2,
                },
                ThreadMeta {
                    thread_id: "t-grandchild".to_string(),
                    parent_thread_id: Some("t-child".to_string()),
                    inherited_turn_count: 2,
                },
                ThreadMeta {
                    thread_id: "t-sibling".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    inherited_turn_count: 2,
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
    assert!(screen.contains("✓ Main"), "Expected root context:\n{screen}");
    assert!(
        screen.contains("  ● Child Context"),
        "Expected depth-1 indented child context:\n{screen}"
    );
    assert!(
        screen.contains("    ✓ Grandchild"),
        "Expected depth-2 indented grandchild context:\n{screen}"
    );
    assert!(
        screen.contains("  ✓ Second Child"),
        "Expected depth-1 sibling context:\n{screen}"
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
                },
                ThreadMeta {
                    thread_id: "t-2".to_string(),
                    parent_thread_id: Some("t-1".to_string()),
                    inherited_turn_count: 3,
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
                },
                ThreadMeta {
                    thread_id: "t-child".to_string(),
                    parent_thread_id: Some("t-root".to_string()),
                    inherited_turn_count: 4,
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
    let legacy = compute_layout(area, false);
    assert_eq!(legacy.composer_area.height, 3);

    // Zero subagents -> strip hidden (None)
    assert_eq!(subagent_strip_height(0), 0);
    let hidden = compute_layout_with_chrome(area, false, 0);
    assert!(hidden.subagent_strip_area.is_none());
    assert_eq!(hidden.status_bar_area, Rect::new(0, 29, 120, 1));
    assert_eq!(hidden.composer_area.y + hidden.composer_area.height, 29);

    // 7 subagents -> capped at 5 + 1 overflow + 2 border = 8 rows
    assert_eq!(subagent_strip_height(7), 8);
    let shown = compute_layout_with_chrome(area, false, subagent_strip_height(7));
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
    assert!(modal_screen.contains("Tab/Shift+Tab"));
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

    // Open from Tree, Transcript, Blackboard and dismiss with Enter
    for pane in [
        FocusedPane::Tree,
        FocusedPane::Transcript,
        FocusedPane::Blackboard,
    ] {
        app.set_focus(pane);
        app.handle_key_event(key(KeyCode::Char('?')));
        assert!(app.show_help_modal, "Expected '?' to open modal from {pane:?}");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(!app.show_help_modal);
        assert_eq!(app.focused_pane(), pane);
    }
}
