mod dispatch;
mod engine_events;
mod persistence;
mod transcript_sync;
mod tree_order;

pub use engine_events::{apply_engine_event, apply_hydrated_session};
pub use transcript_sync::now_epoch_ms;
pub use tree_order::{
    compute_ordered_thread_entries, compute_ordered_thread_entries_with_folds,
    compute_ordered_thread_indices, rebuild_tree_nodes, FoldedThreadEntry, OrderedThreadEntry,
};

use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::Backend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::app::{
    find_config_option, match_config_choice, App, AppAction, FocusedPane, TreeAction, TuiError,
};
use crate::engine_port::{TuiEngineEvent, TuiEnginePort};

use dispatch::{
    cache_snapshot_options, drain_pending_actions, handle_streamed_engine_event, RunnerLocalEvent,
};
use transcript_sync::{
    ensure_paragraph_break, sync_thread_blocks_md, with_thread_transcript_mut,
};
use tree_order::rebuild_tree_nodes as rebuild_nodes;

pub struct TerminalGuard {
    active: bool,
    keyboard_enhanced: bool,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        if io::stdout().is_terminal() && io::stdin().is_terminal() {
            enable_raw_mode()?;
            execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
            let keyboard_enhanced =
                if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
                    execute!(
                        io::stdout(),
                        PushKeyboardEnhancementFlags(
                            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        )
                    )
                    .is_ok()
                } else {
                    false
                };
            Ok(Self {
                active: true,
                keyboard_enhanced,
            })
        } else {
            Ok(Self {
                active: false,
                keyboard_enhanced: false,
            })
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            if self.keyboard_enhanced {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        }
    }
}

pub fn draw_frame<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> io::Result<()> {
    terminal.draw(|frame| crate::render::render(frame, app))?;
    Ok(())
}

pub fn draw_frame_mut<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    terminal.draw(|frame| crate::render::render_mut(frame, app))?;
    Ok(())
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<bool> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Release => {}
        Event::Key(key) => {
            let _ = handle_key_event(app, key);
        }
        Event::Paste(text) => {
            let _ = app.handle_paste_event(&text);
        }
        Event::Mouse(mouse) => {
            let _ = app.handle_mouse_event(mouse);
        }
        Event::Resize(w, h) => {
            app.last_frame_area = Some(ratatui::layout::Rect::new(0, 0, w, h));
            app.maybe_queue_terminal_resize();
        }
        _ => {}
    }
    Ok(app.should_quit())
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<Option<TreeAction>, TuiError> {
    if key.kind == KeyEventKind::Release {
        return Ok(None);
    }
    let was_composer = app.focused_pane() == FocusedPane::Composer
        || app.focus() == FocusedPane::Composer
        || app.active_focus() == FocusedPane::Composer;
    let is_enter = key.code == KeyCode::Enter
        && !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT);
    if was_composer && is_enter {
        app.last_submitted_prompt = None;
    }
    app.handle_key_event(key);
    if key.code == KeyCode::Enter && app.focused_pane() == FocusedPane::Tree {
        if let Some(idx) = app.tree.selected_index() {
            if let Some(crate::ui::tree::TreeNode::Anchor(anchor)) = app.tree.nodes.get(idx) {
                return Ok(Some(TreeAction::ActivateAnchor(anchor.id.clone())));
            }
        }
    }
    Ok(None)
}

fn spawn_key_reader(
    stop: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<Event>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(25)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    })
}

pub async fn run_with_engine<B: Backend>(
    terminal: &mut Terminal<B>,
    app: App,
    engine: Arc<dyn TuiEnginePort>,
) -> io::Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let (key_tx, key_rx) = mpsc::unbounded_channel::<Event>();
    let reader = spawn_key_reader(Arc::clone(&stop), key_tx);
    let result = run_with_engine_events(terminal, app, engine, key_rx).await;
    stop.store(true, Ordering::Relaxed);
    let _ = reader.join();
    result
}

