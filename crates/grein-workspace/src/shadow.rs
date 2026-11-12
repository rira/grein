use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ShadowError {
    SourceNotFound(PathBuf),
    Io(io::Error),
}

impl fmt::Display for ShadowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShadowError::SourceNotFound(path) => {
                write!(f, "source path not found: {}", path.display())
            }
            ShadowError::Io(err) => write!(f, "I/O error: {}", err),
        }
    }
}

impl std::error::Error for ShadowError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ShadowError::SourceNotFound(_) => None,
            ShadowError::Io(err) => Some(err),
        }
    }
}

impl From<io::Error> for ShadowError {
    fn from(err: io::Error) -> Self {
        ShadowError::Io(err)
    }
}

#[derive(Debug)]
pub struct ShadowWorkspace {
    path: PathBuf,
}

impl ShadowWorkspace {
    pub fn create(root: impl AsRef<Path>, thread_id: &str) -> Result<Self, ShadowError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(ShadowError::SourceNotFound(root.to_path_buf()));
        }

        let shadow_path = root.join(".grein").join("shadow").join(thread_id);
        if shadow_path.exists() {
            fs::remove_dir_all(&shadow_path)?;
        }
        fs::create_dir_all(&shadow_path)?;

        copy_tree(root, &shadow_path, root)?;

        Ok(Self { path: shadow_path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cleanup(&self) -> Result<(), ShadowError> {
        if self.path.exists() {
            fs::remove_dir_all(&self.path)?;
        }
        Ok(())
    }
}

fn copy_tree(src: &Path, dst: &Path, root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_name = entry.file_name();

        if src == root && file_name == ".grein" {
            continue;
        }

        let entry_path = entry.path();
        let target_path = dst.join(&file_name);
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            fs::create_dir_all(&target_path)?;
            copy_tree(&entry_path, &target_path, root)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if target_path.exists() {
                fs::remove_file(&target_path)?;
            }
            fs::hard_link(&entry_path, &target_path)?;
        }
    }
    Ok(())
}
