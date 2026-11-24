use std::collections::{BTreeSet, HashMap};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_core::ThreadKind;
use grein_tui::app::{App, AppAction, ConfigPickerModal, ThreadItem};
use grein_tui::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentOptionsSnapshot, HydratedSession, HydratedThread,
    PtyCell, PtyScreenSnapshot, ThreadMeta, TuiEngineEvent, TuiEnginePort, UserPreferencesSnapshot,
    WorktreeEntry,
};
use grein_tui::runner::{
    apply_engine_event, apply_hydrated_session, compute_ordered_thread_entries,
    compute_ordered_thread_entries_with_folds, handle_event, run_with_engine_events,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;

#[derive(Default)]
struct MockEngineState {
    session: Option<HydratedSession>,
    metadata: Vec<ThreadMeta>,
    worktrees: Vec<WorktreeEntry>,
    agent_options_snapshot: AgentOptionsSnapshot,
    set_config_snapshot: Option<AgentOptionsSnapshot>,
    pty_snapshot_data: Option<PtyScreenSnapshot>,
    recent_prompts: Vec<String>,
    workspace_files: Vec<String>,
    event_tx: Option<mpsc::UnboundedSender<TuiEngineEvent>>,
    calls: Vec<String>,
    pty_resizes: Vec<(String, u16, u16)>,
    set_config_calls: Vec<(String, String, String, bool)>,
    fail_load_preferences: bool,
    fail_thread_metadata: bool,
    fail_send_turn: bool,
    fail_run_shell_command: bool,
    fail_fork_branch: bool,
    fail_fork_thread: bool,
    fail_create_root: bool,
    fail_create_root_in_cwd: bool,
    fail_rename_thread: bool,
    fail_merge_thread_with_summary: bool,
    fail_delete_thread: bool,
    fail_cancel_turn: bool,
    fail_resolve_permission: bool,
    fail_agent_options: bool,
    fail_set_agent_config: bool,
    fail_create_terminal: bool,
    fail_pty_write: bool,
    fail_pty_resize: bool,
    fail_pty_snapshot: bool,
    fail_pty_scroll: bool,
    fail_pty_restart: bool,
    fail_ensure_thread_worktree: bool,
    fail_list_worktrees: bool,
    fail_switch_thread_worktree: bool,
    fail_remove_worktree: bool,
    fail_worktree_diff_preview: bool,
    fail_prune_worktrees: bool,
    fail_list_recent_user_prompts: bool,
    fail_list_workspace_files: bool,
    fail_save_theme_preference: bool,
    fail_save_keybinding_preference: bool,
    fail_remove_keybinding_preference: bool,
    fail_save_ui_preference: bool,
    emit_thread_merged_event: bool,
    block_send_turn: Option<Arc<Notify>>,
}

struct FailingMockEngine {
    state: Mutex<MockEngineState>,
}

impl FailingMockEngine {
    fn with_session(session: HydratedSession) -> Arc<Self> {
        let tid = session.active_thread_id.clone();
        Arc::new(Self {
            state: Mutex::new(MockEngineState {
                session: Some(session),
                agent_options_snapshot: AgentOptionsSnapshot {
                    thread_id: tid,
                    config_options: Vec::new(),
                    available_commands: Vec::new(),
                },
                ..Default::default()
            }),
        })
    }

    fn has_call(&self, prefix: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.starts_with(prefix))
    }

    fn call_count(&self, prefix: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.starts_with(prefix))
            .count()
    }
}

