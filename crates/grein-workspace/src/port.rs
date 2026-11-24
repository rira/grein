use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use grein_core::port::dto::{
    PtyScreenSnapshot, ShellCommandResult,
};

use crate::allocator::{ThreadId, WorkspaceError, WorktreeAllocator};
use crate::diff::compute_directory_diff;
use crate::files::list_workspace_files;
use crate::git::{GitError, GitRepository};
use crate::git_status::{inspect_git_status, RepoGitStatus};
use crate::pty_session::InteractivePtySession;
use crate::shadow::ShadowSnapshot;
use crate::shell::run_shell_command_with_pty_blocking;
use crate::worktrees::{
    list_worktrees_across_paths, prune_worktrees as git_prune_worktrees,
    remove_worktree as git_remove_worktree, resolve_primary_repo_root, resolve_worktree_toplevel,
    worktree_diff_preview, DiscoveredWorktree,
};

/// Outbound port for managing isolated per-thread workspace directories (git worktrees or shadow copies).
pub trait WorkspaceIsolationPort: Send + Sync {
    fn open_thread_worktree(
        &self,
        workspace_root: &Path,
        thread_id: &str,
        current_cwd: Option<&Path>,
    ) -> Result<PathBuf, WorkspaceError>;

    fn list_worktrees(
        &self,
        search_paths: &[PathBuf],
        preferred_repo_hint: Option<&Path>,
    ) -> Vec<DiscoveredWorktree>;

    fn diff_preview(&self, path: &Path, max_lines: usize) -> String;

    fn remove_worktree(
        &self,
        repo_root: &Path,
        target_path: &Path,
        force: bool,
        delete_branch: bool,
    ) -> Result<Option<String>, WorkspaceError>;

    fn prune_worktrees(&self, repo_hint: &Path) -> Result<String, WorkspaceError>;
}

/// Git-worktree implementation of [`WorkspaceIsolationPort`] backed by [`WorktreeAllocator`] and [`crate::worktrees`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GitWorktreeDriver;

impl GitWorktreeDriver {
    pub fn new() -> Self {
        Self
    }
}

impl WorkspaceIsolationPort for GitWorktreeDriver {
    fn open_thread_worktree(
        &self,
        workspace_root: &Path,
        thread_id: &str,
        current_cwd: Option<&Path>,
    ) -> Result<PathBuf, WorkspaceError> {
        let primary_root = match resolve_primary_repo_root(workspace_root) {
            Some(root) => root,
            None => {
                let repo = GitRepository::discover(workspace_root).map_err(|err| match err {
                    GitError::NotFound => WorkspaceError::GitError(format!(
                        "workspace {} is not inside a git repository",
                        workspace_root.display()
                    )),
                    GitError::CommandFailed(msg) => WorkspaceError::GitError(msg),
                })?;
                fs::canonicalize(repo.root()).unwrap_or_else(|_| repo.root().to_path_buf())
            }
        };

        let worktrees_base = primary_root.join(".grein").join("worktrees");
        let canonical_wt_base =
            fs::canonicalize(&worktrees_base).unwrap_or_else(|_| worktrees_base.clone());

        if let Some(cur_path) = current_cwd.filter(|p| p.is_dir()) {
            if let Some(toplevel) = resolve_worktree_toplevel(cur_path) {
                if toplevel.starts_with(&canonical_wt_base) && toplevel.join(".git").exists() {
                    return Ok(toplevel);
                }
            }
        }

        let legacy_dir = worktrees_base.join(thread_id);
        if legacy_dir.is_dir() && legacy_dir.join(".git").exists() {
            return Ok(fs::canonicalize(&legacy_dir).unwrap_or(legacy_dir));
        }

        let allocator = WorktreeAllocator::new(GitRepository::new(primary_root));
        let mut session = allocator.open_session_auto_named(ThreadId::from(thread_id));
        session.transition_to_isolated_worktree()?;
        let dir = session.working_directory().to_path_buf();
        Ok(fs::canonicalize(&dir).unwrap_or(dir))
    }

