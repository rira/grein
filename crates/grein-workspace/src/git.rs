use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    NotAGitRepository { path: PathBuf },
    Io(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAGitRepository { path } => {
                write!(f, "not a git repository: {}", path.display())
            }
            Self::Io(msg) => write!(f, "git io error: {msg}"),
        }
    }
}

impl std::error::Error for GitError {}

pub fn discover_repository_root(start_path: &Path) -> Result<PathBuf, GitError> {
    let mut current = start_path;
    loop {
        let git_marker = current.join(".git");
        if git_marker.exists() {
            return Ok(current.to_path_buf());
        }

        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    Err(GitError::NotAGitRepository {
        path: start_path.to_path_buf(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWorkspace {
    root_path: PathBuf,
    read_only: bool,
    isolated: bool,
}

impl SharedWorkspace {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root_path,
            read_only: true,
            isolated: false,
        }
    }

    pub fn detect(path: &Path) -> Result<Self, GitError> {
        let root = discover_repository_root(path)?;
        Ok(Self::new(root))
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn is_isolated(&self) -> bool {
        self.isolated
    }
}