#[async_trait::async_trait]
impl TuiEnginePort for FailingMockEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let mut g = self.state.lock().unwrap();
        g.calls.push("hydrate_session".to_string());
        Ok(g.session.clone().unwrap_or(HydratedSession {
            active_thread_id: "t-main".to_string(),
            threads: vec![HydratedThread {
                id: "t-main".to_string(),
                title: "Main".to_string(),
                custom_name: None,
                turns: Vec::new(),
            }],
        }))
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        let (fail, blocker) = {
            let mut g = self.state.lock().unwrap();
            g.calls.push(format!("send_turn:{thread_id}:{prompt}"));
            (g.fail_send_turn, g.block_send_turn.clone())
        };
        if let Some(notify) = blocker {
            notify.notified().await;
        }
        if fail {
            return Err(io::Error::other("send_turn boom"));
        }
        Ok(())
    }

    async fn fork_thread(&self, thread_id: &str, title: &str) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("fork_thread:{thread_id}:{title}"));
        if g.fail_fork_thread {
            return Err(io::Error::other("fork_thread boom"));
        }
        Ok(format!("{thread_id}-fork"))
    }

    async fn fork_branch(
        &self,
        source_thread_id: &str,
        title: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let fail_branch = {
            let mut g = self.state.lock().unwrap();
            g.calls
                .push(format!("fork_branch:{source_thread_id}:{title:?}"));
            g.fail_fork_branch
        };
        if fail_branch {
            return Err(io::Error::other("fork_branch boom"));
        }
        let id = self
            .fork_thread(source_thread_id, title.unwrap_or(""))
            .await?;
        Ok(HydratedThread {
            id,
            title: title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or("Branch")
                .to_string(),
            custom_name: None,
            turns: Vec::new(),
        })
    }

    async fn merge_thread(&self, source_id: &str, target_id: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("merge_thread:{source_id}:{target_id}"));
        Ok(())
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: &str,
        summary: Option<&str>,
    ) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!(
            "merge_thread_with_summary:{source_id}:{target_id}:{summary:?}"
        ));
        if g.fail_merge_thread_with_summary {
            return Err(io::Error::other("merge boom"));
        }
        if g.emit_thread_merged_event {
            if let Some(tx) = g.event_tx.as_ref() {
                let _ = tx.send(TuiEngineEvent::ThreadMerged {
                    thread_id: source_id.to_string(),
                    target_thread_id: target_id.to_string(),
                    summary: summary.unwrap_or_default().to_string(),
                });
            }
        }
        Ok(())
    }

    async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls
            .push(format!("resolve_permission:{request_id}:{approved}"));
        if g.fail_resolve_permission {
            return Err(io::Error::other("permission boom"));
        }
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut g = self.state.lock().unwrap();
        g.calls.push("subscribe_events".to_string());
        g.event_tx = Some(tx);
        Ok(rx)
    }

    async fn cancel_turn(&self, thread_id: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("cancel_turn:{thread_id}"));
        if g.fail_cancel_turn {
            return Err(io::Error::other("cancel boom"));
        }
        Ok(())
    }

    async fn run_shell_command(&self, thread_id: &str, command: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("run_shell:{thread_id}:{command}"));
        if g.fail_run_shell_command {
            return Err(io::Error::other("shell boom"));
        }
        Ok(())
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, String> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("delete_thread:{thread_id}"));
        if g.fail_delete_thread {
            return Err("delete boom".to_string());
        }
        Ok("t-main".to_string())
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
        let mut g = self.state.lock().unwrap();
        g.calls.push("thread_metadata".to_string());
        if g.fail_thread_metadata {
            return Err("metadata boom".to_string());
        }
        Ok(g.metadata.clone())
    }

    async fn create_root(&self, title: Option<&str>) -> io::Result<HydratedThread> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("create_root:{title:?}"));
        if g.fail_create_root {
            return Err(io::Error::other("create_root boom"));
        }
        Ok(HydratedThread {
            id: "t-new-root".to_string(),
            title: title.unwrap_or("New Root").to_string(),
            custom_name: None,
            turns: Vec::new(),
        })
    }

    async fn create_root_in_cwd(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let fail_cwd = {
            let mut g = self.state.lock().unwrap();
            g.calls.push(format!("create_root_in_cwd:{title:?}:{cwd:?}"));
            g.fail_create_root_in_cwd
        };
        if fail_cwd {
            return Err(io::Error::other("create_root_in_cwd boom"));
        }
        self.create_root(title).await
    }

    async fn rename_thread(&self, thread_id: &str, title: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("rename_thread:{thread_id}:{title}"));
        if g.fail_rename_thread {
            return Err(io::Error::other("rename boom"));
        }
        Ok(())
    }

    async fn agent_options(&self, thread_id: &str) -> io::Result<AgentOptionsSnapshot> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("agent_options:{thread_id}"));
        if g.fail_agent_options {
            return Err(io::Error::other("agent_options boom"));
        }
        let mut snap = g.agent_options_snapshot.clone();
        if snap.thread_id.is_empty() {
            snap.thread_id = thread_id.to_string();
        }
        Ok(snap)
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> io::Result<AgentOptionsSnapshot> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!(
            "set_agent_config:{thread_id}:{config_id}:{value}:{set_default}"
        ));
        g.set_config_calls.push((
            thread_id.to_string(),
            config_id.to_string(),
            value.to_string(),
            set_default,
        ));
        if g.fail_set_agent_config {
            return Err(io::Error::other("set_agent_config boom"));
        }
        Ok(g.set_config_snapshot.clone().unwrap_or_default())
    }

    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        _title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> io::Result<HydratedThread> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!(
            "create_terminal:{parent_thread_id:?}:{cwd:?}:{rows}x{cols}"
        ));
        if g.fail_create_terminal {
            return Err(io::Error::other("create_terminal boom"));
        }
        Ok(HydratedThread {
            id: "t-term-created".to_string(),
            title: "Terminal".to_string(),
            custom_name: None,
            turns: Vec::new(),
        })
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("pty_write:{thread_id}:{}", bytes.len()));
        if g.fail_pty_write {
            return Err(io::Error::other("pty_write boom"));
        }
        Ok(())
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("pty_resize:{thread_id}:{rows}x{cols}"));
        g.pty_resizes.push((thread_id.to_string(), rows, cols));
        if g.fail_pty_resize {
            return Err(io::Error::other("pty_resize boom"));
        }
        Ok(())
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> io::Result<PtyScreenSnapshot> {
        let mut g = self.state.lock().unwrap();
        g.calls
            .push(format!("pty_snapshot:{thread_id}:{rows}x{cols}"));
        if g.fail_pty_snapshot {
            return Err(io::Error::other("pty_snapshot boom"));
        }
        Ok(g.pty_snapshot_data
            .clone()
            .unwrap_or_else(|| make_pty_snapshot("PTY_READY", rows, cols, false)))
    }

    async fn pty_scroll(&self, thread_id: &str, delta: isize) -> io::Result<PtyScreenSnapshot> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("pty_scroll:{thread_id}:{delta}"));
        if g.fail_pty_scroll {
            return Err(io::Error::other("pty_scroll boom"));
        }
        Ok(make_pty_snapshot("PTY_SCROLLED", 20, 80, false))
    }

    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> io::Result<PtyScreenSnapshot> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("pty_restart:{thread_id}:{rows}x{cols}"));
        if g.fail_pty_restart {
            return Err(io::Error::other("pty_restart boom"));
        }
        Ok(make_pty_snapshot("PTY_RESTARTED", rows, cols, false))
    }

    async fn ensure_thread_worktree(&self, thread_id: &str) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("ensure_thread_worktree:{thread_id}"));
        if g.fail_ensure_thread_worktree {
            return Err(io::Error::other("ensure_worktree boom"));
        }
        Ok("/repos/alpha/.grein/worktrees/new".to_string())
    }

    async fn list_worktrees(&self) -> io::Result<Vec<WorktreeEntry>> {
        let mut g = self.state.lock().unwrap();
        g.calls.push("list_worktrees".to_string());
        if g.fail_list_worktrees {
            return Err(io::Error::other("list_worktrees boom"));
        }
        Ok(g.worktrees.clone())
    }

    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.calls
            .push(format!("switch_thread_worktree:{thread_id}:{target_path}"));
        if g.fail_switch_thread_worktree {
            return Err(io::Error::other("switch_worktree boom"));
        }
        Ok(target_path.to_string())
    }

    async fn remove_worktree(&self, path: &str, force: bool) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("remove_worktree:{path}:{force}"));
        if g.fail_remove_worktree {
            return Err(io::Error::other("remove_worktree boom"));
        }
        Ok(())
    }

    async fn worktree_diff_preview(&self, path: &str) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("worktree_diff_preview:{path}"));
        if g.fail_worktree_diff_preview {
            return Err(io::Error::other("diff_preview boom"));
        }
        Ok(String::new())
    }

    async fn prune_worktrees(&self) -> io::Result<String> {
        let mut g = self.state.lock().unwrap();
        g.calls.push("prune_worktrees".to_string());
        if g.fail_prune_worktrees {
            return Err(io::Error::other("prune_worktrees boom"));
        }
        Ok("Pruned 0".to_string())
    }

    async fn load_preferences(&self) -> io::Result<UserPreferencesSnapshot> {
        let mut g = self.state.lock().unwrap();
        g.calls.push("load_preferences".to_string());
        if g.fail_load_preferences {
            return Err(io::Error::other("prefs boom"));
        }
        Ok(UserPreferencesSnapshot::default())
    }

    async fn save_theme_preference(&self, theme: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("save_theme:{theme}"));
        if g.fail_save_theme_preference {
            return Err(io::Error::other("save_theme boom"));
        }
        Ok(())
    }

    async fn save_keybinding_preference(&self, action: &str, binding: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("save_keybinding:{action}:{binding}"));
        if g.fail_save_keybinding_preference {
            return Err(io::Error::other("save_keybinding boom"));
        }
        Ok(())
    }

    async fn remove_keybinding_preference(&self, action: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("remove_keybinding:{action}"));
        if g.fail_remove_keybinding_preference {
            return Err(io::Error::other("remove_keybinding boom"));
        }
        Ok(())
    }

    async fn save_ui_preference(&self, key: &str, value: &str) -> io::Result<()> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("save_ui:{key}:{value}"));
        if g.fail_save_ui_preference {
            return Err(io::Error::other("save_ui boom"));
        }
        Ok(())
    }

    async fn list_recent_user_prompts(&self, limit: usize) -> io::Result<Vec<String>> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("list_recent_user_prompts:{limit}"));
        if g.fail_list_recent_user_prompts {
            return Err(io::Error::other("prompts boom"));
        }
        Ok(g.recent_prompts.clone())
    }

    async fn list_workspace_files(&self, root: Option<&str>) -> io::Result<Vec<String>> {
        let mut g = self.state.lock().unwrap();
        g.calls.push(format!("list_workspace_files:{root:?}"));
        if g.fail_list_workspace_files {
            return Err(io::Error::other("files boom"));
        }
        Ok(g.workspace_files.clone())
    }
}

