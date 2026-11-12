use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeState {
    SharedInspection,
    IsolatedWorktree(PathBuf),
}

#[derive(Debug)]
pub enum WorktreeAllocationError {
    NotAGitRepository(PathBuf),
    GitCommandFailed(String),
    Io(std::io::Error),
}

impl std::fmt::Display for WorktreeAllocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAGitRepository(p) => write!(f, "Not a git repository: {}", p.display()),
            Self::GitCommandFailed(msg) => write!(f, "Git command failed: {msg}"),
            Self::Io(err) => write!(f, "IO error: {err}"),
        }
    }
}

impl std::error::Error for WorktreeAllocationError {}

#[derive(Debug, Clone)]
pub struct LazyWorktreeAllocator {
    repo_root: PathBuf,
    allocated_worktrees: HashMap<ThreadId, PathBuf>,
}

impl LazyWorktreeAllocator {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            allocated_worktrees: HashMap::new(),
        }
    }

    pub fn state_for(&self, thread_id: &ThreadId) -> WorktreeState {
        if let Some(path) = self.allocated_worktrees.get(thread_id) {
            WorktreeState::IsolatedWorktree(path.clone())
        } else {
            WorktreeState::SharedInspection
        }
    }

    pub fn is_allocated(&self, thread_id: &ThreadId) -> bool {
        self.allocated_worktrees.contains_key(thread_id)
    }

    pub fn resolve_path_for_read(
        &self,
        thread_id: &ThreadId,
    ) -> Result<PathBuf, WorktreeAllocationError> {
        if let Some(path) = self.allocated_worktrees.get(thread_id) {
            Ok(path.clone())
        } else {
            Ok(self.repo_root.clone())
        }
    }

    pub fn ensure_allocated_for_mutation(
        &mut self,
        thread_id: &ThreadId,
    ) -> Result<PathBuf, WorktreeAllocationError> {
        if let Some(existing) = self.allocated_worktrees.get(thread_id) {
            return Ok(existing.clone());
        }

        if !self.is_git_repository(&self.repo_root) {
            return Err(WorktreeAllocationError::NotAGitRepository(
                self.repo_root.clone(),
            ));
        }

        let worktree_dir = self
            .repo_root
            .join(".grein")
            .join("worktrees")
            .join(thread_id.as_str());

        if let Some(parent) = worktree_dir.parent() {
            fs::create_dir_all(parent).map_err(WorktreeAllocationError::Io)?;
        }

        let branch_name = format!("grein/{}", thread_id.as_str());
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch_name,
                worktree_dir.to_str().unwrap_or_default(),
                "HEAD",
            ])
            .current_dir(&self.repo_root)
            .output()
            .map_err(WorktreeAllocationError::Io)?;

        if !output.status.success() {
            return Err(WorktreeAllocationError::GitCommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        self.allocated_worktrees
            .insert(thread_id.clone(), worktree_dir.clone());

        Ok(worktree_dir)
    }

    fn is_git_repository(&self, path: &Path) -> bool {
        if !path.exists() {
            return false;
        }
        let output = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(path)
            .output();

        match output {
            Ok(out) => {
                out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true"
            }
            Err(_) => false,
        }
    }
}
