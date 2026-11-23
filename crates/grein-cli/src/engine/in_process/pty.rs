use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use grein_core::model::{AuthorRole, ConversationNode, Thread, ThreadId, ThreadKind, ThreadStatus};
use grein_tui::engine_port::{
    HydratedThread, PtyScreenSnapshot as TuiPtyScreenSnapshot, TuiEngineEvent,
};
use grein_workspace::pty_session::InteractivePtySession;

use super::super::types::{
    normalize_custom_name, parse_typed_id, pty_snapshot_to_tui, storage_err, thread_to_summary,
    EngineError, ThreadSummary, PTY_COALESCE_WINDOW,
};
use super::{EngineRuntime, InProcessEngine};

fn is_worktree_cwd(cwd: &Path) -> bool {
    if let Some(git_root) = cwd.ancestors().find(|dir| dir.join(".git").exists()) {
        if git_root.join(".git").is_file() {
            return true;
        }
    }
    let s = cwd.to_string_lossy();
    s.contains("/.grein/worktrees/")
        || s.ends_with("/.grein/worktrees")
        || s.contains("\\.grein\\worktrees\\")
        || s.ends_with("\\.grein\\worktrees")
}

impl EngineRuntime {
    fn ensure_pty_notifier_started(self: &Arc<Self>) {
        let maybe_rx = self
            .pty_notify_rx
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        let Some(mut rx) = maybe_rx else {
            return;
        };
        let weak_rt: Weak<EngineRuntime> = Arc::downgrade(self);
        let handle = tokio::spawn(async move {
            while let Some(first_tid) = rx.recv().await {
                let mut dirty = HashSet::new();
                dirty.insert(first_tid);
                let deadline = tokio::time::Instant::now() + PTY_COALESCE_WINDOW;
                loop {
                    let remaining =
                        deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, rx.recv()).await {
                        Ok(Some(tid)) => {
                            dirty.insert(tid);
                        }
                        Ok(None) => return,
                        Err(_) => break,
                    }
                }
                let Some(rt) = weak_rt.upgrade() else {
                    break;
                };
                rt.flush_dirty_pty_threads(dirty);
            }
        });
        if let Ok(mut guard) = self.pty_notifier_handle.lock() {
            *guard = Some(handle);
        }
    }

    fn flush_dirty_pty_threads(&self, dirty: HashSet<String>) {
        let mut cwd_updated = false;
        for thread_id in dirty {
            let session = self
                .active_pty_sessions
                .lock()
                .ok()
                .and_then(|map| map.get(&thread_id).cloned());
            let Some(session) = session else {
                continue;
            };
            let _ = session.poll_os_cwd();
            let raw_snap = session.snapshot();
            let raw_cwd = raw_snap.cwd.clone();
            let snap = pty_snapshot_to_tui(raw_snap);
            let exited = snap.exited;
            let exit_code = snap.exit_code;
            self.emit_tui(TuiEngineEvent::PtyScreenUpdated {
                thread_id: thread_id.clone(),
                snapshot: snap,
            });
            if self.sync_pty_cwd_to_storage(&thread_id, &session, raw_cwd.as_deref()) {
                cwd_updated = true;
            }
            if exited {
                let newly_exited = self
                    .pty_exited_threads
                    .lock()
                    .map(|mut seen| seen.insert(thread_id.clone()))
                    .unwrap_or(false);
                if newly_exited {
                    if let Some(tid) = parse_typed_id::<ThreadId>(&thread_id) {
                        if self.storage.get_thread(tid).ok().flatten().is_some() {
                            let _ = self.storage.update_thread_status(tid, ThreadStatus::Idle);
                        }
                    }
                    self.emit_tui(TuiEngineEvent::PtyExited {
                        thread_id: thread_id.to_string(),
                        exit_code,
                    });
                    self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
                        thread_id,
                        status: "idle".to_string(),
                    });
                }
            }
        }
        if cwd_updated {
            if let Ok(meta) = self.compute_thread_metadata() {
                self.emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
            }
        }
    }

    fn sync_pty_cwd_to_storage(
        &self,
        thread_id: &str,
        session: &Arc<InteractivePtySession>,
        raw_cwd: Option<&str>,
    ) -> bool {
        let Some(raw_dir) = raw_cwd.map(str::trim).filter(|s| !s.is_empty()) else {
            return false;
        };
        let Some(tid) = parse_typed_id::<ThreadId>(thread_id) else {
            return false;
        };
        let raw_path = Path::new(raw_dir);
        if !raw_path.is_dir() {
            return false;
        }
        let Ok(canonical_path) = std::fs::canonicalize(raw_path) else {
            return false;
        };
        let Some(canonical_dir) = canonical_path.to_str() else {
            return false;
        };
        let is_still_current = self
            .active_pty_sessions
            .lock()
            .ok()
            .is_some_and(|map| {
                map.get(thread_id)
                    .is_some_and(|cur| Arc::ptr_eq(cur, session))
            });
        if !is_still_current {
            return false;
        }
        if let Ok(Some(thread)) = self.storage.get_thread(tid) {
            if thread.cwd.as_deref() != Some(canonical_dir)
                && self
                    .storage
                    .update_thread_cwd(&tid, Some(canonical_dir))
                    .is_ok()
            {
                return true;
            }
        }
        false
    }

    pub(super) fn resolve_thread_cwd(&self, thread: &Thread) -> PathBuf {
        thread
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| self.cwd.clone())
    }

    fn expand_leading_tilde(raw: &str, home_dir: Option<&str>) -> Option<PathBuf> {
        let home = home_dir.filter(|h| !h.is_empty())?;
        if raw == "~" {
            return Some(PathBuf::from(home));
        }
        raw.strip_prefix("~/").map(|rest| {
            let trimmed_rest = rest.trim_start_matches('/');
            if trimmed_rest.is_empty() {
                PathBuf::from(home)
            } else {
                PathBuf::from(home).join(trimmed_rest)
            }
        })
    }

    fn resolve_terminal_cwd(
        &self,
        explicit_cwd: Option<&str>,
        parent_thread: Option<&Thread>,
    ) -> Result<String, EngineError> {
        let home_dir = std::env::var("HOME").ok();
        self.resolve_terminal_cwd_with_home(explicit_cwd, parent_thread, home_dir.as_deref())
    }

    pub(in crate::engine) fn resolve_terminal_cwd_with_home(
        &self,
        explicit_cwd: Option<&str>,
        parent_thread: Option<&Thread>,
        home_dir: Option<&str>,
    ) -> Result<String, EngineError> {
        let base_dir = parent_thread
            .map(|p| self.resolve_thread_cwd(p))
            .unwrap_or_else(|| self.cwd.clone());
        let candidate = match explicit_cwd.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                if let Some(expanded) = Self::expand_leading_tilde(raw, home_dir) {
                    expanded
                } else {
                    let raw_path = Path::new(raw);
                    if raw_path.is_absolute() {
                        raw_path.to_path_buf()
                    } else {
                        base_dir.join(raw_path)
                    }
                }
            }
            None => base_dir,
        };
        if !candidate.is_dir() {
            return Err(EngineError::InvalidParams(format!(
                "terminal cwd does not exist or is not a directory: {}",
                candidate.display()
            )));
        }
        let canonical = std::fs::canonicalize(&candidate).map_err(|e| {
            EngineError::InvalidParams(format!(
                "failed to resolve terminal cwd {}: {e}",
                candidate.display()
            ))
        })?;
        canonical
            .to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| EngineError::InvalidParams("terminal cwd is not valid UTF-8".to_string()))
    }

    pub(super) async fn teardown_active_pty(
        &self,
        thread_id: &str,
        clear_pending_size: bool,
    ) -> Result<Option<(u16, u16)>, EngineError> {
        let removed_pty = self
            .active_pty_sessions
            .lock()
            .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?
            .remove(thread_id);
        if clear_pending_size || removed_pty.is_some() {
            if let Ok(mut pending) = self.pending_pty_sizes.lock() {
                pending.remove(thread_id);
            }
        }
        if let Ok(mut exited) = self.pty_exited_threads.lock() {
            exited.remove(thread_id);
        }
        if let Some(pty_session) = removed_pty {
            let snap = pty_session.snapshot();
            let dims = (snap.rows, snap.cols);
            let _ = tokio::task::spawn_blocking(move || drop(pty_session)).await;
            Ok(Some(dims))
        } else {
            Ok(None)
        }
    }

    pub(super) async fn respawn_terminal_pty(
        self: &Arc<Self>,
        tid: ThreadId,
        thread_id: &str,
        cwd: PathBuf,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        let target_rows = if rows == 0 { 24 } else { rows.max(4) };
        let target_cols = if cols == 0 { 80 } else { cols.max(10) };
        let session = self
            .spawn_pty_for_thread(thread_id, cwd, target_rows, target_cols)
            .await?;
        let _ = self.storage.update_thread_status(tid, ThreadStatus::Idle);
        let snap = pty_snapshot_to_tui(session.snapshot());
        self.emit_tui(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "idle".to_string(),
        });
        self.emit_tui(TuiEngineEvent::PtyScreenUpdated {
            thread_id: thread_id.to_string(),
            snapshot: snap.clone(),
        });
        Ok(snap)
    }

    async fn spawn_pty_for_thread(
        self: &Arc<Self>,
        thread_id: &str,
        cwd: PathBuf,
        rows: u16,
        cols: u16,
    ) -> Result<Arc<InteractivePtySession>, EngineError> {
        self.ensure_pty_notifier_started();
        let notify_tx = self.pty_notify_tx.clone();
        let tid_for_cb = thread_id.to_string();
        let on_update: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = notify_tx.send(tid_for_cb.clone());
        });
        let clamped_rows = rows.max(4);
        let clamped_cols = cols.max(10);
        let tid_env = thread_id.to_string();
        let is_wt = is_worktree_cwd(&cwd);
        let spawned = tokio::task::spawn_blocking(move || {
            let mut extra_env: Vec<(&str, &str)> = vec![("GREIN_THREAD_ID", &tid_env)];
            if is_wt {
                extra_env.push(("GREIN_WORKTREE", "1"));
            }
            InteractivePtySession::spawn_with_env(
                &cwd,
                clamped_rows,
                clamped_cols,
                None,
                &extra_env,
                on_update,
            )
        })
        .await
        .map_err(|e| EngineError::Transport(format!("PTY spawn task panicked: {e}")))?
        .map_err(|e| EngineError::Transport(format!("failed to spawn PTY session: {e}")))?;

        let session = Arc::new(spawned);
        if let Ok(mut exited) = self.pty_exited_threads.lock() {
            exited.remove(thread_id);
        }
        if let Ok(mut map) = self.active_pty_sessions.lock() {
            map.insert(thread_id.to_string(), Arc::clone(&session));
        }
        let _ = self.pty_notify_tx.send(thread_id.to_string());
        Ok(session)
    }

    async fn ensure_pty_session(
        self: &Arc<Self>,
        thread_id: &str,
        thread: &Thread,
        rows: u16,
        cols: u16,
    ) -> Result<(Arc<InteractivePtySession>, bool), EngineError> {
        if let Some(existing) = self
            .active_pty_sessions
            .lock()
            .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?
            .get(thread_id)
            .cloned()
        {
            return Ok((existing, false));
        }

        let _guard = self.acquire_thread_turn_lock(thread_id).await?;
        let pending_size = {
            let sessions = self
                .active_pty_sessions
                .lock()
                .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?;
            if let Some(existing) = sessions.get(thread_id).cloned() {
                return Ok((existing, false));
            }
            self.pending_pty_sizes
                .lock()
                .map_err(|_| EngineError::Storage("pending pty sizes map poisoned".to_string()))?
                .remove(thread_id)
        };

        let (spawn_rows, spawn_cols, used_pending) = match pending_size {
            Some((pending_rows, pending_cols)) => (pending_rows, pending_cols, true),
            None => (
                if rows == 0 { 24 } else { rows },
                if cols == 0 { 80 } else { cols },
                false,
            ),
        };
        let cwd = self.resolve_thread_cwd(thread);
        let session = self
            .spawn_pty_for_thread(thread_id, cwd, spawn_rows, spawn_cols)
            .await?;

        let race_pending = self
            .pending_pty_sizes
            .lock()
            .map_err(|_| EngineError::Storage("pending pty sizes map poisoned".to_string()))?
            .remove(thread_id);
        if let Some((later_rows, later_cols)) = race_pending {
            let s_clone = Arc::clone(&session);
            tokio::task::spawn_blocking(move || s_clone.resize(later_rows, later_cols))
                .await
                .map_err(|e| EngineError::Transport(format!("PTY resize task panicked: {e}")))?
                .map_err(|e| EngineError::Transport(format!("PTY resize failed: {e}")))?;
            return Ok((session, true));
        }

        Ok((session, used_pending))
    }

    fn require_terminal_thread(&self, thread_id: &str) -> Result<(ThreadId, Thread), EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind != ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(format!(
                "thread {thread_id} is not a terminal thread"
            )));
        }
        Ok((tid, thread))
    }
}

