use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use grein_core::model::{Thread, ThreadId, ThreadKind};
use grein_tui::engine_port::{TuiEngineEvent, WorktreeEntry};
use grein_workspace::allocator::{ThreadId as WorkspaceThreadId, WorktreeAllocator};
use grein_workspace::git::{GitError, GitRepository};
use grein_workspace::git_status::inspect_git_status;
use grein_workspace::pty_session::live_process_cwd;
use grein_workspace::worktrees::{
    list_worktrees_across_paths, prune_worktrees as workspace_prune_worktrees,
    remove_worktree as workspace_remove_worktree, resolve_primary_repo_root,
    resolve_worktree_toplevel, worktree_diff_preview as workspace_worktree_diff_preview,
    DiscoveredWorktree,
};

use super::super::types::{parse_typed_id, storage_err, EngineError};
use super::{EngineRuntime, InProcessEngine};

impl EngineRuntime {
    fn collect_worktree_search_paths(&self, threads: &[Thread]) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        let mut push_unique = |p: PathBuf| {
            if seen.insert(p.clone()) {
                paths.push(p);
            }
        };
        push_unique(self.cwd.clone());
        for thread in threads {
            if let Some(cwd) = thread.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                push_unique(PathBuf::from(cwd));
            }
        }
        if let Ok(sessions) = self.active_pty_sessions.lock() {
            for session in sessions.values() {
                if let Some(cwd) = session
                    .snapshot()
                    .cwd
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    push_unique(PathBuf::from(cwd));
                }
                if let Some(os_cwd) = live_process_cwd(session.child_pid()) {
                    push_unique(os_cwd);
                }
            }
        }
        paths
    }

    fn effective_thread_dir(&self, thread: &Thread) -> PathBuf {
        let raw = thread
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cwd.clone());
        std::fs::canonicalize(&raw).unwrap_or(raw)
    }

    fn longest_matching_worktree_index(
        thread_dir: &Path,
        toplevel: Option<&Path>,
        discovered: &[DiscoveredWorktree],
    ) -> Option<usize> {
        if let Some(top) = toplevel {
            if let Some(idx) = discovered.iter().position(|wt| wt.path == top) {
                return Some(idx);
            }
        }
        discovered
            .iter()
            .enumerate()
            .filter(|(_, wt)| thread_dir.starts_with(&wt.path))
            .max_by_key(|(_, wt)| (wt.path.components().count(), wt.path.as_os_str().len()))
            .map(|(idx, _)| idx)
    }
}

impl InProcessEngine {
    pub(super) async fn ensure_thread_worktree_impl(
        &self,
        thread_id: &str,
    ) -> Result<String, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let _initial = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let thread = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let cur_cwd_opt = thread.cwd.clone();
        let runtime_cwd = self.runtime.cwd.clone();
        let tid_owned = thread_id.to_string();
        let worktree_path = tokio::task::spawn_blocking(move || -> Result<PathBuf, EngineError> {
            let primary_root = match resolve_primary_repo_root(&runtime_cwd) {
                Some(root) => root,
                None => {
                    let repo = GitRepository::discover(&runtime_cwd).map_err(|err| match err {
                        GitError::NotFound => EngineError::InvalidParams(format!(
                            "workspace {} is not inside a git repository",
                            runtime_cwd.display()
                        )),
                        GitError::CommandFailed(msg) => EngineError::Transport(msg),
                    })?;
                    std::fs::canonicalize(repo.root()).unwrap_or_else(|_| repo.root().to_path_buf())
                }
            };

            let worktrees_base = primary_root.join(".grein").join("worktrees");
            let canonical_wt_base =
                std::fs::canonicalize(&worktrees_base).unwrap_or_else(|_| worktrees_base.clone());

            if let Some(cur_cwd) = cur_cwd_opt.as_deref().map(str::trim).filter(|s| !s.is_empty())
            {
                let cur_path = Path::new(cur_cwd);
                if cur_path.is_dir() {
                    if let Some(toplevel) = resolve_worktree_toplevel(cur_path) {
                        if toplevel.starts_with(&canonical_wt_base) && toplevel.join(".git").exists()
                        {
                            return Ok(toplevel);
                        }
                    }
                }
            }

            let legacy_dir = worktrees_base.join(&tid_owned);
            if legacy_dir.is_dir() && legacy_dir.join(".git").exists() {
                return Ok(legacy_dir);
            }

            let allocator = WorktreeAllocator::new(GitRepository::new(primary_root));
            let mut session =
                allocator.open_session_auto_named(WorkspaceThreadId::from(tid_owned));
            session
                .transition_to_isolated_worktree()
                .map_err(|e| EngineError::Transport(format!("failed to create worktree: {e}")))?;
            Ok(session.working_directory().to_path_buf())
        })
        .await
        .map_err(|e| EngineError::Transport(format!("worktree task panicked: {e}")))??;

