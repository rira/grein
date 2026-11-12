#![allow(clippy::needless_borrows_for_generic_args)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::git::{GitRepository, MergeResult};
use crate::teardown::{TeardownAction, TeardownOutcome};

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

    pub fn transition_to_isolated_worktree(&mut self) -> Result<(), WorkspaceError> {
        if self.mode == WorktreeMode::IsolatedWorktree {
            return Ok(());
        }

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

        let add_output = Command::new("git")
            .current_dir(dir)
            .args(["add", "-A"])
            .output()?;

        if !add_output.status.success() {
            let stderr = String::from_utf8_lossy(&add_output.stderr).to_string();
            return Err(WorkspaceError::GitError(stderr));
        }

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
                }
                Ok(TeardownOutcome::Discarded)
            }
            TeardownAction::Merge => {
                let _ = self.merge_to_primary()?;
                Ok(TeardownOutcome::Merged)
            }
        }
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    Io(std::io::Error),
    GitError(String),
    InvalidPath,
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::GitError(msg) => write!(f, "Git error: {msg}"),
            Self::InvalidPath => write!(f, "Invalid worktree path"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<std::io::Error> for WorkspaceError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}
