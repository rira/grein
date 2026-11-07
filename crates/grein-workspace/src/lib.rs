//! grein-workspace: Git worktree manager and shadow snapshot fallback.

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkspaceError {
    #[error("Git command failed: {0}")]
    Git(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Workspace path invalid: {0}")]
    InvalidPath(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceMode {
    GitWorktree { worktree_path: PathBuf },
    ShadowSnapshot { shadow_path: PathBuf },
}

pub struct WorkspaceManager {
    root_dir: PathBuf,
    is_git: bool,
}

impl WorkspaceManager {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        let root = root_dir.as_ref().to_path_buf();
        let is_git = root.join(".git").exists();
        Self {
            root_dir: root,
            is_git,
        }
    }

    pub fn is_git(&self) -> bool {
        self.is_git
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }
}
