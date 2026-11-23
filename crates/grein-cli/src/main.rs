use std::io;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use clap::Parser;
use grein_cli::args::{AttachArgs, Cli, Command, DaemonArgs};
use grein_cli::engine::{
    truncate_chars_with_ellipsis, DaemonEngine, InProcessEngine, PermissionDecisionPayload,
    UnixSocketEngineClient,
};
use grein_tui::engine_port::{
    AgentOptionsSnapshot, HydratedSession, HydratedThread, HydratedTurn, PtyScreenSnapshot,
    ThreadMeta, TuiEngineEvent, TuiEnginePort, UserPreferencesSnapshot, WorktreeEntry,
};
use tokio::sync::{mpsc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialSessionMode {
    Terminal,
    Agent,
}

/// Adapts an engine client (any `DaemonEngine`) to the TUI engine port.
struct EngineClientPort<E> {
    client: E,
    initial_mode: InitialSessionMode,
    events_tx: mpsc::UnboundedSender<TuiEngineEvent>,
    events_rx: Mutex<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>>,
    has_live_stream: AtomicBool,
    in_flight_turns: Arc<AtomicUsize>,
    buffered_sync_errors: Arc<StdMutex<Vec<String>>>,
    suppressed_sync_errors: Arc<StdMutex<Vec<(String, Instant)>>>,
}

impl<E> EngineClientPort<E> {
    #[cfg(test)]
    fn new(client: E) -> Self {
        Self::new_with_mode(client, InitialSessionMode::Agent)
    }

    fn new_with_mode(client: E, initial_mode: InitialSessionMode) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            client,
            initial_mode,
            events_tx,
            events_rx: Mutex::new(Some(events_rx)),
            has_live_stream: AtomicBool::new(false),
            in_flight_turns: Arc::new(AtomicUsize::new(0)),
            buffered_sync_errors: Arc::new(StdMutex::new(Vec::new())),
            suppressed_sync_errors: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    fn emit(&self, event: TuiEngineEvent) {
        let _ = self.events_tx.send(event);
    }
}

impl<E: DaemonEngine> EngineClientPort<E> {
    async fn load_hydrated_turns(&self, thread_id: &str) -> io::Result<Vec<HydratedTurn>> {
        let entries = self
            .client
            .get_thread_transcript(thread_id)
            .await
            .map_err(to_io)?;
        Ok(entries
            .into_iter()
            .map(|entry| HydratedTurn {
                id: entry.node_id,
                role: entry.role,
                content: entry.content,
            })
            .collect())
    }

    async fn hydrate_summaries(
        &self,
        summaries: Vec<grein_cli::engine::ThreadSummary>,
    ) -> io::Result<Vec<HydratedThread>> {
        let mut threads = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let turns = self.load_hydrated_turns(&summary.thread_id).await?;
            threads.push(HydratedThread {
                id: summary.thread_id,
                title: summary.title,
                custom_name: summary.custom_name,
                turns,
            });
        }
        Ok(threads)
    }

    async fn fetch_and_emit_thread_metadata(&self) -> Option<Vec<ThreadMeta>> {
        match self.client.thread_metadata().await {
            Ok(meta) => {
                if !meta.is_empty() {
                    self.emit(TuiEngineEvent::ThreadMetadataUpdated { meta: meta.clone() });
                }
                Some(meta)
            }
            Err(err) => {
                self.emit(TuiEngineEvent::Error {
                    message: err.to_string(),
                });
                None
            }
        }
    }
}

fn error_matches_sync(err_msg: &str, event_msg: &str) -> bool {
    !event_msg.is_empty() && (err_msg == event_msg || err_msg.ends_with(event_msg))
}

fn to_io<E: std::fmt::Display>(err: E) -> io::Error {
    io::Error::other(err.to_string())
}

