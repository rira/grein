#![allow(clippy::needless_borrows_for_generic_args)]

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::git::{GitRepository, MergeResult};
use crate::teardown::{TeardownAction, TeardownOutcome};

const GREIN_EXCLUDE_LINE: &str = ".grein/";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadId(String);

impl ThreadId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Into<String>> From<S> for ThreadId {
    fn from(value: S) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeMode {
    SharedRead,
    IsolatedWorktree,
}

#[derive(Debug, Clone)]
pub struct WorktreeAllocator {
    repo: GitRepository,
}

impl WorktreeAllocator {
    pub fn new(repo: GitRepository) -> Self {
        Self { repo }
    }

    pub fn open_session(&self, thread_id: ThreadId) -> Result<WorktreeSession, WorkspaceError> {
        let worktree_dir = self
            .repo
            .root()
            .join(".grein")
            .join("worktrees")
            .join(thread_id.as_str());
        let branch_name = format!("worktree-{}", thread_id.as_str());

        Ok(WorktreeSession {
            repo: self.repo.clone(),
            thread_id,
            worktree_dir,
            branch_name,
            mode: WorktreeMode::SharedRead,
        })
    }

    pub fn open_session_auto_named(&self, thread_id: ThreadId) -> WorktreeSession {
        let primary_root = crate::worktrees::resolve_primary_repo_root(self.repo.root())
            .unwrap_or_else(|| self.repo.root().to_path_buf());

        let (worktree_dir, branch_name) =
            match crate::worktrees::current_branch_name(self.repo.root()) {
                Some(branch) if branch != "main" && branch != "master" => {
                    let slug = crate::worktrees::sanitize_worktree_dir_slug(&branch);
                    let base_branch = format!("{branch}-wt");
                    let registered_paths: HashSet<PathBuf> =
                        crate::worktrees::list_repo_worktrees(&primary_root)
                            .into_iter()
                            .map(|wt| wt.path)
                            .collect();

                    let mut n = 1usize;
                    loop {
                        let dir_name = if n == 1 {
                            slug.clone()
                        } else {
                            format!("{slug}-{n}")
                        };
                        let candidate_branch = if n == 1 {
                            base_branch.clone()
                        } else {
                            format!("{base_branch}-{n}")
                        };
                        let candidate_path =
                            primary_root.join(".grein").join("worktrees").join(&dir_name);

                        let is_registered = registered_paths.contains(&candidate_path)
                            || fs::canonicalize(&candidate_path)
                                .map(|c| registered_paths.contains(&c))
                                .unwrap_or(false);

                        if !candidate_path.exists()
                            && !is_registered
                            && !branch_ref_exists(&primary_root, &candidate_branch)
                        {
                            break (candidate_path, candidate_branch);
                        }
                        n += 1;
                    }
                }
                _ => (
                    primary_root
                        .join(".grein")
                        .join("worktrees")
                        .join(thread_id.as_str()),
                    format!("worktree-{}", thread_id.as_str()),
                ),
            };

        WorktreeSession {
            repo: GitRepository::new(primary_root),
            thread_id,
            worktree_dir,
            branch_name,
            mode: WorktreeMode::SharedRead,
        }
    }
}

fn branch_ref_exists(repo_root: &Path, branch_name: &str) -> bool {
    let ref_name = format!("refs/heads/{branch_name}");
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["show-ref", "--verify", "--quiet", &ref_name])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[derive(Debug)]
pub struct WorktreeSession {
    repo: GitRepository,
    #[allow(dead_code)]
    thread_id: ThreadId,
    worktree_dir: PathBuf,
    branch_name: String,
    mode: WorktreeMode,
}

pub type Session = WorktreeSession;

impl WorktreeSession {
    pub fn mode(&self) -> WorktreeMode {
        self.mode
    }

    pub fn working_directory(&self) -> &Path {
        match self.mode {
            WorktreeMode::SharedRead => self.repo.root(),
            WorktreeMode::IsolatedWorktree => &self.worktree_dir,
        }
    }

    pub fn branch_name(&self) -> &str {
        &self.branch_name
    }

    pub fn worktree_dir(&self) -> &Path {
        &self.worktree_dir
    }