impl InProcessEngine {
    pub(super) async fn create_terminal_impl(
        &self,
        parent_thread_id: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ThreadSummary, EngineError> {
        let parent_thread = match parent_thread_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                let parent_tid: ThreadId = parse_typed_id(raw)
                    .ok_or_else(|| EngineError::NotFound(format!("thread {raw}")))?;
                let thread = self
                    .runtime
                    .storage
                    .get_thread(parent_tid)
                    .map_err(storage_err)?
                    .ok_or_else(|| EngineError::NotFound(format!("thread {raw}")))?;
                Some(thread)
            }
            None => None,
        };

        let resolved_cwd = self
            .runtime
            .resolve_terminal_cwd(cwd, parent_thread.as_ref())?;
        let explicit_custom = normalize_custom_name(title);

        let (parent_node_id, parent_tid_opt) = match parent_thread.as_ref() {
            Some(parent) => (parent.active_leaf_id, Some(parent.id)),
            None => {
                let root_node = ConversationNode::new_root(AuthorRole::System, "");
                self.runtime
                    .storage
                    .insert_node(&root_node)
                    .map_err(storage_err)?;
                (root_node.id, None)
            }
        };

        let created = self.runtime.insert_new_thread_with_anchor(
            parent_node_id,
            "Terminal".to_string(),
            explicit_custom,
            parent_tid_opt,
            ThreadKind::Terminal,
            Some(resolved_cwd.clone()),
        )?;
        let tid = created.id;
        let tid_str = tid.0.to_string();