    fn list_worktrees(
        &self,
        search_paths: &[PathBuf],
        preferred_repo_hint: Option<&Path>,
    ) -> Vec<DiscoveredWorktree> {
        list_worktrees_across_paths(search_paths, preferred_repo_hint)
    }

    fn diff_preview(&self, path: &Path, max_lines: usize) -> String {
        worktree_diff_preview(path, max_lines)
    }

    fn remove_worktree(
        &self,
        repo_root: &Path,
        target_path: &Path,
        force: bool,
        delete_branch: bool,
    ) -> Result<Option<String>, WorkspaceError> {
        git_remove_worktree(repo_root, target_path, force, delete_branch)
    }

    fn prune_worktrees(&self, repo_hint: &Path) -> Result<String, WorkspaceError> {
        git_prune_worktrees(repo_hint)
    }
}

/// Non-git shadow-directory isolator implementing [`WorkspaceIsolationPort`] via [`ShadowSnapshot`] and [`crate::diff`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ShadowCopyIsolator;

impl ShadowCopyIsolator {
    pub fn new() -> Self {
        Self
    }

    fn validate_thread_id(thread_id: &str) -> Result<&str, WorkspaceError> {
        let trimmed = thread_id.trim();
        if trimmed.is_empty()
            || trimmed == "."
            || trimmed == ".."
            || trimmed.contains(['/', '\\'])
        {
            return Err(WorkspaceError::InvalidPath);
        }
        Ok(trimmed)
    }

    fn resolve_shadow_pair(path: &Path) -> Option<(PathBuf, PathBuf)> {
        let canon = fs::canonicalize(path).ok()?;
        let mut cur: Option<&Path> = Some(canon.as_path());
        while let Some(node) = cur {
            let parent = node.parent()?;
            if parent.file_name().and_then(|n| n.to_str()) == Some("shadow") {
                let grein_dir = parent.parent()?;
                if grein_dir.file_name().and_then(|n| n.to_str()) == Some(".grein") {
                    let primary = grein_dir.parent()?;
                    return Some((primary.to_path_buf(), node.to_path_buf()));
                }
            }
            cur = Some(parent);
        }
        None
    }

    fn resolve_primary_dir(path: &Path) -> Option<PathBuf> {
        if let Some((primary, _)) = Self::resolve_shadow_pair(path) {
            return Some(primary);
        }
        if path.is_dir() {
            fs::canonicalize(path).ok()
        } else {
            None
        }
    }

    fn truncate_preview(preview: String, max_lines: usize) -> String {
        let limit = max_lines.max(1);
        let lines: Vec<&str> = preview.lines().collect();
        if lines.len() > limit {
            let remaining = lines.len() - limit;
            let mut truncated = lines[..limit].join("\n");
            truncated.push_str(&format!("\n... ({remaining} more lines truncated)\n"));
            return truncated;
        }
        preview
    }

    fn count_diff_files(primary: &Path, shadow: &Path) -> usize {
        compute_directory_diff(primary, shadow)
            .map(|diff| {
                diff.lines()
                    .filter(|line| line.starts_with("--- a/") || line.starts_with("Binary files "))
                    .count()
            })
            .unwrap_or(0)
    }
}

