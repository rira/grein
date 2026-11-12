use std::fs;
use std::path::{Path, PathBuf};

/// Detects if running inside an active Git repository with git available in PATH.
pub fn detect_git_environment(path: impl AsRef<Path>) -> bool {
    let p = path.as_ref();
    let git_available = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !git_available {
        return false;
    }

    std::process::Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Manages an ephemeral snapshot directory in non-git environments.
pub struct ShadowSnapshot {
    primary_path: PathBuf,
    shadow_path: PathBuf,
}

impl ShadowSnapshot {
    /// Creates an ephemeral shadow directory at `<primary_dir>/.grein/shadow/<thread_id>`.
    pub fn create(primary_dir: impl AsRef<Path>, thread_id: &str) -> std::io::Result<Self> {
        let primary_path = primary_dir.as_ref().to_path_buf();
        let shadow_path = primary_path.join(".grein/shadow").join(thread_id);

        if shadow_path.exists() {
            fs::remove_dir_all(&shadow_path)?;
        }
        fs::create_dir_all(&shadow_path)?;

        copy_directory_recursive(&primary_path, &shadow_path)?;

        Ok(Self {
            primary_path,
            shadow_path,
        })
    }

    /// Path to the ephemeral shadow snapshot directory.
    pub fn shadow_path(&self) -> &Path {
        &self.shadow_path
    }

    /// Path to the primary workspace directory.
    pub fn primary_path(&self) -> &Path {
        &self.primary_path
    }

    /// Merges verified changes from shadow directory back to primary directory.
    pub fn merge_back(&self) -> std::io::Result<()> {
        copy_directory_recursive(&self.shadow_path, &self.primary_path)
    }

    /// Cleans up and deletes the ephemeral shadow snapshot directory.
    pub fn cleanup(&self) -> std::io::Result<()> {
        if self.shadow_path.exists() {
            fs::remove_dir_all(&self.shadow_path)?;
        }
        Ok(())
    }
}

fn copy_directory_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();

        if name == ".grein" {
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst.join(&name);
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_directory_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = dst_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if dst_path.exists() {
                let _ = fs::remove_file(&dst_path);
            }
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