        if let Err(err) = self
            .runtime
            .spawn_pty_for_thread(&tid_str, PathBuf::from(&resolved_cwd), rows, cols)
            .await
        {
            if let Ok(mut pending) = self.runtime.pending_pty_sizes.lock() {
                pending.remove(&tid_str);
            }
            let _ = self.runtime.storage.delete_thread(tid);
            return Err(err);
        }

        let summary = thread_to_summary(&created, false);
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        self.runtime.emit_tui(TuiEngineEvent::ThreadCreated {
            thread: HydratedThread {
                id: summary.thread_id.clone(),
                title: summary.title.clone(),
                custom_name: summary.custom_name.clone(),
                turns: Vec::new(),
            },
        });
        Ok(summary)
    }

    pub(super) async fn pty_write_impl(
        &self,
        thread_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), EngineError> {
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;
        let (session, _) = self
            .runtime
            .ensure_pty_session(thread_id, &thread, 24, 80)
            .await?;
        tokio::task::spawn_blocking(move || session.write_bytes(&bytes))
            .await
            .map_err(|e| EngineError::Transport(format!("PTY write task panicked: {e}")))?
            .map_err(|e| EngineError::Transport(format!("PTY write failed: {e}")))
    }

    pub(super) async fn pty_resize_impl(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(), EngineError> {
        let _ = self.runtime.require_terminal_thread(thread_id)?;
        if rows == 0 || cols == 0 {
            return Ok(());
        }
        let target_rows = rows.max(4);
        let target_cols = cols.max(10);
        let session = {
            let sessions = self
                .runtime
                .active_pty_sessions
                .lock()
                .map_err(|_| EngineError::Storage("pty session map poisoned".to_string()))?;
            if let Some(existing) = sessions.get(thread_id).cloned() {
                Some(existing)
            } else {
                self.runtime
                    .pending_pty_sizes
                    .lock()
                    .map_err(|_| {
                        EngineError::Storage("pending pty sizes map poisoned".to_string())
                    })?
                    .insert(thread_id.to_string(), (target_rows, target_cols));
                None
            }
        };
        let Some(session) = session else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || session.resize(target_rows, target_cols))
            .await
            .map_err(|e| EngineError::Transport(format!("PTY resize task panicked: {e}")))?
            .map_err(|e| EngineError::Transport(format!("PTY resize failed: {e}")))
    }

    pub(super) async fn pty_snapshot_impl(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;
        let (session, used_pending) = self
            .runtime
            .ensure_pty_session(thread_id, &thread, rows, cols)
            .await?;
        let s_poll = Arc::clone(&session);
        let _ = tokio::task::spawn_blocking(move || s_poll.poll_os_cwd()).await;
        let snap = session.snapshot();
        let final_snap = if !used_pending && rows > 0 && cols > 0 {
            let target_rows = rows.max(4);
            let target_cols = cols.max(10);
            if snap.rows != target_rows || snap.cols != target_cols {
                let s_clone = Arc::clone(&session);
                tokio::task::spawn_blocking(move || s_clone.resize(target_rows, target_cols))
                    .await
                    .map_err(|e| EngineError::Transport(format!("PTY resize task panicked: {e}")))?
                    .map_err(|e| EngineError::Transport(format!("PTY resize failed: {e}")))?;
                session.snapshot()
            } else {
                snap
            }
        } else {
            snap
        };
        if self
            .runtime
            .sync_pty_cwd_to_storage(thread_id, &session, final_snap.cwd.as_deref())
        {
            if let Ok(meta) = self.runtime.compute_thread_metadata() {
                self.runtime
                    .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
            }
        }
        Ok(pty_snapshot_to_tui(final_snap))
    }

    pub(super) async fn pty_scroll_impl(
        &self,
        thread_id: &str,
        delta: isize,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;
        let (session, _) = self
            .runtime
            .ensure_pty_session(thread_id, &thread, 24, 80)
            .await?;
        session.scroll_by(delta);
        let snap = pty_snapshot_to_tui(session.snapshot());
        self.runtime.emit_tui(TuiEngineEvent::PtyScreenUpdated {
            thread_id: thread_id.to_string(),
            snapshot: snap.clone(),
        });
        Ok(snap)
    }

    pub(super) async fn pty_restart_impl(
        &self,
        thread_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<TuiPtyScreenSnapshot, EngineError> {
        let (tid, _) = self.runtime.require_terminal_thread(thread_id)?;
        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let (_, thread) = self.runtime.require_terminal_thread(thread_id)?;

        let _ = self.runtime.teardown_active_pty(thread_id, true).await?;
        let cwd = self.runtime.resolve_thread_cwd(&thread);
        let snap = self
            .runtime
            .respawn_terminal_pty(tid, thread_id, cwd, rows, cols)
            .await?;
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        Ok(snap)
    }
}