impl WorkspaceIsolationPort for ShadowCopyIsolator {
    fn open_thread_worktree(
        &self,
        workspace_root: &Path,
        thread_id: &str,
        current_cwd: Option<&Path>,
    ) -> Result<PathBuf, WorkspaceError> {
        let tid = Self::validate_thread_id(thread_id)?;
        let primary_root = Self::resolve_primary_dir(workspace_root)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "workspace root not found"))?;
        let shadow_base = primary_root.join(".grein").join("shadow");
        let canonical_shadow_base =
            fs::canonicalize(&shadow_base).unwrap_or_else(|_| shadow_base.clone());

        if let Some(cur_path) = current_cwd.filter(|p| p.is_dir()) {
            if let Some((cur_primary, shadow_root)) = Self::resolve_shadow_pair(cur_path) {
                if cur_primary == primary_root && shadow_root.starts_with(&canonical_shadow_base) {
                    return Ok(shadow_root);
                }
            }
        }

        let existing = shadow_base.join(tid);
        if existing.is_dir() {
            return Ok(fs::canonicalize(&existing).unwrap_or(existing));
        }

        let snapshot = ShadowSnapshot::create(&primary_root, tid)?;
        Ok(fs::canonicalize(snapshot.shadow_path())
            .unwrap_or_else(|_| snapshot.shadow_path().to_path_buf()))
    }

    fn list_worktrees(
        &self,
        search_paths: &[PathBuf],
        preferred_repo_hint: Option<&Path>,
    ) -> Vec<DiscoveredWorktree> {
        let preferred_root = preferred_repo_hint.and_then(Self::resolve_primary_dir);

        let mut seen_roots = HashSet::new();
        for path in search_paths.iter().map(PathBuf::as_path).chain(preferred_repo_hint) {
            if let Some(root) = Self::resolve_primary_dir(path) {
                seen_roots.insert(root);
            }
        }

        let mut entries = Vec::new();
        for repo_root in seen_roots {
            let repo_name = path_basename(&repo_root);
            entries.push(DiscoveredWorktree {
                repo_root: repo_root.clone(),
                repo_name: repo_name.clone(),
                path: repo_root.clone(),
                branch: None,
                head_short: None,
                is_primary: true,
                is_detached: false,
                is_grein_managed: false,
                dirty_count: 0,
            });

            let shadow_base = repo_root.join(".grein").join("shadow");
            if let Ok(read_dir) = fs::read_dir(&shadow_base) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let Ok(canon_path) = fs::canonicalize(&path) else {
                        continue;
                    };
                    let dir_name = path_basename(&canon_path);
                    let dirty_count = Self::count_diff_files(&repo_root, &canon_path);
                    entries.push(DiscoveredWorktree {
                        repo_root: repo_root.clone(),
                        repo_name: repo_name.clone(),
                        path: canon_path,
                        branch: Some(format!("shadow-{dir_name}")),
                        head_short: None,
                        is_primary: false,
                        is_detached: false,
                        is_grein_managed: true,
                        dirty_count,
                    });
                }
            }
        }

        entries.sort_by(|a, b| {
            (
                preferred_root.as_deref() != Some(a.repo_root.as_path()),
                a.repo_name.to_ascii_lowercase(),
                &a.repo_root,
                !a.is_primary,
                &a.path,
            )
                .cmp(&(
                    preferred_root.as_deref() != Some(b.repo_root.as_path()),
                    b.repo_name.to_ascii_lowercase(),
                    &b.repo_root,
                    !b.is_primary,
                    &b.path,
                ))
        });

        entries
    }

    fn diff_preview(&self, path: &Path, max_lines: usize) -> String {
        if !path.is_dir() {
            return "Worktree directory not found\n".to_string();
        }
        let Some((primary_root, shadow_root)) = Self::resolve_shadow_pair(path) else {
            return "Not a shadow workspace\n".to_string();
        };
        match compute_directory_diff(&primary_root, &shadow_root) {
            Ok(diff) if diff.trim().is_empty() => {
                "Clean working tree (no uncommitted changes)\n".to_string()
            }
            Ok(diff) => Self::truncate_preview(diff, max_lines),
            Err(err) => format!("Failed to compute shadow diff: {err}\n"),
        }
    }

    fn remove_worktree(
        &self,
        repo_root: &Path,
        target_path: &Path,
        force: bool,
        _delete_branch: bool,
    ) -> Result<Option<String>, WorkspaceError> {
        let primary_root = Self::resolve_primary_dir(repo_root).ok_or_else(|| {
            WorkspaceError::GitError(format!("Not a workspace directory: {}", repo_root.display()))
        })?;
        let canonical_target = fs::canonicalize(target_path).map_err(|_| {
            WorkspaceError::GitError(format!(
                "Unknown or missing shadow path: {}",
                target_path.display()
            ))
        })?;
        if canonical_target == primary_root {
            return Err(WorkspaceError::GitError(
                "Cannot remove the primary worktree".to_string(),
            ));
        }
        let shadow_base = fs::canonicalize(primary_root.join(".grein").join("shadow"))
            .unwrap_or_else(|_| primary_root.join(".grein").join("shadow"));
        if canonical_target.parent() != Some(shadow_base.as_path()) || !canonical_target.is_dir() {
            return Err(WorkspaceError::GitError(format!(
                "Path is not a managed shadow workspace of {}: {}",
                primary_root.display(),
                canonical_target.display()
            )));
        }
        if !force {
            let diff = compute_directory_diff(&primary_root, &canonical_target)?;
            if !diff.trim().is_empty() {
                return Err(WorkspaceError::GitError(format!(
                    "shadow workspace {} has uncommitted changes; pass force=true to remove",
                    canonical_target.display()
                )));
            }
        }
        fs::remove_dir_all(&canonical_target)?;
        Ok(None)
    }

    fn prune_worktrees(&self, repo_hint: &Path) -> Result<String, WorkspaceError> {
        let primary_root = Self::resolve_primary_dir(repo_hint).ok_or_else(|| {
            WorkspaceError::GitError(format!("Not a workspace directory: {}", repo_hint.display()))
        })?;
        let shadow_base = primary_root.join(".grein").join("shadow");
        if shadow_base.is_dir() && fs::read_dir(&shadow_base)?.next().is_none() {
            let _ = fs::remove_dir(&shadow_base);
        }
        Ok("Shadow workspaces clean (nothing to prune)".to_string())
    }
}