pub async fn run_with_engine_events<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: App,
    engine: Arc<dyn TuiEnginePort>,
    mut key_rx: mpsc::UnboundedReceiver<Event>,
) -> io::Result<()> {
    let mut engine_events = engine.subscribe_events().await?;
    let session = engine.hydrate_session().await?;
    apply_hydrated_session(&mut app, &session);
    match engine.load_preferences().await {
        Ok(prefs) => {
            app.apply_preferences(&prefs);
        }
        Err(err) => {
            app.status_notice = Some(format!("Preferences load failed: {err}"));
        }
    }
    match engine.thread_metadata().await {
        Ok(meta) => {
            if !meta.is_empty() {
                apply_engine_event(&mut app, TuiEngineEvent::ThreadMetadataUpdated { meta });
            }
        }
        Err(err) => {
            app.status_notice = Some(format!("Metadata sync failed: {err}"));
        }
    }

    let (local_ev_tx, mut local_ev_rx) = mpsc::unbounded_channel::<RunnerLocalEvent>();
    let (err_tx, mut err_rx) = mpsc::unbounded_channel::<(Option<String>, String)>();
    let (pty_write_tx, mut pty_write_rx) = mpsc::unbounded_channel::<(String, Vec<u8>)>();
    let pty_engine = Arc::clone(&engine);
    let pty_local_ev_tx = local_ev_tx.clone();
    let pty_writer_handle = tokio::spawn(async move {
        while let Some((thread_id, bytes)) = pty_write_rx.recv().await {
            if let Err(err) = pty_engine.pty_write(&thread_id, bytes).await {
                let _ = pty_local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                    "Terminal input failed: {err}"
                )));
            }
        }
    });
    let mut pending_local_merges: HashMap<String, usize> = HashMap::new();
    while let Ok(event) = engine_events.try_recv() {
        handle_streamed_engine_event(&mut app, event, &mut pending_local_merges);
    }
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    drain_pending_actions(
        &mut app,
        &engine,
        &local_ev_tx,
        &err_tx,
        &pty_write_tx,
        &mut pending_local_merges,
    );

    loop {
        draw_frame_mut(terminal, &mut app)?;
        drain_pending_actions(
            &mut app,
            &engine,
            &local_ev_tx,
            &err_tx,
            &pty_write_tx,
            &mut pending_local_merges,
        );
        if app.should_quit() {
            break;
        }

        tokio::select! {
            ev_opt = key_rx.recv() => {
                let Some(ev) = ev_opt else {
                    break;
                };
                handle_event(&mut app, ev)?;
                drain_pending_actions(
                    &mut app,
                    &engine,
                    &local_ev_tx,
                    &err_tx,
                    &pty_write_tx,
                    &mut pending_local_merges,
                );
            }
            Some(event) = engine_events.recv() => {
                handle_streamed_engine_event(&mut app, event, &mut pending_local_merges);
            }
            Some(local_event) = local_ev_rx.recv() => {
                match local_event {
                    RunnerLocalEvent::Engine(event) => apply_engine_event(&mut app, event),
                    RunnerLocalEvent::OptimisticThreadMeta(meta) => {
                        if !app.thread_meta.contains_key(&meta.thread_id) {
                            app.thread_meta.insert(meta.thread_id.clone(), meta);
                            let active = app.active_thread_id.clone();
                            app.unfold_ancestors_for(&active);
                            rebuild_nodes(&mut app);
                            app.sync_focus_for_active_thread();
                            app.maybe_request_pty_snapshot(&active);
                        }
                    }
                    RunnerLocalEvent::StatusNotice(notice) => {
                        app.status_notice = Some(notice);
                    }
                    RunnerLocalEvent::PtySnapshotLoaded { thread_id, snapshot } => {
                        apply_engine_event(
                            &mut app,
                            TuiEngineEvent::PtyScreenUpdated { thread_id, snapshot },
                        );
                    }
                    RunnerLocalEvent::WorktreeReady { thread_id, path } => {
                        if let Some(meta) = app.thread_meta.get_mut(&thread_id) {
                            meta.cwd = Some(path.clone());
                        }
                        app.status_notice = Some(format!("Worktree ready: {path}"));
                    }
                    RunnerLocalEvent::WorktreesListed(entries) => {
                        app.worktree_list_in_flight = false;
                        app.worktrees = entries;
                        app.clamp_worktree_selection();
                        app.sync_worktree_autocomplete(false);
                        app.maybe_fetch_selected_worktree_diff(false);
                        if app.status_notice.as_deref() == Some("Refreshing worktrees...") {
                            app.status_notice = None;
                        }
                    }
                    RunnerLocalEvent::WorktreesListFailed(err) => {
                        app.worktree_list_in_flight = false;
                        app.status_notice = Some(format!("List worktrees failed: {err}"));
                    }
                    RunnerLocalEvent::WorktreeSwitched { thread_id, path } => {
                        if let Some(meta) = app.thread_meta.get_mut(&thread_id) {
                            meta.cwd = Some(path.clone());
                        }
                        app.status_notice = Some(format!("Switched worktree: {path}"));
                    }
                    RunnerLocalEvent::WorktreeRemoved { path } => {
                        app.worktrees.retain(|w| w.path != path);
                        app.clamp_worktree_selection();
                        if app.worktree_diff_path.as_deref() == Some(path.as_str()) {
                            app.worktree_diff_path = None;
                            app.worktree_diff_preview.clear();
                            app.worktree_diff_scroll = 0;
                            app.worktree_diff_max_scroll = 0;
                        }
                        app.maybe_fetch_selected_worktree_diff(false);
                        app.status_notice = Some(format!("Deleted worktree: {path}"));
                    }
                    RunnerLocalEvent::WorktreeDiffLoaded { path, diff } => {
                        if app.worktree_diff_in_flight.as_deref() == Some(path.as_str()) {
                            app.worktree_diff_in_flight = None;
                        }
                        if app.worktree_diff_path.as_deref() == Some(path.as_str()) {
                            app.worktree_diff_preview = diff;
                            app.worktree_diff_scroll = 0;
                            app.worktree_diff_max_scroll = 0;
                        }
                    }
                    RunnerLocalEvent::WorktreeDiffFailed { path, err } => {
                        if app.worktree_diff_in_flight.as_deref() == Some(path.as_str()) {
                            app.worktree_diff_in_flight = None;
                        }
                        if app.worktree_diff_path.as_deref() == Some(path.as_str()) {
                            app.worktree_diff_preview = format!("Diff unavailable: {err}");
                        }
                    }
                    RunnerLocalEvent::WorktreesPruned(summary) => {
                        let msg = if summary.trim().is_empty() {
                            "Pruned stale worktrees".to_string()
                        } else {
                            summary
                        };
                        app.status_notice = Some(msg);
                    }
                    RunnerLocalEvent::OpenConfigPicker {
                        snapshot,
                        config_id,
                    } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        app.open_config_picker_from_options(&snapshot.thread_id, &config_id);
                    }
                    RunnerLocalEvent::ResolveConfigQuery {
                        snapshot,
                        config_id,
                        query,
                        set_default,
                    } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        let tid = snapshot.thread_id.clone();
                        if let Some(opt) = app
                            .thread_config_options
                            .get(&tid)
                            .and_then(|opts| find_config_option(opts, &config_id))
                            .cloned()
                        {
                            if opt.options.is_empty() {
                                app.pending_actions.push_back(AppAction::SetAgentConfig {
                                    thread_id: tid,
                                    config_id: opt.id,
                                    value: query,
                                    set_default,
                                });
                            } else if let Some(choice) = match_config_choice(&opt.options, &query) {
                                app.pending_actions.push_back(AppAction::SetAgentConfig {
                                    thread_id: tid,
                                    config_id: opt.id,
                                    value: choice.value.clone(),
                                    set_default,
                                });
                            } else {
                                app.status_notice =
                                    Some(format!("No {} option matching '{query}'", opt.name));
                            }
                        } else {
                            app.status_notice =
                                Some(format!("Option '{config_id}' not exposed by agent"));
                        }
                    }
                    RunnerLocalEvent::ConfigApplied {
                        snapshot,
                        config_id,
                        value,
                        set_default,
                    } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        let tid = if snapshot.thread_id.is_empty() {
                            app.active_thread_id.clone()
                        } else {
                            snapshot.thread_id
                        };
                        if snapshot.config_options.is_empty() {
                            if let Some(opts) = app.thread_config_options.get_mut(&tid) {
                                if let Some(opt) = opts.iter_mut().find(|o| o.id == config_id) {
                                    opt.current_value = value.clone();
                                }
                                let cloned = opts.clone();
                                app.sync_agent_meta_from_config_options(&tid, &cloned);
                            }
                        }
                        let scope = if set_default { "default" } else { "branch" };
                        app.status_notice = Some(format!("Set {config_id} = {value} ({scope})"));
                    }
                    RunnerLocalEvent::CommandsFetched { snapshot } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        let tid = snapshot.thread_id.clone();
                        app.record_agent_commands_note(&tid, &snapshot.available_commands);
                    }
                    RunnerLocalEvent::MergeFallback {
                        thread_id,
                        target_thread_id,
                        summary,
                    } => {
                        while let Ok(ev) = engine_events.try_recv() {
                            handle_streamed_engine_event(&mut app, ev, &mut pending_local_merges);
                        }
                        if let Some(cnt) = pending_local_merges.get_mut(&thread_id) {
                            if *cnt > 0 {
                                *cnt -= 1;
                                apply_engine_event(
                                    &mut app,
                                    TuiEngineEvent::ThreadMerged {
                                        thread_id,
                                        target_thread_id,
                                        summary: summary.unwrap_or_default(),
                                    },
                                );
                            }
                        }
                    }
                    RunnerLocalEvent::MergeAborted(thread_id) => {
                        if let Some(cnt) = pending_local_merges.get_mut(&thread_id) {
                            if *cnt > 0 {
                                *cnt -= 1;
                            }
                        }
                    }
                }
                drain_pending_actions(
                    &mut app,
                    &engine,
                    &local_ev_tx,
                    &err_tx,
                    &pty_write_tx,
                    &mut pending_local_merges,
                );
            }
            Some((thread_id_opt, message)) = err_rx.recv() => {
                let target_thread = thread_id_opt.unwrap_or_else(|| app.active_thread_id.clone());
                app.streaming_assistant_threads.remove(&target_thread);
                app.thread_thoughts.remove(&target_thread);
                app.thread_active_tool.remove(&target_thread);
                app.thread_turn_started_ms.remove(&target_thread);
                app.thread_statuses
                    .insert(target_thread.clone(), "error".to_string());
                app.status_notice = Some(format!("Error: {message}"));
                with_thread_transcript_mut(&mut app, &target_thread, |buf| {
                    ensure_paragraph_break(buf);
                    buf.push_str(&format!("Error: {message}\n\n"));
                });
                app.thread_blocks
                    .entry(target_thread.clone())
                    .or_default()
                    .push_error(message);
                sync_thread_blocks_md(&mut app, &target_thread);
                if target_thread == app.active_thread_id {
                    app.sync_anchors();
                }
            }
            _ = tick.tick() => {
                app.tick_count = app.tick_count.wrapping_add(1);
            }
        }
    }

    drop(pty_write_tx);
    let _ = pty_writer_handle.await;

    Ok(())
}