fn make_pty_snapshot(line: &str, rows: u16, cols: u16, exited: bool) -> PtyScreenSnapshot {
    let mut row_cells = Vec::new();
    for ch in line.chars() {
        row_cells.push(PtyCell {
            contents: ch.to_string(),
            ..Default::default()
        });
    }
    PtyScreenSnapshot {
        rows,
        cols,
        cells: vec![row_cells],
        exited,
        exit_code: exited.then_some(1),
        ..Default::default()
    }
}

fn single_thread_session(id: &str, title: &str) -> HydratedSession {
    HydratedSession {
        active_thread_id: id.to_string(),
        threads: vec![HydratedThread {
            id: id.to_string(),
            title: title.to_string(),
            custom_name: None,
            turns: Vec::new(),
        }],
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

fn send_key(tx: &mpsc::UnboundedSender<Event>, ev: KeyEvent) {
    tx.send(Event::Key(ev)).unwrap();
}

fn send_text(tx: &mpsc::UnboundedSender<Event>, text: &str) {
    for ch in text.chars() {
        send_key(tx, key(KeyCode::Char(ch)));
    }
}

fn submit_line(tx: &mpsc::UnboundedSender<Event>, line: &str) {
    send_text(tx, line);
    send_key(tx, key(KeyCode::Enter));
}

async fn wait_for<F>(desc: &str, mut cond: F)
where
    F: FnMut() -> bool,
{
    for _ in 0..160 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {desc}");
}

fn render_to_string(terminal: &Terminal<TestBackend>) -> String {
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

fn spawn_runner(
    engine: &Arc<FailingMockEngine>,
    width: u16,
    height: u16,
) -> (
    mpsc::UnboundedSender<Event>,
    tokio::task::JoinHandle<io::Result<Terminal<TestBackend>>>,
) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();
    let engine_dyn: Arc<dyn TuiEnginePort> = Arc::clone(engine) as Arc<dyn TuiEnginePort>;
    let handle = tokio::spawn(async move {
        run_with_engine_events(&mut terminal, App::new(), engine_dyn, key_rx).await?;
        Ok(terminal)
    });
    (key_tx, handle)
}

async fn wait_subscribed(engine: &FailingMockEngine) {
    wait_for("subscribed", || engine.state.lock().unwrap().event_tx.is_some()).await;
}

async fn settle_and_finish(
    key_tx: mpsc::UnboundedSender<Event>,
    handle: tokio::task::JoinHandle<io::Result<Terminal<TestBackend>>>,
) -> String {
    let _ = key_tx.send(Event::FocusGained);
    tokio::time::sleep(Duration::from_millis(65)).await;
    drop(key_tx);
    let terminal = timeout(Duration::from_secs(3), handle)
        .await
        .expect("runner finish timeout")
        .unwrap()
        .unwrap();
    render_to_string(&terminal)
}

#[tokio::test]
async fn test_startup_load_preferences_and_metadata_failures_set_status_notices() {
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_load_preferences = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    wait_for("load_preferences called", || engine.has_call("load_preferences")).await;

    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Preferences load failed: prefs boom"),
        "expected startup preferences error notice:\n{screen}"
    );

    let engine_meta = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine_meta.state.lock().unwrap().fail_thread_metadata = true;
    let (key_tx_m, handle_m) = spawn_runner(&engine_meta, 160, 30);
    wait_subscribed(&engine_meta).await;
    wait_for("thread_metadata called", || {
        engine_meta.has_call("thread_metadata")
    })
    .await;

    let screen_m = settle_and_finish(key_tx_m, handle_m).await;
    assert!(
        screen_m.contains("Metadata sync failed: metadata boom"),
        "expected startup metadata error notice:\n{screen_m}"
    );
}

#[tokio::test]
async fn test_send_turn_shell_fork_and_create_root_failures_recover_cleanly() {
    // 1. send_turn failure -> err_rx sets status_notice + transcript error; subsequent send succeeds
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_send_turn = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;

    submit_line(&key_tx, "hello fail");
    wait_for("send_turn called", || engine.has_call("send_turn:t-1:hello fail")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: send_turn boom"), "{screen}");

    // 2. run_shell_command failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_run_shell_command = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "!ls -la");
    wait_for("run_shell called", || engine.has_call("run_shell:t-1:ls -la")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: shell boom"), "{screen}");

    // 3. fork_branch failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_fork_branch = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/branch broken-fork");
    wait_for("fork_branch called", || engine.has_call("fork_branch:t-1:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: fork_branch boom"), "{screen}");
    assert!(!screen.contains("broken-fork"), "{screen}");

    // 4. create_root failure (delegated via create_root_in_cwd)
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_create_root = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/new broken-root");
    wait_for("create_root called", || engine.has_call("create_root:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: create_root boom"), "{screen}");

    // 5. create_root_in_cwd direct failure from Worktrees sidebar ('n')
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.fail_create_root_in_cwd = true;
        g.worktrees.push(WorktreeEntry {
            repo_root: "/repos/alpha".to_string(),
            repo_name: "alpha".to_string(),
            path: "/repos/alpha/.grein/worktrees/wt-1".to_string(),
            branch: Some("feat/wt-1".to_string()),
            head_short: Some("abc1234".to_string()),
            is_primary: false,
            is_detached: false,
            is_grein_managed: true,
            dirty_count: 0,
            attached_thread_ids: Vec::new(),
        });
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    send_key(&key_tx, ctrl('g'));
    send_key(&key_tx, key(KeyCode::Tab));
    wait_for("worktrees listed", || engine.has_call("list_worktrees")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Down));
    send_key(&key_tx, key(KeyCode::Char('n')));
    wait_for("create_root_in_cwd called", || {
        engine.has_call("create_root_in_cwd:")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Error: create_root_in_cwd boom"),
        "expected create_root_in_cwd failure:\n{screen}"
    );
}

#[tokio::test]
async fn test_merge_failure_aborts_and_merge_fallback_completes_when_event_omitted() {
    let make_merge_engine = |fail_merge: bool| {
        let session = HydratedSession {
            active_thread_id: "t-child".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-main".to_string(),
                    title: "Main".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-child".to_string(),
                    title: "Child".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                },
            ],
        };
        let eng = FailingMockEngine::with_session(session);
        {
            let mut g = eng.state.lock().unwrap();
            g.metadata = vec![
                ThreadMeta {
                    thread_id: "t-main".to_string(),
                    parent_thread_id: None,
                    kind: ThreadKind::Agent,
                    ..Default::default()
                },
                ThreadMeta {
                    thread_id: "t-child".to_string(),
                    parent_thread_id: Some("t-main".to_string()),
                    kind: ThreadKind::Agent,
                    ..Default::default()
                },
            ];
            g.fail_merge_thread_with_summary = fail_merge;
            g.emit_thread_merged_event = false;
        }
        eng
    };

    // Sub-run A: Isolated MergeAborted verifies Error notice and unmerged state on screen
    let abort_engine = make_merge_engine(true);
    let (key_tx_a, handle_a) = spawn_runner(&abort_engine, 160, 30);
    wait_subscribed(&abort_engine).await;
    wait_for("metadata loaded", || abort_engine.has_call("thread_metadata")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    submit_line(&key_tx_a, "/join abort only");
    wait_for("merge attempt", || {
        abort_engine.call_count("merge_thread_with_summary:") >= 1
    })
    .await;
    let abort_screen = settle_and_finish(key_tx_a, handle_a).await;
    assert!(abort_screen.contains("Error: merge boom"), "{abort_screen}");
    assert!(!abort_screen.contains("[joined]"), "{abort_screen}");

    // Sub-run B: MergeAborted followed by MergeFallback recovery
    let engine = make_merge_engine(true);

    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    wait_for("metadata loaded", || engine.has_call("thread_metadata")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Step 1: Failing merge triggers MergeAborted + err_rx without marking [joined]
    submit_line(&key_tx, "/join abort test");
    wait_for("merge attempt 1", || {
        engine.call_count("merge_thread_with_summary:") >= 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Step 2: Recover and succeed WITHOUT emitting ThreadMerged -> MergeFallback synthesizes join
    {
        let mut g = engine.state.lock().unwrap();
        g.fail_merge_thread_with_summary = false;
        // Clear post-merge metadata response so only MergeFallback can mark [joined].
        g.metadata.clear();
    }
    submit_line(&key_tx, "/join fallback summary");
    wait_for("merge attempt 2", || {
        engine.call_count("merge_thread_with_summary:") >= 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Switch to Main to inspect the fallback-synthesized blockquote in transcript
    send_key(&key_tx, ctrl('g'));
    send_key(&key_tx, key(KeyCode::Up));
    send_key(&key_tx, key(KeyCode::Enter));

    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("[joined]"),
        "MergeFallback must mark child [joined]:\n{screen}"
    );
    assert!(
        screen.contains("Joined from Child: fallback summary"),
        "MergeFallback must append join summary to Main:\n{screen}"
    );
    assert!(
        screen.contains("Joined Child → Main"),
        "MergeFallback must set joined status notice:\n{screen}"
    );
    assert!(
        !screen.contains("abort test"),
        "Aborted merge must not record aborted summary:\n{screen}"
    );
}

#[tokio::test]
async fn test_cancel_turn_permission_delete_and_rename_failures() {
    // 1. cancel_turn failure while send_turn is in-flight
    let notify = Arc::new(Notify::new());
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.block_send_turn = Some(Arc::clone(&notify));
        g.fail_cancel_turn = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "slow prompt");
    wait_for("send_turn started", || engine.has_call("send_turn:t-1:slow prompt")).await;
    send_key(&key_tx, key(KeyCode::Esc));
    wait_for("cancel_turn called", || engine.has_call("cancel_turn:t-1")).await;
    notify.notify_waiters();
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: cancel boom"), "{screen}");

    // 2. resolve_permission failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_resolve_permission = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    engine
        .state
        .lock()
        .unwrap()
        .event_tx
        .as_ref()
        .unwrap()
        .send(TuiEngineEvent::PermissionRequested {
            request_id: "perm-fail-1".to_string(),
            description: "Allow risky action?".to_string(),
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Char('y')));
    wait_for("resolve_permission called", || {
        engine.has_call("resolve_permission:perm-fail-1:true")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: permission boom"), "{screen}");
    assert!(!screen.contains("Allow risky action?"), "{screen}");

    // 3. delete_thread failure
    let session_two = HydratedSession {
        active_thread_id: "t-2".to_string(),
        threads: vec![
            HydratedThread {
                id: "t-1".to_string(),
                title: "Main".to_string(),
                custom_name: None,
                turns: Vec::new(),
            },
            HydratedThread {
                id: "t-2".to_string(),
                title: "KeepMe".to_string(),
                custom_name: None,
                turns: Vec::new(),
            },
        ],
    };
    let engine = FailingMockEngine::with_session(session_two);
    engine.state.lock().unwrap().fail_delete_thread = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/delete t-2");
    wait_for("delete_thread called", || engine.has_call("delete_thread:t-2")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: delete boom"), "{screen}");
    assert!(screen.contains("KeepMe"), "{screen}");

    // 4. rename_thread failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "OrigTitle"));
    engine.state.lock().unwrap().fail_rename_thread = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/rename RejectedName");
    wait_for("rename_thread called", || {
        engine.has_call("rename_thread:t-1:RejectedName")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(screen.contains("Error: rename boom"), "{screen}");
    assert!(screen.contains("OrigTitle"), "{screen}");
}

#[tokio::test]
async fn test_agent_options_failures_query_resolution_and_empty_config_applied_fallback() {
    // 1. agent_options failures across picker, query, and commands modes
    for (cmd, expected_notice) in [
        ("/model", "Failed to load options: agent_options boom"),
        ("/model gpt", "Failed to load options: agent_options boom"),
        ("/commands", "Failed to load commands: agent_options boom"),
    ] {
        let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
        engine.state.lock().unwrap().fail_agent_options = true;
        let (key_tx, handle) = spawn_runner(&engine, 160, 30);
        wait_subscribed(&engine).await;
        submit_line(&key_tx, cmd);
        wait_for("agent_options called", || engine.has_call("agent_options:t-1")).await;
        let screen = settle_and_finish(key_tx, handle).await;
        assert!(
            screen.contains(expected_notice),
            "cmd `{cmd}` expected `{expected_notice}`:\n{screen}"
        );
    }

    // 2. ResolveConfigQuery: no-match & not-exposed
    let model_opt = AgentConfigOption {
        id: "model".to_string(),
        name: "Model".to_string(),
        category: Some("model".to_string()),
        current_value: "gpt-4o".to_string(),
        options: vec![
            AgentConfigChoice {
                value: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                description: None,
            },
            AgentConfigChoice {
                value: "claude-sonnet".to_string(),
                name: "Claude Sonnet".to_string(),
                description: None,
            },
        ],
    };

    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().agent_options_snapshot = AgentOptionsSnapshot {
        thread_id: "t-1".to_string(),
        config_options: vec![model_opt.clone()],
        available_commands: Vec::new(),
    };
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/model zzz-no-match");
    wait_for("agent_options called for no-match", || {
        engine.has_call("agent_options:t-1")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("No Model option matching 'zzz-no-match'"),
        "{screen}"
    );
    assert!(engine.state.lock().unwrap().set_config_calls.is_empty());

    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().agent_options_snapshot = AgentOptionsSnapshot {
        thread_id: "t-1".to_string(),
        config_options: vec![model_opt.clone()],
        available_commands: Vec::new(),
    };
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/thinking high");
    wait_for("agent_options called for not-exposed", || {
        engine.has_call("agent_options:t-1")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Option 'thought_level' not exposed by agent"),
        "{screen}"
    );
    assert!(engine.state.lock().unwrap().set_config_calls.is_empty());

    // 3. ResolveConfigQuery with empty options (freeform) + set_agent_config failure
    let freeform_opt = AgentConfigOption {
        id: "model".to_string(),
        name: "Model".to_string(),
        category: Some("model".to_string()),
        current_value: "gpt-4o".to_string(),
        options: Vec::new(),
    };
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.agent_options_snapshot = AgentOptionsSnapshot {
            thread_id: "t-1".to_string(),
            config_options: vec![freeform_opt],
            available_commands: Vec::new(),
        };
        g.fail_set_agent_config = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/model custom-freeform");
    wait_for("set_agent_config called for freeform", || {
        engine.has_call("set_agent_config:t-1:model:custom-freeform:false")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Config update failed: set_agent_config boom"),
        "{screen}"
    );

    // 4. ConfigApplied empty-snapshot fallback updates cached config value, status bar & notice
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.agent_options_snapshot = AgentOptionsSnapshot {
            thread_id: "t-1".to_string(),
            config_options: vec![model_opt],
            available_commands: Vec::new(),
        };
        g.set_config_snapshot = Some(AgentOptionsSnapshot {
            thread_id: String::new(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        });
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/model default claude-sonnet");
    wait_for("set_agent_config called for fallback", || {
        engine.has_call("set_agent_config:t-1:model:claude-sonnet:true")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Set model = claude-sonnet (default)"),
        "expected ConfigApplied notice:\n{screen}"
    );
    assert!(
        screen.contains("claude-sonnet"),
        "expected empty-snapshot fallback to sync model onto status bar:\n{screen}"
    );
}

#[tokio::test]
async fn test_pty_write_resize_snapshot_scroll_restart_and_create_terminal_failures() {
    // 1. create_terminal failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_create_terminal = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/term");
    wait_for("create_terminal called", || engine.has_call("create_terminal:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Create terminal failed: create_terminal boom"),
        "{screen}"
    );

    let term_session = single_thread_session("t-term", "Terminal");
    let term_meta = vec![ThreadMeta {
        thread_id: "t-term".to_string(),
        kind: ThreadKind::Terminal,
        cwd: Some("/tmp".to_string()),
        ..Default::default()
    }];

    // 2. pty_snapshot failure on terminal boot
    let engine = FailingMockEngine::with_session(term_session.clone());
    {
        let mut g = engine.state.lock().unwrap();
        g.metadata = term_meta.clone();
        g.fail_pty_snapshot = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 120, 26);
    wait_subscribed(&engine).await;
    wait_for("pty_snapshot called", || engine.has_call("pty_snapshot:t-term:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Terminal snapshot failed: pty_snapshot boom"),
        "{screen}"
    );

    // 3. pty_resize failure
    let engine = FailingMockEngine::with_session(term_session.clone());
    {
        let mut g = engine.state.lock().unwrap();
        g.metadata = term_meta.clone();
        g.fail_pty_resize = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 120, 26);
    wait_subscribed(&engine).await;
    wait_for("pty_resize called", || engine.has_call("pty_resize:t-term:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Terminal resize failed: pty_resize boom"),
        "{screen}"
    );

    // 4. pty_write failure inside pty_writer_handle
    let engine = FailingMockEngine::with_session(term_session.clone());
    {
        let mut g = engine.state.lock().unwrap();
        g.metadata = term_meta.clone();
        g.fail_pty_write = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 120, 26);
    wait_subscribed(&engine).await;
    wait_for("pty_snapshot done", || engine.has_call("pty_snapshot:t-term:")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Char('a')));
    wait_for("pty_write called", || engine.has_call("pty_write:t-term:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Terminal input failed: pty_write boom"),
        "{screen}"
    );

    // 5. pty_scroll failure
    let engine = FailingMockEngine::with_session(term_session.clone());
    {
        let mut g = engine.state.lock().unwrap();
        g.metadata = term_meta.clone();
        g.fail_pty_scroll = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 120, 26);
    wait_subscribed(&engine).await;
    wait_for("pty_snapshot done", || engine.has_call("pty_snapshot:t-term:")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, alt('k'));
    wait_for("pty_scroll called", || engine.has_call("pty_scroll:t-term:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Terminal scroll failed: pty_scroll boom"),
        "{screen}"
    );

    // 6. pty_restart failure on exited terminal
    let engine = FailingMockEngine::with_session(term_session);
    {
        let mut g = engine.state.lock().unwrap();
        g.metadata = term_meta;
        g.pty_snapshot_data = Some(make_pty_snapshot("EXITED", 20, 80, true));
        g.fail_pty_restart = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 120, 26);
    wait_subscribed(&engine).await;
    wait_for("pty_snapshot done", || engine.has_call("pty_snapshot:t-term:")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Enter));
    wait_for("pty_restart called", || engine.has_call("pty_restart:t-term:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Terminal restart failed: pty_restart boom"),
        "{screen}"
    );
}

#[tokio::test]
async fn test_worktree_dispatch_failures_and_list_recovery() {
    let sample_wt = WorktreeEntry {
        repo_root: "/repos/alpha".to_string(),
        repo_name: "alpha".to_string(),
        path: "/repos/alpha/.grein/worktrees/feat-1".to_string(),
        branch: Some("feat/1".to_string()),
        head_short: Some("1234567".to_string()),
        is_primary: false,
        is_detached: false,
        is_grein_managed: true,
        dirty_count: 1,
        attached_thread_ids: Vec::new(),
    };

    // 1. list_worktrees failure -> WorktreesListFailed resets in-flight flag so retry succeeds
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.worktrees = vec![sample_wt.clone()];
        g.fail_list_worktrees = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    send_key(&key_tx, ctrl('g'));
    send_key(&key_tx, key(KeyCode::Tab));
    wait_for("first list_worktrees failed", || {
        engine.call_count("list_worktrees") >= 1
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("List worktrees failed: list_worktrees boom"),
        "{screen}"
    );

    // Retry recovery check in live loop (failing first, then 'r' succeeds)
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.worktrees = vec![sample_wt.clone()];
        g.fail_list_worktrees = true;
        g.fail_worktree_diff_preview = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    send_key(&key_tx, ctrl('g'));
    send_key(&key_tx, key(KeyCode::Tab));
    wait_for("first list_worktrees", || engine.call_count("list_worktrees") >= 1).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    engine.state.lock().unwrap().fail_list_worktrees = false;
    send_key(&key_tx, key(KeyCode::Char('r')));
    wait_for("retry list_worktrees", || engine.call_count("list_worktrees") >= 2).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Down));
    wait_for("worktree_diff_preview called", || {
        engine.has_call("worktree_diff_preview:")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("feat/1"),
        "expected recovered worktree list:\n{screen}"
    );
    assert!(
        screen.contains("Diff unavailable: diff_preview boom"),
        "expected WorktreeDiffFailed message:\n{screen}"
    );

    // 2. ensure_thread_worktree failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_ensure_thread_worktree = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/worktree");
    wait_for("ensure_thread_worktree called", || {
        engine.has_call("ensure_thread_worktree:t-1")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Worktree failed: ensure_worktree boom"),
        "{screen}"
    );

    // 3. switch_thread_worktree failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.worktrees = vec![sample_wt.clone()];
        g.fail_switch_thread_worktree = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    send_key(&key_tx, ctrl('g'));
    send_key(&key_tx, key(KeyCode::Tab));
    wait_for("list_worktrees", || engine.has_call("list_worktrees")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Down));
    send_key(&key_tx, key(KeyCode::Enter));
    wait_for("switch_thread_worktree called", || {
        engine.has_call("switch_thread_worktree:t-1:")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Worktree switch failed: switch_worktree boom"),
        "{screen}"
    );

    // 4. remove_worktree failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.worktrees = vec![sample_wt];
        g.fail_remove_worktree = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    send_key(&key_tx, ctrl('g'));
    send_key(&key_tx, key(KeyCode::Tab));
    wait_for("list_worktrees", || engine.has_call("list_worktrees")).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Down));
    send_key(&key_tx, key(KeyCode::Char('d')));
    tokio::time::sleep(Duration::from_millis(20)).await;
    send_key(&key_tx, key(KeyCode::Char('y')));
    wait_for("remove_worktree called", || engine.has_call("remove_worktree:")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Delete worktree failed: remove_worktree boom"),
        "{screen}"
    );
    assert!(
        screen.contains("feat/1"),
        "failed removal must keep worktree in list:\n{screen}"
    );

    // 5. prune_worktrees failure
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    engine.state.lock().unwrap().fail_prune_worktrees = true;
    let (key_tx, handle) = spawn_runner(&engine, 160, 30);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/prune");
    wait_for("prune_worktrees called", || engine.has_call("prune_worktrees")).await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("Prune worktrees failed: prune_worktrees boom"),
        "{screen}"
    );
}

#[tokio::test]
async fn test_fetch_autocomplete_context_failures_degrade_gracefully() {
    // Both sources fail -> zero error notice, zero crash
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.fail_list_recent_user_prompts = true;
        g.fail_list_workspace_files = true;
    }
    let (key_tx, handle) = spawn_runner(&engine, 140, 28);
    wait_subscribed(&engine).await;
    wait_for("both autocomplete calls executed", || {
        engine.has_call("list_recent_user_prompts:") && engine.has_call("list_workspace_files:")
    })
    .await;
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(!screen.contains("Error:"), "{screen}");
    assert!(!screen.contains("boom"), "{screen}");

    // Partial failure: prompts fail, files succeed -> @file completion still works
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.fail_list_recent_user_prompts = true;
        g.workspace_files = vec!["crates/grein-tui/src/runner/dispatch.rs".to_string()];
    }
    let (key_tx, handle) = spawn_runner(&engine, 140, 28);
    wait_subscribed(&engine).await;
    wait_for("autocomplete fetched", || {
        engine.has_call("list_recent_user_prompts:") && engine.has_call("list_workspace_files:")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_text(&key_tx, "@disp");
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Tab));
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("@dispatch.rs"),
        "expected workspace file autocomplete despite prompt fetch failure:\n{screen}"
    );

    // Partial failure: files fail, prompts succeed -> prompt completion still works
    let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
    {
        let mut g = engine.state.lock().unwrap();
        g.fail_list_workspace_files = true;
        g.recent_prompts = vec!["audit dispatch recovery".to_string()];
    }
    let (key_tx, handle) = spawn_runner(&engine, 140, 28);
    wait_subscribed(&engine).await;
    wait_for("autocomplete fetched", || {
        engine.has_call("list_recent_user_prompts:") && engine.has_call("list_workspace_files:")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_text(&key_tx, "aud");
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_key(&key_tx, key(KeyCode::Tab));
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(
        screen.contains("audit dispatch recovery"),
        "expected prompt history autocomplete despite file fetch failure:\n{screen}"
    );
}

#[tokio::test]
async fn test_preference_save_and_remove_failures_surface_status_notices() {
    let cases = [
        ("/theme dracula", "save_theme:dracula", "Save theme failed: save_theme boom"),
        (
            "/bind help F1",
            "save_keybinding:help:f1",
            "Save keybinding failed: save_keybinding boom",
        ),
        (
            "/unbind help",
            "remove_keybinding:help",
            "Remove keybinding failed: remove_keybinding boom",
        ),
        (
            "/autocomplete off",
            "save_ui:composer_autocomplete:off",
            "Save UI preference failed: save_ui boom",
        ),
    ];

    for (cmd, expected_call, expected_notice) in cases {
        let engine = FailingMockEngine::with_session(single_thread_session("t-1", "Main"));
        {
            let mut g = engine.state.lock().unwrap();
            g.fail_save_theme_preference = true;
            g.fail_save_keybinding_preference = true;
            g.fail_remove_keybinding_preference = true;
            g.fail_save_ui_preference = true;
        }
        let (key_tx, handle) = spawn_runner(&engine, 160, 30);
        wait_subscribed(&engine).await;
        submit_line(&key_tx, cmd);
        wait_for("preference call", || engine.has_call(expected_call)).await;
        let screen = settle_and_finish(key_tx, handle).await;
        assert!(
            screen.contains(expected_notice),
            "cmd `{cmd}` expected `{expected_notice}`:\n{screen}"
        );
    }
}

#[tokio::test]
async fn test_event_resize_updates_frame_area_and_queues_pty_resize_for_terminal() {
    // 1. Direct handle_event coverage on Agent vs Terminal threads
    let mut agent_app = App::new();
    apply_hydrated_session(&mut agent_app, &single_thread_session("t-agent", "Agent"));
    assert!(!handle_event(&mut agent_app, Event::Resize(130, 38)).unwrap());
    assert_eq!(agent_app.last_frame_area, Some(Rect::new(0, 0, 130, 38)));
    assert!(
        agent_app.pending_actions.is_empty(),
        "Resize on non-terminal thread must not queue PtyResize"
    );

    let mut term_app = App::new();
    apply_hydrated_session(&mut term_app, &single_thread_session("t-term", "Terminal"));
    apply_engine_event(
        &mut term_app,
        TuiEngineEvent::ThreadMetadataUpdated {
            meta: vec![ThreadMeta {
                thread_id: "t-term".to_string(),
                kind: ThreadKind::Terminal,
                ..Default::default()
            }],
        },
    );
    term_app.pending_actions.clear();

    assert!(!handle_event(&mut term_app, Event::Resize(140, 42)).unwrap());
    assert_eq!(term_app.last_frame_area, Some(Rect::new(0, 0, 140, 42)));
    assert!(term_app.terminal_pane_size.0 >= 1 && term_app.terminal_pane_size.1 >= 1);
    let first_resize = term_app.pending_actions.pop_front();
    assert!(matches!(
        first_resize,
        Some(AppAction::PtyResize { ref thread_id, rows, cols })
            if thread_id == "t-term" && rows > 20 && cols > 80
    ));

    // Duplicate same-size resize does not re-queue PtyResize
    handle_event(&mut term_app, Event::Resize(140, 42)).unwrap();
    assert!(term_app.pending_actions.is_empty());

    // Different size re-queues PtyResize with smaller dimensions
    handle_event(&mut term_app, Event::Resize(90, 22)).unwrap();
    let second_resize = term_app.pending_actions.pop_front();
    assert!(matches!(
        second_resize,
        Some(AppAction::PtyResize { ref thread_id, rows, cols })
            if thread_id == "t-term" && rows < 40 && cols < 90
    ));

    // 2. Live runner loop coverage: Event::Resize over key_rx dispatches engine.pty_resize
    let engine = FailingMockEngine::with_session(single_thread_session("t-term", "Terminal"));
    engine.state.lock().unwrap().metadata = vec![ThreadMeta {
        thread_id: "t-term".to_string(),
        kind: ThreadKind::Terminal,
        ..Default::default()
    }];
    let (key_tx, handle) = spawn_runner(&engine, 100, 24);
    wait_subscribed(&engine).await;
    wait_for("initial pty_resize at 100x24", || {
        !engine.state.lock().unwrap().pty_resizes.is_empty()
    })
    .await;

    key_tx.send(Event::Resize(160, 48)).unwrap();
    wait_for("resized pty_resize for 160x48", || {
        engine
            .state
            .lock()
            .unwrap()
            .pty_resizes
            .iter()
            .any(|(id, rows, cols)| id == "t-term" && *rows >= 40 && *cols >= 100)
    })
    .await;
    let _ = settle_and_finish(key_tx, handle).await;
}

#[tokio::test]
async fn test_open_config_picker_live_update_delete_close_and_thread_merged_edges() {
    // 1. ThreadDeleted closes ConfigPickerModal only when targeting the deleted thread
    let mut app = App::new();
    apply_hydrated_session(
        &mut app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-1".to_string(),
                    title: "One".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-2".to_string(),
                    title: "Two".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                },
            ],
        },
    );
    app.config_picker = Some(ConfigPickerModal {
        thread_id: "t-1".to_string(),
        config_id: "model".to_string(),
        title: "Model".to_string(),
        current_value: "a".to_string(),
        options: vec![AgentConfigChoice {
            value: "a".to_string(),
            name: "A".to_string(),
            description: None,
        }],
        selected_index: 0,
        set_default: false,
    });

    // Deleting unrelated thread keeps modal open
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadDeleted {
            thread_id: "t-2".to_string(),
            next_active_thread_id: "t-1".to_string(),
        },
    );
    assert!(app.config_picker.is_some());

    // Deleting target thread closes modal
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ThreadDeleted {
            thread_id: "t-1".to_string(),
            next_active_thread_id: String::new(),
        },
    );
    assert!(app.config_picker.is_none());

    // 2. AgentConfigUpdated live-refreshes open ConfigPickerModal and clamps selected_index
    let mut modal_app = App::new();
    apply_hydrated_session(&mut modal_app, &single_thread_session("t-1", "One"));
    modal_app.config_picker = Some(ConfigPickerModal {
        thread_id: "t-1".to_string(),
        config_id: "model".to_string(),
        title: "Model".to_string(),
        current_value: "m1".to_string(),
        options: vec![
            AgentConfigChoice {
                value: "m1".to_string(),
                name: "M1".to_string(),
                description: None,
            },
            AgentConfigChoice {
                value: "m2".to_string(),
                name: "M2".to_string(),
                description: None,
            },
            AgentConfigChoice {
                value: "m3".to_string(),
                name: "M3".to_string(),
                description: None,
            },
        ],
        selected_index: 2,
        set_default: false,
    });

    // Update for different thread does not touch modal
    apply_engine_event(
        &mut modal_app,
        TuiEngineEvent::AgentConfigUpdated {
            thread_id: "t-other".to_string(),
            config_options: vec![AgentConfigOption {
                id: "model".to_string(),
                name: "Model".to_string(),
                category: Some("model".to_string()),
                current_value: "other".to_string(),
                options: vec![AgentConfigChoice {
                    value: "other".to_string(),
                    name: "Other".to_string(),
                    description: None,
                }],
            }],
        },
    );
    assert_eq!(modal_app.config_picker.as_ref().unwrap().current_value, "m1");

    // Update for t-1 with fewer choices updates current_value, options, and clamps selected_index
    apply_engine_event(
        &mut modal_app,
        TuiEngineEvent::AgentConfigUpdated {
            thread_id: "t-1".to_string(),
            config_options: vec![AgentConfigOption {
                id: "model".to_string(),
                name: "Model".to_string(),
                category: Some("model".to_string()),
                current_value: "n2".to_string(),
                options: vec![
                    AgentConfigChoice {
                        value: "n1".to_string(),
                        name: "N1".to_string(),
                        description: None,
                    },
                    AgentConfigChoice {
                        value: "n2".to_string(),
                        name: "N2".to_string(),
                        description: None,
                    },
                ],
            }],
        },
    );
    let refreshed = modal_app.config_picker.as_ref().unwrap();
    assert_eq!(refreshed.current_value, "n2");
    assert_eq!(refreshed.options.len(), 2);
    assert_eq!(refreshed.selected_index, 1);

    // Update with empty options preserves existing picker choices and updates current_value safely
    apply_engine_event(
        &mut modal_app,
        TuiEngineEvent::AgentConfigUpdated {
            thread_id: "t-1".to_string(),
            config_options: vec![AgentConfigOption {
                id: "model".to_string(),
                name: "Model".to_string(),
                category: Some("model".to_string()),
                current_value: "n1".to_string(),
                options: Vec::new(),
            }],
        },
    );
    let after_empty = modal_app.config_picker.as_ref().unwrap();
    assert_eq!(after_empty.current_value, "n1");
    assert_eq!(after_empty.options.len(), 2);

    // 3. Live runner verification: open modal -> AgentConfigUpdated live-refreshes -> ThreadDeleted closes
    let engine = FailingMockEngine::with_session(HydratedSession {
        active_thread_id: "t-2".to_string(),
        threads: vec![
            HydratedThread {
                id: "t-1".to_string(),
                title: "Main".to_string(),
                custom_name: None,
                turns: Vec::new(),
            },
            HydratedThread {
                id: "t-2".to_string(),
                title: "Branch".to_string(),
                custom_name: None,
                turns: Vec::new(),
            },
        ],
    });
    engine.state.lock().unwrap().agent_options_snapshot = AgentOptionsSnapshot {
        thread_id: "t-2".to_string(),
        config_options: vec![AgentConfigOption {
            id: "model".to_string(),
            name: "Model".to_string(),
            category: Some("model".to_string()),
            current_value: "old-model".to_string(),
            options: vec![AgentConfigChoice {
                value: "old-model".to_string(),
                name: "Old Model".to_string(),
                description: None,
            }],
        }],
        available_commands: Vec::new(),
    };
    let (key_tx, handle) = spawn_runner(&engine, 140, 28);
    wait_subscribed(&engine).await;
    submit_line(&key_tx, "/model");
    wait_for("agent_options for modal", || {
        engine.has_call("agent_options:t-2")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;

    let ev_tx = engine.state.lock().unwrap().event_tx.clone().unwrap();
    ev_tx
        .send(TuiEngineEvent::AgentConfigUpdated {
            thread_id: "t-2".to_string(),
            config_options: vec![AgentConfigOption {
                id: "model".to_string(),
                name: "Model".to_string(),
                category: Some("model".to_string()),
                current_value: "live-refreshed-model".to_string(),
                options: vec![AgentConfigChoice {
                    value: "live-refreshed-model".to_string(),
                    name: "Live Refreshed".to_string(),
                    description: None,
                }],
            }],
        })
        .unwrap();
    let _ = key_tx.send(Event::FocusGained);
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Closing modal via ThreadDeleted leaves screen without modal title
    ev_tx
        .send(TuiEngineEvent::ThreadDeleted {
            thread_id: "t-2".to_string(),
            next_active_thread_id: "t-1".to_string(),
        })
        .unwrap();
    let screen = settle_and_finish(key_tx, handle).await;
    assert!(!screen.contains("Select Model"), "{screen}");
    assert!(!screen.contains("Branch"), "{screen}");

    // 4. ThreadMerged empty-target and self-target edge cases
    let mut merge_app = App::new();
    apply_hydrated_session(
        &mut merge_app,
        &HydratedSession {
            active_thread_id: "t-1".to_string(),
            threads: vec![
                HydratedThread {
                    id: "t-1".to_string(),
                    title: "Alpha".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                },
                HydratedThread {
                    id: "t-2".to_string(),
                    title: "Beta".to_string(),
                    custom_name: None,
                    turns: Vec::new(),
                },
            ],
        },
    );
    apply_engine_event(
        &mut merge_app,
        TuiEngineEvent::ThreadMerged {
            thread_id: "t-1".to_string(),
            target_thread_id: String::new(),
            summary: "ignored".to_string(),
        },
    );
    assert!(merge_app.thread_meta.get("t-1").is_some_and(|m| m.merged));
    assert_eq!(
        merge_app.status_notice.as_deref(),
        Some("Joined Alpha to Shared Context")
    );
    assert!(merge_app.active_transcript_md.is_empty());

    apply_engine_event(
        &mut merge_app,
        TuiEngineEvent::ThreadMerged {
            thread_id: "t-2".to_string(),
            target_thread_id: "t-2".to_string(),
            summary: "self".to_string(),
        },
    );
    assert!(merge_app.thread_meta.get("t-2").is_some_and(|m| m.merged));
    assert_eq!(
        merge_app.status_notice.as_deref(),
        Some("Joined Beta to Shared Context")
    );
    assert!(merge_app
        .thread_transcripts
        .get("t-2")
        .is_some_and(String::is_empty));
}

#[test]
fn test_tree_order_recovers_from_cycles_self_parent_and_missing_parent_orphans() {
    let threads = vec![
        ThreadItem {
            id: "self".to_string(),
            title: "Self".to_string(),
            ..Default::default()
        },
        ThreadItem {
            id: "cyc-a".to_string(),
            title: "Cycle A".to_string(),
            ..Default::default()
        },
        ThreadItem {
            id: "cyc-b".to_string(),
            title: "Cycle B".to_string(),
            ..Default::default()
        },
        ThreadItem {
            id: "cyc-c".to_string(),
            title: "Cycle C".to_string(),
            ..Default::default()
        },
        ThreadItem {
            id: "cyc-leaf".to_string(),
            title: "Cycle Leaf".to_string(),
            ..Default::default()
        },
        ThreadItem {
            id: "orphan".to_string(),
            title: "Orphan".to_string(),
            ..Default::default()
        },
        ThreadItem {
            id: "orphan-child".to_string(),
            title: "Orphan Child".to_string(),
            ..Default::default()
        },
    ];

    let mut meta = HashMap::new();
    let link = |id: &str, parent: &str| {
        (
            id.to_string(),
            ThreadMeta {
                thread_id: id.to_string(),
                parent_thread_id: Some(parent.to_string()),
                ..Default::default()
            },
        )
    };
    meta.extend([
        link("self", "self"),
        link("cyc-a", "cyc-b"),
        link("cyc-b", "cyc-a"),
        link("cyc-c", "cyc-b"),
        link("cyc-leaf", "cyc-c"),
        link("orphan", "missing-parent-id"),
        link("orphan-child", "orphan"),
    ]);

    let ordered = compute_ordered_thread_entries(&threads, &meta);
    let ordered_ids: Vec<(&str, usize, &str)> = ordered
        .iter()
        .map(|e| (threads[e.index].id.as_str(), e.depth, e.prefix.as_str()))
        .collect();
    assert_eq!(
        ordered_ids,
        vec![
            ("self", 0, ""),
            ("orphan", 0, ""),
            ("orphan-child", 1, "└─ "),
            ("cyc-a", 0, ""),
            ("cyc-b", 1, "└─ "),
            ("cyc-c", 2, "   └─ "),
            ("cyc-leaf", 3, "      └─ "),
        ]
    );

    // Folded cycle root counts all reachable cyclic/descendant nodes without infinite loop
    let mut collapsed = BTreeSet::new();
    collapsed.insert("cyc-a".to_string());
    let folded = compute_ordered_thread_entries_with_folds(&threads, &meta, &collapsed);
    let folded_summary: Vec<(&str, usize, bool, usize)> = folded
        .iter()
        .map(|e| {
            (
                threads[e.index].id.as_str(),
                e.depth,
                e.has_children,
                e.hidden_descendants,
            )
        })
        .collect();
    assert_eq!(
        folded_summary,
        vec![
            ("self", 0, false, 0),
            ("orphan", 0, true, 0),
            ("orphan-child", 1, false, 0),
            ("cyc-a", 0, true, 3),
        ]
    );
}