fn path_basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Outbound port for read-only git repository inspection and tracked/untracked file discovery.
pub trait GitInspectorPort: Send + Sync {
    fn resolve_primary_repo_root(&self, path: &Path) -> Option<PathBuf>;

    fn resolve_worktree_toplevel(&self, path: &Path) -> Option<PathBuf>;

    fn inspect_git_status(&self, path: &Path) -> Option<RepoGitStatus>;

    fn list_workspace_files(&self, root: &Path, max_entries: usize) -> Vec<String>;
}

/// CLI `git` implementation of [`GitInspectorPort`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GitCliInspector;

impl GitCliInspector {
    pub fn new() -> Self {
        Self
    }
}

impl GitInspectorPort for GitCliInspector {
    fn resolve_primary_repo_root(&self, path: &Path) -> Option<PathBuf> {
        resolve_primary_repo_root(path)
    }

    fn resolve_worktree_toplevel(&self, path: &Path) -> Option<PathBuf> {
        resolve_worktree_toplevel(path)
    }

    fn inspect_git_status(&self, path: &Path) -> Option<RepoGitStatus> {
        inspect_git_status(path)
    }

    fn list_workspace_files(&self, root: &Path, max_entries: usize) -> Vec<String> {
        list_workspace_files(root, max_entries)
    }
}

/// Outbound handle to a running interactive PTY terminal session.
pub trait TerminalPtyHandle: Send + Sync {
    fn write_bytes(&self, bytes: &[u8]) -> io::Result<()>;

    fn resize(&self, rows: u16, cols: u16) -> io::Result<()>;

    fn scroll(&self, delta: isize) -> usize;

    fn set_scrollback(&self, offset: usize) -> usize;

    fn snapshot(&self) -> PtyScreenSnapshot;