#[async_trait::async_trait]
impl<E> TuiEnginePort for EngineClientPort<E>
where
    E: DaemonEngine + Send + Sync + 'static,
{
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let summaries = self.client.list_threads().await.map_err(to_io)?;
        let (active_thread_id, threads) = if !summaries.is_empty() {
            let threads = self.hydrate_summaries(summaries).await?;
            let active_thread_id = threads
                .first()
                .map(|t| t.id.clone())
                .unwrap_or_default();
            (active_thread_id, threads)
        } else {
            match self.initial_mode {
                InitialSessionMode::Terminal => {
                    let term = self
                        .client
                        .create_terminal(None, None, None, 24, 80)
                        .await
                        .map_err(to_io)?;
                    let hydrated = HydratedThread {
                        id: term.thread_id.clone(),
                        title: term.title,
                        custom_name: term.custom_name,
                        turns: Vec::new(),
                    };
                    (term.thread_id, vec![hydrated])
                }
                InitialSessionMode::Agent => {
                    let main = self.client.ensure_main_thread().await.map_err(to_io)?;
                    let summaries = self.client.list_threads().await.map_err(to_io)?;
                    let mut threads = self.hydrate_summaries(summaries).await?;
                    if threads.is_empty() {
                        let turns = self.load_hydrated_turns(&main.thread_id).await?;
                        threads.push(HydratedThread {
                            id: main.thread_id.clone(),
                            title: main.title,
                            custom_name: main.custom_name,
                            turns,
                        });
                    }
                    let active_thread_id = threads
                        .first()
                        .map(|t| t.id.clone())
                        .unwrap_or(main.thread_id);
                    (active_thread_id, threads)
                }
            }
        };
        let _ = self.fetch_and_emit_thread_metadata().await;
        if let Ok(agent_metas) = self.client.agent_metadata().await {
            for entry in agent_metas {
                if entry.model.is_some() || entry.mode.is_some() {
                    self.emit(TuiEngineEvent::AgentMetaUpdated {
                        thread_id: entry.thread_id.clone(),
                        model: entry.model,
                        mode: entry.mode,
                    });
                }
                if !entry.config_options.is_empty() {
                    self.emit(TuiEngineEvent::AgentConfigUpdated {
                        thread_id: entry.thread_id.clone(),
                        config_options: entry.config_options,
                    });
                }
                if !entry.available_commands.is_empty() {
                    self.emit(TuiEngineEvent::AgentCommandsUpdated {
                        thread_id: entry.thread_id,
                        commands: entry.available_commands,
                    });
                }
            }
        }
        Ok(HydratedSession {
            active_thread_id,
            threads,
        })
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        if self.has_live_stream.load(Ordering::SeqCst) {
            self.in_flight_turns.fetch_add(1, Ordering::SeqCst);
            let res = self.client.send_turn_for_thread(thread_id, prompt).await;
            if let Err(ref err) = res {
                let err_msg = err.to_string();
                let consumed_from_buffer = {
                    let mut buf = self
                        .buffered_sync_errors
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if let Some(pos) = buf.iter().position(|m| error_matches_sync(&err_msg, m)) {
                        buf.remove(pos);
                        true
                    } else {
                        false
                    }
                };
                if !consumed_from_buffer {
                    let mut sup = self
                        .suppressed_sync_errors
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    sup.push((err_msg, Instant::now()));
                }
            }
            let remaining = self.in_flight_turns.fetch_sub(1, Ordering::SeqCst) - 1;
            if remaining == 0 {
                let leftover: Vec<String> = {
                    let mut buf = self
                        .buffered_sync_errors
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    std::mem::take(&mut *buf)
                };
                for msg in leftover {
                    self.emit(TuiEngineEvent::Error { message: msg });
                }
            }
            res.map(|_| ()).map_err(to_io)?;
            return Ok(());
        }

        self.emit(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "streaming".to_string(),
        });
        self.client
            .send_turn_for_thread(thread_id, prompt)
            .await
            .map_err(to_io)?;
        if let Some(last) = self
            .client
            .get_thread_transcript(thread_id)
            .await
            .map_err(to_io)?
            .pop()
        {
            if !last.role.eq_ignore_ascii_case("user") {
                self.emit(TuiEngineEvent::StreamChunk {
                    thread_id: thread_id.to_string(),
                    content: last.content,
                });
            }
        }
        self.emit(TuiEngineEvent::TurnCompleted {
            thread_id: thread_id.to_string(),
        });
        self.emit(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "idle".to_string(),
        });
        Ok(())
    }

    async fn fork_thread(&self, thread_id: &str, anchor_id: &str) -> io::Result<String> {
        let trimmed_anchor = anchor_id.trim();
        let (source, title) = if trimmed_anchor.is_empty() {
            (Some(thread_id), None)
        } else if trimmed_anchor.len() == 36
            && trimmed_anchor
                .as_bytes()
                .iter()
                .enumerate()
                .all(|(i, b)| match i {
                    8 | 13 | 18 | 23 => *b == b'-',
                    _ => b.is_ascii_hexdigit(),
                })
        {
            (Some(trimmed_anchor), None)
        } else {
            (Some(thread_id), Some(trimmed_anchor))
        };
        let forked = self
            .client
            .fork_thread_with_title(source, title)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let turns = self.load_hydrated_turns(&forked.thread_id).await?;
            self.emit(TuiEngineEvent::ThreadCreated {
                thread: HydratedThread {
                    id: forked.thread_id.clone(),
                    title: forked.title,
                    custom_name: forked.custom_name,
                    turns,
                },
            });
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(forked.thread_id)
    }

    async fn fork_branch(
        &self,
        source_thread_id: &str,
        title: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let forked = self
            .client
            .fork_thread_with_title(Some(source_thread_id), title)
            .await
            .map_err(to_io)?;
        let turns = self.load_hydrated_turns(&forked.thread_id).await?;
        let hydrated = HydratedThread {
            id: forked.thread_id,
            title: forked.title,
            custom_name: forked.custom_name,
            turns,
        };
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ThreadCreated {
                thread: hydrated.clone(),
            });
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(hydrated)
    }

    async fn create_root(&self, title: Option<&str>) -> io::Result<HydratedThread> {
        let created = self.client.create_root_thread(title).await.map_err(to_io)?;
        let hydrated = HydratedThread {
            id: created.thread_id,
            title: created.title,
            custom_name: created.custom_name,
            turns: Vec::new(),
        };
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ThreadCreated {
                thread: hydrated.clone(),
            });
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(hydrated)
    }

    async fn rename_thread(&self, thread_id: &str, title: &str) -> io::Result<()> {
        let renamed = self
            .client
            .rename_thread(thread_id, title)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ThreadCustomNameUpdated {
                thread_id: renamed.thread_id,
                custom_name: renamed.custom_name,
            });
        }
        Ok(())
    }

    async fn merge_thread(&self, source_id: &str, target_id: &str) -> io::Result<()> {
        self.merge_thread_with_summary(source_id, target_id, None)
            .await
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: &str,
        summary: Option<&str>,
    ) -> io::Result<()> {
        let target_opt = (!target_id.trim().is_empty()).then_some(target_id.trim());
        let merged = self
            .client
            .merge_thread_with_summary(source_id, target_opt, summary)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let resolved_summary = summary
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|explicit| truncate_chars_with_ellipsis(explicit, 400))
                .unwrap_or_default();
            let meta_opt = self.fetch_and_emit_thread_metadata().await;
            let resolved_target = target_opt
                .filter(|t| *t != merged.thread_id)
                .map(str::to_string)
                .or_else(|| {
                    meta_opt.and_then(|meta| {
                        meta.into_iter()
                            .find(|m| m.thread_id == merged.thread_id)
                            .and_then(|m| m.parent_thread_id)
                    })
                })
                .unwrap_or_default();
            self.emit(TuiEngineEvent::ThreadMerged {
                thread_id: merged.thread_id,
                target_thread_id: resolved_target,
                summary: resolved_summary,
            });
        }
        Ok(())
    }

    async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()> {
        let payload = PermissionDecisionPayload {
            request_id: request_id.to_string(),
            allow: approved,
        };
        self.client
            .resolve_permission(payload)
            .await
            .map_err(to_io)?;
        Ok(())
    }

    async fn cancel_turn(&self, thread_id: &str) -> io::Result<()> {
        self.client.cancel_turn(thread_id).await.map_err(to_io)
    }

    async fn run_shell_command(&self, thread_id: &str, command: &str) -> io::Result<()> {
        let result = self
            .client
            .run_shell_command(thread_id, command)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ShellOutput {
                thread_id: thread_id.to_string(),
                command: result.command,
                stdout: result.stdout,
                stderr: result.stderr,
                exit_code: result.exit_code,
            });
        }
        Ok(())
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, String> {
        self.client
            .delete_thread(thread_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
        self.client.thread_metadata().await.map_err(|e| {
            let msg = e.to_string();
            self.emit(TuiEngineEvent::Error {
                message: msg.clone(),
            });
            msg
        })
    }

    async fn agent_options(&self, thread_id: &str) -> io::Result<AgentOptionsSnapshot> {
        self.client.agent_options(thread_id).await.map_err(to_io)
    }

    async fn set_agent_config(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> io::Result<AgentOptionsSnapshot> {
        let snapshot = self
            .client
            .set_agent_config(thread_id, config_id, value, set_default)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::AgentConfigUpdated {
                thread_id: snapshot.thread_id.clone(),
                config_options: snapshot.config_options.clone(),
            });
        }
        Ok(snapshot)
    }

    async fn create_terminal(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> io::Result<HydratedThread> {
        let created = self
            .client
            .create_terminal(parent_thread_id, title, cwd, rows, cols)
            .await
            .map_err(to_io)?;
        let hydrated = HydratedThread {
            id: created.thread_id,
            title: created.title,
            custom_name: created.custom_name,
            turns: Vec::new(),
        };
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let _ = self.fetch_and_emit_thread_metadata().await;
            self.emit(TuiEngineEvent::ThreadCreated {
                thread: hydrated.clone(),
            });
        }
        Ok(hydrated)
    }

    async fn pty_write(&self, thread_id: &str, bytes: Vec<u8>) -> io::Result<()> {
        self.client.pty_write(thread_id, bytes).await.map_err(to_io)
    }

    async fn pty_resize(&self, thread_id: &str, rows: u16, cols: u16) -> io::Result<()> {
        self.client
            .pty_resize(thread_id, rows, cols)
            .await
            .map_err(to_io)
    }

    async fn pty_snapshot(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> io::Result<PtyScreenSnapshot> {
        self.client
            .pty_snapshot(thread_id, rows, cols)
            .await
            .map_err(to_io)
    }

    async fn ensure_thread_worktree(&self, thread_id: &str) -> io::Result<String> {
        let cwd = self
            .client
            .ensure_thread_worktree(thread_id)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(cwd)
    }

    async fn list_worktrees(&self) -> io::Result<Vec<WorktreeEntry>> {
        self.client.list_worktrees().await.map_err(to_io)
    }

    async fn switch_thread_worktree(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> io::Result<String> {
        let cwd = self
            .client
            .switch_thread_worktree(thread_id, target_path)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(cwd)
    }

    async fn remove_worktree(&self, path: &str, force: bool) -> io::Result<()> {
        self.client
            .remove_worktree(path, force)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(())
    }

    async fn worktree_diff_preview(&self, path: &str) -> io::Result<String> {
        self.client.worktree_diff_preview(path).await.map_err(to_io)
    }

    async fn prune_worktrees(&self) -> io::Result<String> {
        let summary = self.client.prune_worktrees().await.map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(summary)
    }

    async fn load_preferences(&self) -> io::Result<UserPreferencesSnapshot> {
        let Some(path) = grein_core::default_prefs_db_path() else {
            return Ok(UserPreferencesSnapshot::default());
        };
        if !path.exists() {
            return Ok(UserPreferencesSnapshot::default());
        }
        let store = grein_core::UserPrefsStore::open(&path).map_err(to_io)?;
        Ok(UserPreferencesSnapshot {
            theme: store.theme().map_err(to_io)?,
            theme_overrides: store.theme_prefs().map_err(to_io)?,
            keybindings: store.keybindings().map_err(to_io)?,
            ui: store.ui_prefs().map_err(to_io)?,
        })
    }

    async fn save_theme_preference(&self, theme: &str) -> io::Result<()> {
        if let Some(store) = grein_core::UserPrefsStore::open_default().map_err(to_io)? {
            store.clear_theme_prefs().map_err(to_io)?;
            store.set_theme(theme).map_err(to_io)?;
        }
        Ok(())
    }

    async fn save_keybinding_preference(&self, action: &str, binding: &str) -> io::Result<()> {
        if let Some(store) = grein_core::UserPrefsStore::open_default().map_err(to_io)? {
            store.set_keybinding(action, binding).map_err(to_io)?;
        }
        Ok(())
    }

    async fn remove_keybinding_preference(&self, action: &str) -> io::Result<()> {
        if let Some(store) = grein_core::UserPrefsStore::open_default().map_err(to_io)? {
            let _ = store.remove_keybinding(action).map_err(to_io)?;
        }
        Ok(())
    }

    async fn save_ui_preference(&self, key: &str, value: &str) -> io::Result<()> {
        if let Some(store) = grein_core::UserPrefsStore::open_default().map_err(to_io)? {
            store.set_ui_pref(key, value).map_err(to_io)?;
        }
        Ok(())
    }

    async fn pty_scroll(&self, thread_id: &str, delta: isize) -> io::Result<PtyScreenSnapshot> {
        let snapshot = self
            .client
            .pty_scroll(thread_id, delta)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::PtyScreenUpdated {
                thread_id: thread_id.to_string(),
                snapshot: snapshot.clone(),
            });
        }
        Ok(snapshot)
    }

    async fn pty_restart(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> io::Result<PtyScreenSnapshot> {
        let snapshot = self
            .client
            .pty_restart(thread_id, rows, cols)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let _ = self.fetch_and_emit_thread_metadata().await;
            self.emit(TuiEngineEvent::ThreadStatusChanged {
                thread_id: thread_id.to_string(),
                status: "idle".to_string(),
            });
            self.emit(TuiEngineEvent::PtyScreenUpdated {
                thread_id: thread_id.to_string(),
                snapshot: snapshot.clone(),
            });
        }
        Ok(snapshot)
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let rx = self
            .events_rx
            .lock()
            .await
            .take()
            .ok_or_else(|| io::Error::other("event stream already subscribed"))?;
        if let Some(mut engine_rx) = self.client.subscribe_events().await.map_err(to_io)? {
            self.has_live_stream.store(true, Ordering::SeqCst);
            let tx = self.events_tx.clone();
            let in_flight = Arc::clone(&self.in_flight_turns);
            let buffered = Arc::clone(&self.buffered_sync_errors);
            let suppressed = Arc::clone(&self.suppressed_sync_errors);
            tokio::spawn(async move {
                while let Some(ev) = engine_rx.recv().await {
                    if let TuiEngineEvent::Error { ref message } = ev {
                        let is_suppressed = {
                            let mut sup = suppressed.lock().unwrap_or_else(|p| p.into_inner());
                            let now = Instant::now();
                            sup.retain(|(_, ts)| now.duration_since(*ts) < Duration::from_secs(5));
                            if let Some(pos) = sup
                                .iter()
                                .position(|(err_msg, _)| error_matches_sync(err_msg, message))
                            {
                                sup.remove(pos);
                                true
                            } else {
                                false
                            }
                        };
                        if is_suppressed {
                            continue;
                        }
                        if in_flight.load(Ordering::SeqCst) > 0 {
                            let mut buf = buffered.lock().unwrap_or_else(|p| p.into_inner());
                            buf.push(message.clone());
                            continue;
                        }
                    }
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(rx)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    let initial_mode = if cli.agent {
        InitialSessionMode::Agent
    } else {
        InitialSessionMode::Terminal
    };

    match cli.command {
        None | Some(Command::Standalone {}) => {
            if std::io::stdout().is_terminal() {
                let engine = InProcessEngine::open(&grein_cli::daemon::default_db_path())?;
                grein_tui::run_with_engine(Arc::new(EngineClientPort::new_with_mode(
                    engine,
                    initial_mode,
                )))
                .await?;
            }
        }
        Some(Command::Daemon(DaemonArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            grein_cli::daemon::run_daemon_with_socket(socket_path).await?;
        }
        Some(Command::Attach(AttachArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            let transport = UnixSocketEngineClient::connect(&socket_path).await?;
            if std::io::stdout().is_terminal() {
                grein_tui::run_with_engine(Arc::new(EngineClientPort::new_with_mode(
                    transport,
                    initial_mode,
                )))
                .await?;
            }
        }
        Some(Command::Threads(args)) => {
            grein_cli::headless::handle_threads(&args).await?;
        }
        Some(Command::Status(args)) => {
            grein_cli::headless::handle_status(&args).await?;
        }
        Some(Command::Agents(args)) => {
            grein_cli::headless::handle_agents(&args).await?;
        }
        Some(Command::Doctor(args)) => {
            grein_cli::doctor::handle_doctor(&args)?;
        }
        Some(Command::Settings(args)) => {
            grein_cli::settings_cmd::handle_settings(&args)?;
        }
        Some(Command::Theme(args)) => {
            grein_cli::settings_cmd::handle_theme(&args)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_client_port_hydrates_main_thread_and_no_duplicate_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open");
        let port = EngineClientPort::new(engine);

        let session = port.hydrate_session().await.expect("hydrate");
        assert_eq!(session.threads.len(), 1);
        assert_eq!(session.active_thread_id, session.threads[0].id);
        assert_eq!(session.threads[0].title, "Main");
        assert!(session.threads[0].turns.is_empty());

        let mut rx = port.subscribe_events().await.expect("subscribe");

        let forked = port
            .fork_branch(&session.active_thread_id, Some("auth"))
            .await
            .expect("fork_branch");
        assert_eq!(forked.custom_name.as_deref(), Some("auth"));
        assert_eq!(forked.title, "Branch 2");

        let err = port
            .send_turn(&session.active_thread_id, "hello")
            .await
            .expect_err("offline send_turn must fail");
        assert!(!err.to_string().is_empty());

        port.run_shell_command(&session.active_thread_id, "printf 'hi'")
            .await
            .expect("shell");

        let mut events = Vec::new();
        for _ in 0..20 {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
            if events
                .iter()
                .any(|e| matches!(e, TuiEngineEvent::ShellOutput { .. }))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, TuiEngineEvent::Error { .. })),
            "EngineClientPort must not emit duplicate Error event when send_turn returns Err"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ShellOutput { stdout, exit_code, .. } if stdout == "hi" && *exit_code == 0
        )));

        drop(port);

        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        let port2 = EngineClientPort::new(reopened);
        let session2 = port2.hydrate_session().await.expect("rehydrate");
        assert_eq!(session2.threads.len(), 2);
        assert_eq!(session2.threads[0].turns.len(), 1);
        assert_eq!(session2.threads[0].turns[0].role, "user");
        assert_eq!(session2.threads[0].turns[0].content, "hello");
    }

    #[tokio::test]
    async fn test_engine_client_port_live_acp_streams_permissions_and_rehydrates() {
        use grein_cli::engine::AcpBackendConfig;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;
        use tokio::time::timeout;

        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let script_path = temp.path().join("mock_tui_acp.sh");
        let script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-tui"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-tui","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Mocked TUI Reply"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
        std::fs::write(&script_path, script).expect("write");
        let mut perms = std::fs::metadata(&script_path)
            .expect("meta")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).expect("chmod");

        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };
        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
                .expect("open");
        let port = Arc::new(EngineClientPort::new(engine));

        let session = port.hydrate_session().await.expect("hydrate");
        let main_id = session.active_thread_id.clone();
        let mut rx = port.subscribe_events().await.expect("subscribe");

        port.send_turn(&main_id, "ping acp")
            .await
            .expect("send_turn");

        let mut chunks = String::new();
        let mut saw_completed = false;
        while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
            match ev {
                TuiEngineEvent::StreamChunk { content, .. } => chunks.push_str(&content),
                TuiEngineEvent::TurnCompleted { .. } => {
                    saw_completed = true;
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(chunks, "Mocked TUI Reply");
        assert!(saw_completed);

        port.cancel_turn(&main_id).await.expect("cancel_turn");
        let forked = port
            .fork_branch(&main_id, Some("side-branch"))
            .await
            .expect("fork_branch");
        assert!(
            forked.turns.is_empty(),
            "forked branch must hydrate only branch-local turns, got {:?}",
            forked.turns
        );
        let meta = port.thread_metadata().await.expect("thread_metadata");
        assert_eq!(meta.len(), 2);
        let fork_meta = meta
            .iter()
            .find(|m| m.thread_id == forked.id)
            .expect("fork meta");
        assert_eq!(fork_meta.parent_thread_id.as_deref(), Some(main_id.as_str()));
        assert_eq!(fork_meta.inherited_turn_count, 2);

        port.merge_thread(&forked.id, &main_id)
            .await
            .expect("merge_thread");
        port.resolve_permission("nonexistent-perm", true)
            .await
            .expect("resolve_permission");

        let next_active = port
            .delete_thread(&forked.id)
            .await
            .expect("delete_thread");
        assert_eq!(next_active, main_id);
        assert!(port.delete_thread(&main_id).await.is_err());

        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        let port2 = EngineClientPort::new(reopened);
        let hydrated = port2.hydrate_session().await.expect("rehydrate");
        assert_eq!(hydrated.threads.len(), 1);
        assert_eq!(hydrated.threads[0].turns.len(), 3);
        assert_eq!(hydrated.threads[0].turns[0].content, "ping acp");
        assert_eq!(hydrated.threads[0].turns[1].content, "Mocked TUI Reply");
        assert!(hydrated.threads[0].turns[2]
            .content
            .contains("[Decision Landmark"));
    }

    #[tokio::test]
    async fn test_engine_client_port_forwards_async_background_errors() {
        use grein_cli::engine::{
            EngineError, EngineEvent, SessionSnapshot, ShellCommandResult, ThreadSummary,
            TranscriptEntry, TurnDispatchResult,
        };

        struct BgErrorEngine {
            rx: Mutex<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>>,
        }

        #[async_trait::async_trait]
        impl DaemonEngine for BgErrorEngine {
            async fn ping(&self) -> Result<(), EngineError> {
                Ok(())
            }
            async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
                Ok(ThreadSummary {
                    thread_id: "t-1".into(),
                    title: "Main".into(),
                    custom_name: None,
                    parent_node_id: "n-1".into(),
                    active_leaf_id: "n-1".into(),
                    merged: false,
                    status: Default::default(),
                })
            }
            async fn send_turn(&self, _prompt: &str) -> Result<TurnDispatchResult, EngineError> {
                Ok(TurnDispatchResult {
                    node_id: "n-2".into(),
                })
            }
            async fn send_turn_for_thread(
                &self,
                _thread_id: &str,
                _prompt: &str,
            ) -> Result<TurnDispatchResult, EngineError> {
                Ok(TurnDispatchResult {
                    node_id: "n-2".into(),
                })
            }
            async fn fork_thread(
                &self,
                _parent_node_id: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn fork_thread_with_title(
                &self,
                _source: Option<&str>,
                _title: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn create_root_thread(
                &self,
                _title: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn rename_thread(
                &self,
                _thread_id: &str,
                _title: &str,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
                Ok(vec![self.ensure_main_thread().await?])
            }
            async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
                Ok(Vec::new())
            }
            async fn get_thread_transcript(
                &self,
                _thread_id: &str,
            ) -> Result<Vec<TranscriptEntry>, EngineError> {
                Ok(Vec::new())
            }
            async fn merge_thread(&self, _thread_id: &str) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn merge_thread_with_summary(
                &self,
                _source_id: &str,
                _target_id: Option<&str>,
                _summary: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn resolve_permission(
                &self,
                _decision: PermissionDecisionPayload,
            ) -> Result<EngineEvent, EngineError> {
                unreachable!()
            }
            async fn cancel_turn(&self, _thread_id: &str) -> Result<(), EngineError> {
                Ok(())
            }
            async fn run_shell_command(
                &self,
                _thread_id: &str,
                _command: &str,
            ) -> Result<ShellCommandResult, EngineError> {
                unreachable!()
            }
            async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
                unreachable!()
            }
            async fn subscribe_events(
                &self,
            ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
                Ok(self.rx.lock().await.take())
            }
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let port = EngineClientPort::new(BgErrorEngine {
            rx: Mutex::new(Some(rx)),
        });
        let _ = port.hydrate_session().await.expect("hydrate");
        let mut sub_rx = port.subscribe_events().await.expect("subscribe");

        tx.send(TuiEngineEvent::Error {
            message: "async wakeup failed".to_string(),
        })
        .expect("send bg error");

        let mut saw_bg_err = false;
        while let Ok(Some(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(150), sub_rx.recv()).await
        {
            if matches!(ev, TuiEngineEvent::Error { ref message } if message == "async wakeup failed")
            {
                saw_bg_err = true;
                break;
            }
        }
        assert!(
            saw_bg_err,
            "async background TuiEngineEvent::Error must be forwarded to TUI"
        );
    }

    #[tokio::test]
    async fn test_engine_client_port_offline_fallbacks_and_metadata_error_forwarding() {
        use grein_cli::engine::{
            EngineError, EngineEvent, SessionSnapshot, ShellCommandResult, ThreadSummary,
            TranscriptEntry, TurnDispatchResult,
        };

        let engine = InProcessEngine::in_memory().expect("in_memory");
        let port = EngineClientPort::new(engine);
        let session = port.hydrate_session().await.expect("hydrate");
        let main_id = session.active_thread_id.clone();

        // Notice: subscribe_events NOT called -> has_live_stream == false!
        // Drain initial events via direct receiver? Actually subscribe_events on InProcessEngine
        // would flip has_live_stream to true. So test offline EngineClientPort with a no-stream
        // DaemonEngine wrapper or test create_root/rename/merge before subscribe_events!
        let root2 = port.create_root(Some("Offline Root")).await.expect("create_root");
        assert_eq!(root2.custom_name.as_deref(), Some("Offline Root"));
        assert_eq!(root2.title, "Root 2");
        port.rename_thread(&root2.id, "Renamed Offline Root")
            .await
            .expect("rename");
        port.rename_thread(&root2.id, "")
            .await
            .expect("clear offline rename");
        port.rename_thread(&root2.id, "Renamed Offline Root")
            .await
            .expect("restore offline rename");
        let forked = port
            .fork_branch(&main_id, Some("Offline Branch"))
            .await
            .expect("fork_branch");
        port.merge_thread_with_summary(&forked.id, &main_id, Some("Offline decision"))
            .await
            .expect("merge_thread_with_summary");

        let mut rx = port.events_rx.lock().await.take().expect("events_rx");
        let mut offline_evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            offline_evs.push(ev);
        }
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCreated { thread } if thread.id == root2.id
        )));
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
                if thread_id == &root2.id && custom_name.as_deref() == Some("Renamed Offline Root")
        )));
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
                if thread_id == &root2.id && custom_name.is_none()
        )));
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCreated { thread } if thread.id == forked.id
        )));
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, summary }
                if thread_id == &forked.id
                    && target_thread_id == &main_id
                    && summary == "Offline decision"
        )));

        struct FailingMetaEngine;
        #[async_trait::async_trait]
        impl DaemonEngine for FailingMetaEngine {
            async fn ping(&self) -> Result<(), EngineError> { Ok(()) }
            async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
                Ok(ThreadSummary {
                    thread_id: "t-1".into(),
                    title: "Main".into(),
                    custom_name: None,
                    parent_node_id: "n-1".into(),
                    active_leaf_id: "n-1".into(),
                    merged: false,
                    status: Default::default(),
                })
            }
            async fn send_turn(&self, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
            async fn send_turn_for_thread(&self, _: &str, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
            async fn fork_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn fork_thread_with_title(&self, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn create_root_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn rename_thread(&self, _: &str, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
                Ok(vec![self.ensure_main_thread().await?])
            }
            async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
            async fn get_thread_transcript(&self, _: &str) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
            async fn merge_thread(&self, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn merge_thread_with_summary(&self, _: &str, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn resolve_permission(&self, _: PermissionDecisionPayload) -> Result<EngineEvent, EngineError> { unreachable!() }
            async fn cancel_turn(&self, _: &str) -> Result<(), EngineError> { Ok(()) }
            async fn run_shell_command(&self, _: &str, _: &str) -> Result<ShellCommandResult, EngineError> { unreachable!() }
            async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> { unreachable!() }
            async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
                Err(EngineError::Storage("metadata db failure".to_string()))
            }
        }

        let err_port = EngineClientPort::new(FailingMetaEngine);
        let mut err_rx = err_port.subscribe_events().await.expect("subscribe");
        let _ = err_port.hydrate_session().await.expect("hydrate succeeds");
        assert!(err_port.thread_metadata().await.is_err());

        let mut err_evs = Vec::new();
        while let Ok(ev) = err_rx.try_recv() {
            err_evs.push(ev);
        }
        assert!(
            err_evs
                .iter()
                .filter(|e| matches!(e, TuiEngineEvent::Error { message } if message.contains("metadata db failure")))
                .count()
                >= 2,
            "thread_metadata Err must emit TuiEngineEvent::Error instead of ignoring: {err_evs:?}"
        );
    }

    #[tokio::test]
    async fn test_engine_client_port_hydrates_cached_options_commands_and_config_rpcs() {
        use grein_cli::engine::{
            AgentMetaEntry, EngineError, EngineEvent, SessionSnapshot, ShellCommandResult,
            ThreadSummary, TranscriptEntry, TurnDispatchResult,
        };
        use grein_tui::engine_port::{AgentConfigChoice, AgentConfigOption, AgentSlashCommand};

        struct CachedMetaEngine;

        #[async_trait::async_trait]
        impl DaemonEngine for CachedMetaEngine {
            async fn ping(&self) -> Result<(), EngineError> { Ok(()) }
            async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
                Ok(ThreadSummary {
                    thread_id: "t-cached".into(),
                    title: "Main".into(),
                    custom_name: None,
                    parent_node_id: "n-1".into(),
                    active_leaf_id: "n-1".into(),
                    merged: false,
                    status: Default::default(),
                })
            }
            async fn send_turn(&self, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
            async fn send_turn_for_thread(&self, _: &str, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
            async fn fork_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn fork_thread_with_title(&self, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn create_root_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn rename_thread(&self, _: &str, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
                Ok(vec![self.ensure_main_thread().await?])
            }
            async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
            async fn get_thread_transcript(&self, _: &str) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
            async fn merge_thread(&self, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn merge_thread_with_summary(&self, _: &str, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn resolve_permission(&self, _: PermissionDecisionPayload) -> Result<EngineEvent, EngineError> { unreachable!() }
            async fn cancel_turn(&self, _: &str) -> Result<(), EngineError> { Ok(()) }
            async fn run_shell_command(&self, _: &str, _: &str) -> Result<ShellCommandResult, EngineError> { unreachable!() }
            async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> { unreachable!() }
            async fn agent_metadata(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
                Ok(vec![AgentMetaEntry {
                    thread_id: "t-cached".into(),
                    model: Some("fake-model".into()),
                    mode: Some("default".into()),
                    config_options: vec![AgentConfigOption {
                        id: "model".into(),
                        name: "Model".into(),
                        category: Some("model".into()),
                        current_value: "fake-model".into(),
                        options: vec![AgentConfigChoice {
                            value: "fake-model".into(),
                            name: "Fake Model".into(),
                            description: None,
                        }],
                    }],
                    available_commands: vec![AgentSlashCommand {
                        name: "compact".into(),
                        description: "Compact context".into(),
                    }],
                }])
            }
            async fn agent_options(&self, thread_id: &str) -> Result<AgentOptionsSnapshot, EngineError> {
                Ok(AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: self.agent_metadata().await?.remove(0).config_options,
                    available_commands: self.agent_metadata().await?.remove(0).available_commands,
                })
            }
            async fn set_agent_config(
                &self,
                thread_id: &str,
                config_id: &str,
                value: &str,
                _set_default: bool,
            ) -> Result<AgentOptionsSnapshot, EngineError> {
                Ok(AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: vec![AgentConfigOption {
                        id: config_id.to_string(),
                        name: "Model".into(),
                        category: Some("model".into()),
                        current_value: value.to_string(),
                        options: Vec::new(),
                    }],
                    available_commands: Vec::new(),
                })
            }
        }

        let port = EngineClientPort::new(CachedMetaEngine);
        let session = port.hydrate_session().await.expect("hydrate");
        assert_eq!(session.active_thread_id, "t-cached");
        let mut rx = port.subscribe_events().await.expect("subscribe");

        let opts = port.agent_options("t-cached").await.expect("agent_options");
        assert_eq!(opts.config_options.len(), 1);
        assert_eq!(opts.available_commands.len(), 1);

        let updated = port
            .set_agent_config("t-cached", "model", "fake-model-fast", true)
            .await
            .expect("set_agent_config");
        assert_eq!(updated.config_options[0].current_value, "fake-model-fast");

        let mut evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            evs.push(ev);
        }
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::AgentMetaUpdated { thread_id, model, .. }
                if thread_id == "t-cached" && model.as_deref() == Some("fake-model")
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::AgentConfigUpdated { thread_id, config_options }
                if thread_id == "t-cached" && config_options.len() == 1
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::AgentCommandsUpdated { thread_id, commands }
                if thread_id == "t-cached" && commands.len() == 1 && commands[0].name == "compact"
        )));
    }

    #[tokio::test]
    async fn test_engine_client_port_terminal_and_worktree_methods() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).expect("mkdir repo");
        let run_git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .expect("git");
            assert!(status.success(), "git failed: {:?}", args);
        };
        run_git(&["init"]);
        run_git(&["config", "user.name", "Grein Test"]);
        run_git(&["config", "user.email", "test@grein.dev"]);
        std::fs::write(repo_root.join("file.txt"), "hello\n").expect("write");
        run_git(&["add", "."]);
        run_git(&["commit", "-m", "init"]);

        let db_path = temp.path().join("session.db");
        let engine = InProcessEngine::open_with_config(&db_path, repo_root.clone(), None)
            .expect("open");
        let port = EngineClientPort::new(engine);

        let session = port.hydrate_session().await.expect("hydrate");
        let main_id = session.active_thread_id.clone();

        let branch = port
            .fork_branch(&main_id, Some("wt-port"))
            .await
            .expect("fork");
        let wt_path = port
            .ensure_thread_worktree(&branch.id)
            .await
            .expect("ensure_thread_worktree");
        assert!(std::path::Path::new(&wt_path).join(".git").exists());

        let term = port
            .create_terminal(Some(&branch.id), None, None, 24, 100)
            .await
            .expect("create_terminal");
        assert_eq!(term.title, "Terminal");

        port.pty_resize(&term.id, 30, 240)
            .await
            .expect("pty_resize");
        port.pty_write(&term.id, b"printf '\\nPORT_%s\\n' \"$(pwd)\"\n".to_vec())
            .await
            .expect("pty_write");

        let expected = format!("PORT_{wt_path}");
        let mut saw_output = false;
        let mut last_screen = String::new();
        for _ in 0..150 {
            let snap = port
                .pty_snapshot(&term.id, 30, 240)
                .await
                .expect("pty_snapshot");
            assert_eq!(snap.rows, 30);
            assert_eq!(snap.cols, 240);
            last_screen = snap.plain_text();
            if last_screen.lines().any(|l| l.trim() == expected) {
                saw_output = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(
            saw_output,
            "EngineClientPort must drive PTY in worktree cwd; last screen:\n{last_screen}"
        );
    }

    #[tokio::test]
    async fn test_engine_client_port_create_terminal_non_live_emits_metadata_before_created() {
        let temp = tempfile::tempdir().expect("tempdir");
        let work_dir = temp.path().join("term_port_dir");
        std::fs::create_dir_all(&work_dir).expect("mkdir");
        let canonical_dir = std::fs::canonicalize(&work_dir).expect("canonical");
        let db_path = temp.path().join("port_order.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None)
                .expect("open");
        let port = EngineClientPort::new(engine);

        let session = port.hydrate_session().await.expect("hydrate");
        let mut rx = port.events_rx.lock().await.take().expect("events_rx");
        while rx.try_recv().is_ok() {}

        let term = port
            .create_terminal(
                Some(&session.active_thread_id),
                Some("Offline Term"),
                None,
                24,
                80,
            )
            .await
            .expect("create_terminal");

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }

        let meta_idx = events
            .iter()
            .position(|ev| {
                matches!(
                    ev,
                    TuiEngineEvent::ThreadMetadataUpdated { meta }
                        if meta.iter().any(|m| {
                            m.thread_id == term.id
                                && m.kind == grein_core::model::ThreadKind::Terminal
                                && m.cwd.as_deref() == Some(canonical_dir.to_string_lossy().as_ref())
                        })
                )
            })
            .expect("expected ThreadMetadataUpdated containing new terminal meta");
        let created_idx = events
            .iter()
            .position(|ev| {
                matches!(
                    ev,
                    TuiEngineEvent::ThreadCreated { thread } if thread.id == term.id
                )
            })
            .expect("expected ThreadCreated for terminal");

        assert!(
            meta_idx < created_idx,
            "non-live EngineClientPort ThreadMetadataUpdated (idx {meta_idx}) must precede ThreadCreated (idx {created_idx}): {events:?}"
        );
    }

    #[tokio::test]
    async fn test_engine_client_port_pty_scroll_and_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let work_dir = temp.path().join("port_pty_ws");
        std::fs::create_dir_all(&work_dir).expect("mkdir");
        let canonical_dir = std::fs::canonicalize(&work_dir).expect("canonical");
        let db_path = temp.path().join("port_pty.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None).expect("open");
        let port = EngineClientPort::new(engine);

        let session = port.hydrate_session().await.expect("hydrate");
        let mut rx = port.subscribe_events().await.expect("subscribe");
        let term = port
            .create_terminal(Some(&session.active_thread_id), Some("Port Term"), None, 24, 80)
            .await
            .expect("create_terminal");

        port.pty_write(
            &term.id,
            b"printf '\\n'; i=1; while [ \"$i\" -le 50 ]; do printf \"PORT_L_%02d\\n\" \"$i\"; i=$((i+1)); done; printf \"PORT_%s\" \"READY\"; read -r grein_pause; exit 7\n".to_vec(),
        )
        .await
        .expect("pty_write");

        let mut saw_50 = false;
        for _ in 0..150 {
            let snap = port.pty_snapshot(&term.id, 24, 80).await.expect("snapshot");
            let text = snap.plain_text();
            if text.lines().any(|l| l.trim() == "PORT_L_50")
                && text.lines().any(|l| l.trim() == "PORT_READY")
            {
                saw_50 = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_50, "expected PORT_L_50 in EngineClientPort snapshot");

        let scrolled = port.pty_scroll(&term.id, 10).await.expect("pty_scroll");
        assert_eq!(scrolled.scrollback_offset, 10);
        let restored = port.pty_scroll(&term.id, -10).await.expect("pty_scroll -10");
        assert_eq!(restored.scrollback_offset, 0);

        port.pty_write(&term.id, b"\n".to_vec())
            .await
            .expect("exit 7");
        let mut saw_exit = false;
        for _ in 0..150 {
            let snap = port.pty_snapshot(&term.id, 24, 80).await.expect("snapshot exited");
            if snap.exited && snap.exit_code == Some(7) {
                saw_exit = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_exit, "expected exit code 7 before restart");

        while rx.try_recv().is_ok() {}
        let restarted = port.pty_restart(&term.id, 26, 96).await.expect("pty_restart");
        assert!(!restarted.exited);
        assert_eq!(restarted.exit_code, None);
        assert_eq!((restarted.rows, restarted.cols), (26, 96));

        let mut saw_restart_event = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(TuiEngineEvent::PtyScreenUpdated { thread_id, snapshot })) =
                tokio::time::timeout(Duration::from_millis(150), rx.recv()).await
            {
                if thread_id == term.id
                    && !snapshot.exited
                    && snapshot.exit_code.is_none()
                    && snapshot.rows == 26
                    && snapshot.cols == 96
                {
                    saw_restart_event = true;
                    break;
                }
            }
        }
        assert!(
            saw_restart_event,
            "EngineClientPort::pty_restart must forward PtyScreenUpdated"
        );
    }

    #[test]
    fn test_cli_agent_flag_defaults_false_and_parses_true() {
        let default_cli = Cli::try_parse_from(["grein"]).expect("parse default grein");
        assert!(!default_cli.agent);

        let agent_cli = Cli::try_parse_from(["grein", "--agent"]).expect("parse grein --agent");
        assert!(agent_cli.agent);

        let attach_agent =
            Cli::try_parse_from(["grein", "attach", "--agent"]).expect("parse attach --agent");
        assert!(attach_agent.agent);

        let standalone_agent = Cli::try_parse_from(["grein", "--agent", "standalone"])
            .expect("parse --agent standalone");
        assert!(standalone_agent.agent);
    }

    #[tokio::test]
    async fn test_engine_client_port_terminal_mode_empty_db_hydrates_root_terminal_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let canonical_dir = std::fs::canonicalize(temp.path()).expect("canonical tempdir");
        let db_path = temp.path().join("term_first.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None).expect("open");
        let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Terminal);

        let session = port.hydrate_session().await.expect("hydrate terminal mode");
        assert_eq!(session.threads.len(), 1);
        assert_eq!(session.active_thread_id, session.threads[0].id);
        assert_eq!(session.threads[0].title, "Terminal");
        assert!(session.threads[0].turns.is_empty());

        let meta = port.thread_metadata().await.expect("thread_metadata");
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].thread_id, session.active_thread_id);
        assert_eq!(meta[0].kind, grein_core::model::ThreadKind::Terminal);
        assert!(meta[0].parent_thread_id.is_none());
        assert_eq!(
            meta[0].cwd.as_deref(),
            Some(canonical_dir.to_string_lossy().as_ref())
        );

        let mut rx = port.events_rx.lock().await.take().expect("events_rx");
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(
            events.iter().any(|ev| matches!(
                ev,
                TuiEngineEvent::ThreadMetadataUpdated { meta }
                    if meta.len() == 1
                        && meta[0].thread_id == session.active_thread_id
                        && meta[0].kind == grein_core::model::ThreadKind::Terminal
            )),
            "offline hydrate_session must emit ThreadMetadataUpdated for root terminal: {events:?}"
        );
    }

    #[tokio::test]
    async fn test_engine_client_port_agent_mode_empty_db_hydrates_main_agent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("agent_first.db");

        let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("open");
        let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Agent);

        let session = port.hydrate_session().await.expect("hydrate agent mode");
        assert_eq!(session.threads.len(), 1);
        assert_eq!(session.active_thread_id, session.threads[0].id);
        assert_eq!(session.threads[0].title, "Main");

        let meta = port.thread_metadata().await.expect("thread_metadata");
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].kind, grein_core::model::ThreadKind::Agent);
    }

    #[tokio::test]
    async fn test_engine_client_port_reopen_terminal_only_db_preserves_threads_without_main() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("reopen_term_only.db");

        let term_id = {
            let engine =
                InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                    .expect("open 1");
            let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Terminal);
            let session = port.hydrate_session().await.expect("hydrate 1");
            assert_eq!(session.threads.len(), 1);
            session.active_thread_id
        };

        for mode in [InitialSessionMode::Terminal, InitialSessionMode::Agent] {
            let reopened =
                InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                    .expect("reopen");
            let port = EngineClientPort::new_with_mode(reopened, mode);
            let session = port.hydrate_session().await.expect("rehydrate");
            assert_eq!(
                session.threads.len(),
                1,
                "reopening terminal-only DB in {mode:?} mode must not inject Main agent"
            );
            assert_eq!(session.active_thread_id, term_id);
            assert_eq!(session.threads[0].id, term_id);
            assert_eq!(session.threads[0].title, "Terminal");

            let meta = port.thread_metadata().await.expect("meta");
            assert_eq!(meta.len(), 1);
            assert_eq!(meta[0].kind, grein_core::model::ThreadKind::Terminal);
        }
    }

    #[tokio::test]
    async fn test_engine_client_port_terminal_mode_with_live_stream_hydrates_cleanly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let canonical_dir = std::fs::canonicalize(temp.path()).expect("canonical tempdir");
        let db_path = temp.path().join("live_term_first.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None).expect("open");
        let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Terminal);

        let mut rx = port.subscribe_events().await.expect("subscribe");
        let session = port.hydrate_session().await.expect("hydrate");
        assert_eq!(session.threads.len(), 1);
        assert_eq!(session.threads[0].title, "Terminal");
        assert_eq!(session.active_thread_id, session.threads[0].id);

        let mut events = Vec::new();
        while let Ok(Some(ev)) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            events.push(ev);
        }
        assert!(
            events.iter().any(|ev| matches!(
                ev,
                TuiEngineEvent::ThreadMetadataUpdated { meta }
                    if meta.iter().any(|m| {
                        m.thread_id == session.active_thread_id
                            && m.kind == grein_core::model::ThreadKind::Terminal
                    })
            )),
            "live stream terminal-first hydration must deliver ThreadMetadataUpdated: {events:?}"
        );
    }

    static PREFS_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct EnvVarGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvVarGuard {
        fn capture(keys: &[&'static str]) -> Self {
            let saved = keys
                .iter()
                .map(|&k| (k, std::env::var_os(k)))
                .collect();
            Self { saved }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            for (k, val) in self.saved.drain(..) {
                match val {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn test_cli_parses_doctor_settings_and_theme_subcommands() {
        let doc = Cli::try_parse_from(["grein", "doctor", "--json"]).expect("parse doctor");
        assert_eq!(
            doc.command,
            Some(Command::Doctor(grein_cli::args::DoctorArgs { json: true }))
        );

        let set_exp = Cli::try_parse_from([
            "grein",
            "settings",
            "export",
            "out.json",
            "--only",
            "theme,ui",
        ])
        .expect("parse settings export");
        match set_exp.command {
            Some(Command::Settings(s)) => match s.command {
                grein_cli::args::SettingsCommand::Export(args) => {
                    assert_eq!(args.file.as_deref(), Some(std::path::Path::new("out.json")));
                    assert_eq!(args.format, None);
                    assert_eq!(args.only, vec!["theme".to_string(), "ui".to_string()]);
                }
                other => panic!("unexpected settings command: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }

        let set_imp = Cli::try_parse_from([
            "grein",
            "settings",
            "import",
            "-",
            "--format",
            "json",
            "--replace",
        ])
        .expect("parse settings import");
        match set_imp.command {
            Some(Command::Settings(s)) => match s.command {
                grein_cli::args::SettingsCommand::Import(args) => {
                    assert_eq!(args.file, std::path::PathBuf::from("-"));
                    assert_eq!(args.format, grein_cli::args::InputFormatArg::Json);
                    assert!(args.replace);
                }
                other => panic!("unexpected settings command: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }

        let theme_exp = Cli::try_parse_from(["grein", "theme", "export", "--format", "toml"])
            .expect("parse theme export");
        match theme_exp.command {
            Some(Command::Theme(t)) => match t.command {
                grein_cli::args::ThemeCommand::Export(args) => {
                    assert_eq!(args.file, None);
                    assert_eq!(args.format, Some(grein_cli::args::OutputFormatArg::Toml));
                }
                other => panic!("unexpected theme command: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }

        let theme_imp =
            Cli::try_parse_from(["grein", "theme", "import", "colleague.toml", "--replace"])
                .expect("parse theme import");
        match theme_imp.command {
            Some(Command::Theme(t)) => match t.command {
                grein_cli::args::ThemeCommand::Import(args) => {
                    assert_eq!(args.file, std::path::PathBuf::from("colleague.toml"));
                    assert_eq!(args.format, grein_cli::args::InputFormatArg::Auto);
                    assert!(args.replace);
                }
                other => panic!("unexpected theme command: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_engine_client_port_global_preferences_round_trip() {
        let _lock = PREFS_ENV_LOCK.lock().await;
        let _guard = EnvVarGuard::capture(&["GREIN_CONFIG_DB", "XDG_CONFIG_HOME", "HOME"]);

        let temp = tempfile::tempdir().expect("tempdir");
        let prefs_db = temp.path().join("nested").join("prefs.db");
        std::env::set_var("GREIN_CONFIG_DB", &prefs_db);
        std::env::remove_var("XDG_CONFIG_HOME");

        let session_db = temp.path().join("session.db");
        let engine =
            InProcessEngine::open_with_config(&session_db, temp.path().to_path_buf(), None)
                .expect("open engine");
        let port = EngineClientPort::new(engine);

        let initial = port.load_preferences().await.expect("initial load_preferences");
        assert_eq!(initial, UserPreferencesSnapshot::default());
        assert!(
            !prefs_db.exists(),
            "load_preferences must not eagerly create prefs DB when absent"
        );

        port.save_ui_preference("sidebar_width", "36")
            .await
            .expect("save ui pref");
        port.save_keybinding_preference("quit", "q")
            .await
            .expect("save keybinding quit");
        port.save_keybinding_preference("focus_next", "Tab")
            .await
            .expect("save keybinding focus_next");
        port.save_theme_preference("nord")
            .await
            .expect("save theme nord");

        grein_core::UserPrefsStore::open(&prefs_db)
            .expect("open store")
            .set_theme_pref("accent", "#abcdef")
            .expect("set accent");

        let hydrated = port.load_preferences().await.expect("load hydrated prefs");
        assert_eq!(hydrated.theme.as_deref(), Some("nord"));
        assert_eq!(
            hydrated.theme_overrides.get("accent").map(String::as_str),
            Some("#abcdef")
        );
        assert_eq!(
            hydrated.keybindings.get("quit").map(String::as_str),
            Some("q")
        );
        assert_eq!(
            hydrated.keybindings.get("focus_next").map(String::as_str),
            Some("Tab")
        );
        assert_eq!(
            hydrated.ui.get("sidebar_width").map(String::as_str),
            Some("36")
        );

        // Switching theme clears previous custom theme overrides cleanly.
        port.save_theme_preference("dracula")
            .await
            .expect("save theme dracula");
        port.remove_keybinding_preference("quit")
            .await
            .expect("remove keybinding quit");

        let updated = port.load_preferences().await.expect("load updated prefs");
        assert_eq!(updated.theme.as_deref(), Some("dracula"));
        assert!(
            !updated.theme_overrides.contains_key("accent"),
            "save_theme_preference must clear prior theme color overrides: {:?}",
            updated.theme_overrides
        );
        assert!(!updated.keybindings.contains_key("quit"));
        assert_eq!(
            updated.keybindings.get("focus_next").map(String::as_str),
            Some("Tab")
        );
        assert_eq!(
            updated.ui.get("sidebar_width").map(String::as_str),
            Some("36")
        );
    }

    #[tokio::test]
    async fn test_engine_client_port_preferences_handle_unset_and_unwritable_home_safely() {
        let _lock = PREFS_ENV_LOCK.lock().await;
        let _guard = EnvVarGuard::capture(&["GREIN_CONFIG_DB", "XDG_CONFIG_HOME", "HOME"]);

        let temp = tempfile::tempdir().expect("tempdir");
        let session_db = temp.path().join("session.db");
        let engine =
            InProcessEngine::open_with_config(&session_db, temp.path().to_path_buf(), None)
                .expect("open engine");
        let port = EngineClientPort::new(engine);

        std::env::remove_var("GREIN_CONFIG_DB");
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("HOME");

        assert_eq!(
            port.load_preferences().await.expect("load with unset env"),
            UserPreferencesSnapshot::default()
        );
        port.save_theme_preference("nord")
            .await
            .expect("save_theme_preference with None path");
        port.save_keybinding_preference("quit", "q")
            .await
            .expect("save_keybinding_preference with None path");
        port.remove_keybinding_preference("quit")
            .await
            .expect("remove_keybinding_preference with None path");
        port.save_ui_preference("sidebar_width", "32")
            .await
            .expect("save_ui_preference with None path");

        // Simulates Nix homeless-shelter read-only HOME when prefs.db does not exist.
        std::env::set_var("HOME", "/nonexistent/grein-homeless-shelter");
        assert_eq!(
            port.load_preferences()
                .await
                .expect("load with nonexistent HOME"),
            UserPreferencesSnapshot::default()
        );
    }
}