    /// Resolves the git info exclude file, preferring `git rev-parse --git-path`.
    fn exclude_file_path(&self) -> PathBuf {
        let root = self.repo.root();
        let resolved = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--git-path", "info/exclude"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        match resolved {
            Some(p) => {
                let path = PathBuf::from(p);
                if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                }
            }
            None => root.join(".git").join("info").join("exclude"),
        }
    }

    /// Ensures a `.grein/` line exists in the git info exclude file (idempotent).
    fn ensure_grein_excluded(&self) -> Result<(), WorkspaceError> {
        let exclude_path = self.exclude_file_path();
        let existing = fs::read_to_string(&exclude_path).unwrap_or_default();

        if existing.lines().any(|l| l.trim() == GREIN_EXCLUDE_LINE) {
            return Ok(());
        }

        if let Some(parent) = exclude_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude_path)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        file.write_all(GREIN_EXCLUDE_LINE.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }

    fn stage_all(&self, dir: &Path) -> Result<(), WorkspaceError> {
        self.ensure_grein_excluded()?;

        let add_output = Command::new("git")
            .current_dir(dir)
            .args(["add", "-A"])
            .output()?;

        if !add_output.status.success() {
            let stderr = String::from_utf8_lossy(&add_output.stderr).to_string();
            return Err(WorkspaceError::GitError(stderr));
        }

        // Best-effort: unstage anything under .grein; non-zero exit is ignored.
        let _ = Command::new("git")
            .current_dir(dir)
            .args(["reset", "-q", "--", ".grein"])
            .output();

        Ok(())
    }

    fn remove_worktree_and_branch(&self) -> Result<(), WorkspaceError> {
        let worktree_path_str = self
            .worktree_dir
            .to_str()
            .ok_or(WorkspaceError::InvalidPath)?;

        let _ = Command::new("git")
            .current_dir(self.repo.root())
            .args(["worktree", "remove", "--force", worktree_path_str])
            .output();

        let _ = Command::new("git")
            .current_dir(self.repo.root())
            .args(["branch", "-D", self.branch_name.as_str()])
            .output();

        if self.worktree_dir.exists() {
            let _ = fs::remove_dir_all(&self.worktree_dir);
        }
        Ok(())
    }

    pub fn transition_to_isolated_worktree(&mut self) -> Result<(), WorkspaceError> {
        if self.mode == WorktreeMode::IsolatedWorktree {
            return Ok(());
        }

        self.ensure_grein_excluded()?;

        if let Some(parent) = self.worktree_dir.parent() {
            fs::create_dir_all(parent)?;
        }

        let worktree_path_str = self
            .worktree_dir
            .to_str()
            .ok_or(WorkspaceError::InvalidPath)?;

        let output = Command::new("git")
            .current_dir(self.repo.root())
            .args([
                "worktree",
                "add",
                "-B",
                self.branch_name.as_str(),
                worktree_path_str,
                "HEAD",
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(WorkspaceError::GitError(stderr));
        }

        self.mode = WorktreeMode::IsolatedWorktree;
        Ok(())
    }

    pub fn commit(&self, message: &str) -> Result<(), WorkspaceError> {
        let dir = self.working_directory();

        self.stage_all(dir)?;

        let commit_output = Command::new("git")
            .current_dir(dir)
            .args(["commit", "-m", message])
            .output()?;

        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr).to_string();
            return Err(WorkspaceError::GitError(stderr));
        }

        Ok(())
    }

    pub fn merge_to_primary(&self) -> Result<MergeResult, WorkspaceError> {
        let output = Command::new("git")
            .current_dir(self.repo.root())
            .args(["merge", self.branch_name.as_str()])
            .output()?;

        if output.status.success() {
            Ok(MergeResult::Success)
        } else {
            let diff_output = Command::new("git")
                .current_dir(self.repo.root())
                .args(["diff", "--name-only", "--diff-filter=U"])
                .output()?;

            let files_str = String::from_utf8_lossy(&diff_output.stdout);
            let conflicting_files: Vec<String> = files_str
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();

            let _ = Command::new("git")
                .current_dir(self.repo.root())
                .args(["merge", "--abort"])
                .output();

            Ok(MergeResult::Conflict { conflicting_files })
        }
    }

    pub fn close(&mut self, action: TeardownAction) -> Result<TeardownOutcome, WorkspaceError> {
        match action {
            TeardownAction::AutoPruneClean => {
                if self.mode == WorktreeMode::IsolatedWorktree {
                    let status_output = Command::new("git")
                        .current_dir(&self.worktree_dir)
                        .args(["status", "--porcelain"])
                        .output()?;

                    let stdout = String::from_utf8_lossy(&status_output.stdout);
                    if stdout.trim().is_empty() {
                        self.remove_worktree_and_branch()?;
                        self.mode = WorktreeMode::SharedRead;
                        Ok(TeardownOutcome::Pruned)
                    } else {
                        Ok(TeardownOutcome::Kept)
                    }
                } else {
                    Ok(TeardownOutcome::Pruned)
                }
            }
            TeardownAction::Keep => Ok(TeardownOutcome::Kept),
            TeardownAction::Discard => {
                if self.mode == WorktreeMode::IsolatedWorktree {
                    self.remove_worktree_and_branch()?;
                }
                Ok(TeardownOutcome::Discarded)
            }
            TeardownAction::Merge => match self.merge_to_primary()? {
                MergeResult::Success => {
                    if self.mode == WorktreeMode::IsolatedWorktree {
                        self.remove_worktree_and_branch()?;
                        self.mode = WorktreeMode::SharedRead;
                    }
                    Ok(TeardownOutcome::Merged)
                }
                MergeResult::Conflict { conflicting_files } => {
                    Err(WorkspaceError::MergeConflict(conflicting_files))
                }
            },
        }
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    Io(std::io::Error),
    GitError(String),
    InvalidPath,
    MergeConflict(Vec<String>),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::GitError(msg) => write!(f, "Git error: {msg}"),
            Self::InvalidPath => write!(f, "Invalid worktree path"),
            Self::MergeConflict(files) => {
                write!(f, "Merge conflict in files: {}", files.join(", "))
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<std::io::Error> for WorkspaceError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}