    fn restart_in_cwd(&self, cwd: &Path) -> io::Result<Arc<dyn TerminalPtyHandle>>;

    fn is_exited(&self) -> bool;

    fn child_pid(&self) -> u32;

    fn live_cwd(&self) -> Option<PathBuf>;

    fn poll_os_cwd(&self) -> Option<String>;

    fn terminal_title(&self) -> Option<String>;
}

impl TerminalPtyHandle for InteractivePtySession {
    fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        InteractivePtySession::write_bytes(self, bytes)
    }

    fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        InteractivePtySession::resize(self, rows, cols)
    }

    fn scroll(&self, delta: isize) -> usize {
        InteractivePtySession::scroll_by(self, delta)
    }

    fn set_scrollback(&self, offset: usize) -> usize {
        InteractivePtySession::set_scrollback(self, offset)
    }

    fn snapshot(&self) -> PtyScreenSnapshot {
        InteractivePtySession::snapshot(self)
    }

    fn restart_in_cwd(&self, cwd: &Path) -> io::Result<Arc<dyn TerminalPtyHandle>> {
        InteractivePtySession::restart_in_cwd(self, cwd)
            .map(|session| Arc::new(session) as Arc<dyn TerminalPtyHandle>)
    }

    fn is_exited(&self) -> bool {
        InteractivePtySession::is_exited(self)
    }

    fn child_pid(&self) -> u32 {
        InteractivePtySession::child_pid(self)
    }

    fn live_cwd(&self) -> Option<PathBuf> {
        InteractivePtySession::live_cwd(self)
    }

    fn poll_os_cwd(&self) -> Option<String> {
        InteractivePtySession::poll_os_cwd(self)
    }

    fn terminal_title(&self) -> Option<String> {
        InteractivePtySession::terminal_title(self)
    }
}

/// Outbound factory port for spawning interactive PTY sessions.
pub trait PtySpawnerPort: Send + Sync {
    fn spawn_with_env(
        &self,
        cwd: &Path,
        rows: u16,
        cols: u16,
        shell: Option<&str>,
        extra_env: &[(&str, &str)],
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Arc<dyn TerminalPtyHandle>>;
}

/// Unix PTY implementation of [`PtySpawnerPort`] backed by [`InteractivePtySession`].
#[derive(Debug, Default, Clone, Copy)]
pub struct UnixPtySpawner;

impl UnixPtySpawner {
    pub fn new() -> Self {
        Self
    }
}

impl PtySpawnerPort for UnixPtySpawner {
    fn spawn_with_env(
        &self,
        cwd: &Path,
        rows: u16,
        cols: u16,
        shell: Option<&str>,
        extra_env: &[(&str, &str)],
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Arc<dyn TerminalPtyHandle>> {
        let session =
            InteractivePtySession::spawn_with_env(cwd, rows, cols, shell, extra_env, on_update)?;
        Ok(Arc::new(session))
    }
}

/// Outbound port for executing one-shot shell commands with bounded output and timeout.
pub trait ShellExecutorPort: Send + Sync {
    fn run_shell_command(
        &self,
        cwd: &Path,
        command: &str,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> io::Result<ShellCommandResult>;
}

/// PTY-backed implementation of [`ShellExecutorPort`] delegating to [`run_shell_command_with_pty_blocking`].
#[derive(Debug, Default, Clone, Copy)]
pub struct PtyShellExecutor;

impl PtyShellExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl ShellExecutorPort for PtyShellExecutor {
    fn run_shell_command(
        &self,
        cwd: &Path,
        command: &str,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> io::Result<ShellCommandResult> {
        let out = run_shell_command_with_pty_blocking(cwd, command, timeout, max_output_bytes)?;
        Ok(ShellCommandResult {
            command: out.command,
            stdout: out.stdout,
            stderr: out.stderr,
            exit_code: out.exit_code,
            timed_out: out.timed_out,
        })
    }
}
