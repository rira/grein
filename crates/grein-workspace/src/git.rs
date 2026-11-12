use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepository {
    root: PathBuf,
}

impl GitRepository {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn discover(path: impl AsRef<Path>) -> Result<Self, GitError> {
        let mut current = path.as_ref().to_path_buf();
        loop {
            if current.join(".git").exists() {
                return Ok(Self { root: current });
            }
            if !current.pop() {
                return Err(GitError::NotFound);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    Success,
    Conflict { conflicting_files: Vec<String> },
}

#[derive(Debug)]
pub enum GitError {
    NotFound,
    CommandFailed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Git repository not found"),
            Self::CommandFailed(msg) => write!(f, "Git command failed: {msg}"),
        }
    }
}

impl std::error::Error for GitError {}