        let canonical = std::fs::canonicalize(&worktree_path).map_err(|e| {
            EngineError::Transport(format!(
                "failed to canonicalize worktree {}: {e}",
                worktree_path.display()
            ))
        })?;
        let path_str = canonical
            .to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                EngineError::InvalidParams("worktree path is not valid UTF-8".to_string())
            })?;

        let already_inside_worktree = thread.cwd.as_deref().is_some_and(|cur| {
            let cur_path = Path::new(cur);
            cur_path.starts_with(&canonical)
                || std::fs::canonicalize(cur_path)
                    .map(|c| c.starts_with(&canonical))
                    .unwrap_or(false)
        });
        if !already_inside_worktree {
            if thread.kind == ThreadKind::Terminal {
                let old_dims = self.runtime.teardown_active_pty(thread_id, false).await?;
                self.runtime
                    .storage
                    .update_thread_cwd(&tid, Some(&path_str))
                    .map_err(storage_err)?;
                if let Some((rows, cols)) = old_dims {
                    let _ = self
                        .runtime
                        .respawn_terminal_pty(tid, thread_id, PathBuf::from(&path_str), rows, cols)
                        .await?;
                }
            } else {
                self.runtime
                    .storage
                    .update_thread_cwd(&tid, Some(&path_str))
                    .map_err(storage_err)?;
                self.runtime
                    .reset_thread_acp_session(tid, thread_id)
                    .await?;
            }
        }

        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        Ok(path_str)
    }

    pub(super) async fn list_worktrees_impl(&self) -> Result<Vec<WorktreeEntry>, EngineError> {
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let search_paths = self.runtime.collect_worktree_search_paths(&threads);
        let preferred_hint = self.runtime.cwd.clone();
        let thread_dirs: Vec<(String, PathBuf)> = threads
            .iter()
            .map(|t| (t.id.0.to_string(), self.runtime.effective_thread_dir(t)))
            .collect();

        tokio::task::spawn_blocking(move || {
            let discovered = list_worktrees_across_paths(&search_paths, Some(&preferred_hint));
            let mut attached: Vec<Vec<String>> = vec![Vec::new(); discovered.len()];
            let mut toplevel_cache: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();

            for (tid_str, t_dir) in thread_dirs {
                let toplevel = toplevel_cache
                    .entry(t_dir.clone())
                    .or_insert_with(|| resolve_worktree_toplevel(&t_dir))
                    .as_deref();
                if let Some(idx) =
                    EngineRuntime::longest_matching_worktree_index(&t_dir, toplevel, &discovered)
                {
                    attached[idx].push(tid_str);
                }
            }

            discovered
                .into_iter()
                .zip(attached)
                .map(|(wt, attached_thread_ids)| WorktreeEntry {
                    repo_root: wt.repo_root.to_string_lossy().into_owned(),
                    repo_name: wt.repo_name,
                    path: wt.path.to_string_lossy().into_owned(),
                    branch: wt.branch,
                    head_short: wt.head_short,
                    is_primary: wt.is_primary,
                    is_detached: wt.is_detached,
                    is_grein_managed: wt.is_grein_managed,
                    dirty_count: wt.dirty_count,
                    attached_thread_ids,
                })
                .collect()
        })
        .await
        .map_err(|e| EngineError::Transport(format!("list_worktrees task panicked: {e}")))
    }

    pub(super) async fn switch_thread_worktree_impl(
        &self,
        thread_id: &str,
        target_path: &str,
    ) -> Result<String, EngineError> {
        let trimmed_target = target_path.trim();
        if trimmed_target.is_empty() {
            return Err(EngineError::InvalidParams(
                "target worktree path must not be empty".to_string(),
            ));
        }

        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let _initial = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let thread = self
            .runtime
            .storage
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let search_paths = self.runtime.collect_worktree_search_paths(&threads);
        let preferred_hint = self.runtime.cwd.clone();
        let raw_target = PathBuf::from(trimmed_target);
        let cur_cwd = thread
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        let (canonical_target, changed) = tokio::task::spawn_blocking(move || -> Result<(PathBuf, bool), EngineError> {
            let canonical_target = std::fs::canonicalize(&raw_target).map_err(|_| {
                EngineError::InvalidParams(format!(
                    "target worktree path does not exist: {}",
                    raw_target.display()
                ))
            })?;
            if !canonical_target.is_dir() {
                return Err(EngineError::InvalidParams(format!(
                    "target worktree path is not a directory: {}",
                    canonical_target.display()
                )));
            }
            let discovered = list_worktrees_across_paths(&search_paths, Some(&preferred_hint));
            if !discovered.iter().any(|wt| wt.path == canonical_target) {
                return Err(EngineError::InvalidParams(format!(
                    "path is not a registered worktree: {}",
                    canonical_target.display()
                )));
            }

            let cur_toplevel = cur_cwd
                .as_deref()
                .and_then(|c| resolve_worktree_toplevel(Path::new(c)));
            let changed = cur_toplevel.as_ref() != Some(&canonical_target);
            Ok((canonical_target, changed))
        })
        .await
        .map_err(|e| EngineError::Transport(format!("switch_thread_worktree task panicked: {e}")))??;

        let target_str = canonical_target
            .to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                EngineError::InvalidParams("worktree path is not valid UTF-8".to_string())
            })?;

        if changed {
            if thread.kind == ThreadKind::Terminal {
                let old_dims = self.runtime.teardown_active_pty(thread_id, false).await?;
                self.runtime
                    .storage
                    .update_thread_cwd(&tid, Some(&target_str))
                    .map_err(storage_err)?;
                if let Some((rows, cols)) = old_dims {
                    let _ = self
                        .runtime
                        .respawn_terminal_pty(
                            tid,
                            thread_id,
                            PathBuf::from(&target_str),
                            rows,
                            cols,
                        )
                        .await?;
                }
            } else {
                self.runtime
                    .storage
                    .update_thread_cwd(&tid, Some(&target_str))
                    .map_err(storage_err)?;
                self.runtime
                    .reset_thread_acp_session(tid, thread_id)
                    .await?;
            }
        }

        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        Ok(target_str)
    }

    pub(super) async fn remove_worktree_impl(
        &self,
        path: &str,
        force: bool,
    ) -> Result<(), EngineError> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(EngineError::InvalidParams(
                "worktree path must not be empty".to_string(),
            ));
        }

        let raw_target = PathBuf::from(trimmed);
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let search_paths = self.runtime.collect_worktree_search_paths(&threads);
        let preferred_hint = self.runtime.cwd.clone();

        let (entry, attached_thread_ids) = tokio::task::spawn_blocking(
            move || -> Result<(DiscoveredWorktree, Vec<String>), EngineError> {
                let canonical_target = std::fs::canonicalize(&raw_target).map_err(|_| {
                    EngineError::InvalidParams(format!(
                        "unknown or missing worktree path: {}",
                        raw_target.display()
                    ))
                })?;
                let discovered = list_worktrees_across_paths(&search_paths, Some(&preferred_hint));
                let Some(target_idx) =
                    discovered.iter().position(|wt| wt.path == canonical_target)
                else {
                    return Err(EngineError::InvalidParams(format!(
                        "path is not a registered worktree: {}",
                        canonical_target.display()
                    )));
                };
                let entry = discovered[target_idx].clone();
                if entry.is_primary {
                    return Err(EngineError::InvalidParams(
                        "cannot remove primary repository worktree".to_string(),
                    ));
                }
                if !force
                    && inspect_git_status(&canonical_target).is_some_and(|st| st.dirty_count > 0)
                {
                    return Err(EngineError::InvalidParams(format!(
                        "worktree {} has uncommitted changes; pass force=true to remove",
                        canonical_target.display()
                    )));
                }

                let mut attached = Vec::new();
                for thread in &threads {
                    let Some(raw_cwd) = thread
                        .cwd
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    else {
                        continue;
                    };
                    let raw_path = PathBuf::from(raw_cwd);
                    let canon_cwd = std::fs::canonicalize(&raw_path).unwrap_or(raw_path);
                    let toplevel = resolve_worktree_toplevel(&canon_cwd);
                    let matched_idx = EngineRuntime::longest_matching_worktree_index(
                        &canon_cwd,
                        toplevel.as_deref(),
                        &discovered,
                    );
                    if matched_idx == Some(target_idx)
                        || (matched_idx.is_none() && canon_cwd.starts_with(&canonical_target))
                    {
                        attached.push(thread.id.0.to_string());
                    }
                }
                attached.sort();
                attached.dedup();
                Ok((entry, attached))
            },
        )
        .await
        .map_err(|e| EngineError::Transport(format!("remove_worktree task panicked: {e}")))??;

        let mut turn_guards = Vec::with_capacity(attached_thread_ids.len());
        for tid_str in &attached_thread_ids {
            turn_guards.push(self.runtime.acquire_thread_turn_lock(tid_str).await?);
        }

        let repo_root_str = entry.repo_root.to_string_lossy().into_owned();
        let mut torn_down_ptys: Vec<(ThreadId, String, (u16, u16))> = Vec::new();
        for tid_str in &attached_thread_ids {
            let Some(tid) = parse_typed_id::<ThreadId>(tid_str) else {
                continue;
            };
            let Some(thread) = self.runtime.storage.get_thread(tid).map_err(storage_err)? else {
                continue;
            };
            if thread.kind == ThreadKind::Terminal {
                let old_dims = self.runtime.teardown_active_pty(tid_str, false).await?;
                self.runtime
                    .storage
                    .update_thread_cwd(&tid, Some(&repo_root_str))
                    .map_err(storage_err)?;
                if let Some(dims) = old_dims {
                    torn_down_ptys.push((tid, tid_str.clone(), dims));
                }
            } else {
                self.runtime.reset_thread_acp_session(tid, tid_str).await?;
                self.runtime
                    .storage
                    .update_thread_cwd(&tid, Some(&repo_root_str))
                    .map_err(storage_err)?;
            }
        }

        let repo_root_for_rm = entry.repo_root.clone();
        let target_for_rm = entry.path.clone();
        tokio::task::spawn_blocking(move || {
            workspace_remove_worktree(&repo_root_for_rm, &target_for_rm, force, false)
        })
        .await
        .map_err(|e| EngineError::Transport(format!("remove_worktree task panicked: {e}")))?
        .map_err(|e| EngineError::InvalidParams(format!("failed to remove worktree: {e}")))?;

        for (tid, tid_str, (rows, cols)) in torn_down_ptys {
            let _ = self
                .runtime
                .respawn_terminal_pty(tid, &tid_str, entry.repo_root.clone(), rows, cols)
                .await?;
        }

        drop(turn_guards);
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(TuiEngineEvent::ThreadMetadataUpdated { meta });
        Ok(())
    }

    pub(super) async fn worktree_diff_preview_impl(
        &self,
        path: &str,
    ) -> Result<String, EngineError> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(EngineError::InvalidParams(
                "worktree path must not be empty".to_string(),
            ));
        }
        let target = PathBuf::from(trimmed);
        tokio::task::spawn_blocking(move || workspace_worktree_diff_preview(&target, 400))
            .await
            .map_err(|e| {
                EngineError::Transport(format!("worktree_diff_preview task panicked: {e}"))
            })
    }

    pub(super) async fn prune_worktrees_impl(&self) -> Result<String, EngineError> {
        let repo_hint = self.runtime.cwd.clone();
        tokio::task::spawn_blocking(move || workspace_prune_worktrees(&repo_hint))
            .await
            .map_err(|e| EngineError::Transport(format!("prune_worktrees task panicked: {e}")))?
            .map_err(|e| EngineError::InvalidParams(format!("failed to prune worktrees: {e}")))
    }
}
