use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use grein_cli::engine::{
    truncate_chars_with_ellipsis, DaemonEngine, PermissionDecisionPayload,
};
use grein_tui::engine_port::{
    AgentOptionsSnapshot, HydratedSession, HydratedThread, HydratedTurn, PtyScreenSnapshot,
    ThreadMeta, TuiEngineEvent, TuiEnginePort, UserPreferencesSnapshot, WorktreeEntry,
};
use tokio::sync::{mpsc, Mutex};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_session_prefs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialSessionMode {
    Terminal,
    Agent,
}

/// Adapts an engine client (any `DaemonEngine`) to the TUI engine port.
pub(super) struct EngineClientPort<E> {
    client: E,
    initial_mode: InitialSessionMode,
    events_tx: mpsc::UnboundedSender<TuiEngineEvent>,
    pub(super) events_rx: Mutex<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>>,
    has_live_stream: AtomicBool,
    in_flight_turns: Arc<AtomicUsize>,
    buffered_sync_errors: Arc<StdMutex<Vec<String>>>,
    suppressed_sync_errors: Arc<StdMutex<Vec<(String, Instant)>>>,
}

impl<E> EngineClientPort<E> {
    #[cfg(test)]
    pub(super) fn new(client: E) -> Self {
        Self::new_with_mode(client, InitialSessionMode::Agent)
    }

    pub(super) fn new_with_mode(client: E, initial_mode: InitialSessionMode) -> Self {
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
