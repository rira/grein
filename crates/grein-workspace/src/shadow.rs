use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

    /// Merges verified changes (additions, modifications and deletions) from the
    /// shadow directory back to the primary directory.
    pub fn merge_back(&self) -> std::io::Result<()> {
        copy_directory_recursive(&self.shadow_path, &self.primary_path)?;
        remove_deleted_entries(&self.primary_path, &self.shadow_path, &self.primary_path)?;
        Ok(())
    }

    /// Cleans up and deletes the ephemeral shadow snapshot directory.
    pub fn cleanup(&self) -> std::io::Result<()> {
        if self.shadow_path.exists() {
            fs::remove_dir_all(&self.shadow_path)?;
        }
        Ok(())
    }
}

fn is_protected_name(name: &std::ffi::OsStr) -> bool {
    name == ".grein" || name == ".git"
}

/// Removes files in `cur` (under `primary_root`) that no longer exist in the shadow
/// tree, pruning directories emptied by those removals. Returns whether anything
/// was removed beneath `cur`.
fn remove_deleted_entries(
    primary_root: &Path,
    shadow_root: &Path,
    cur: &Path,
) -> std::io::Result<bool> {
    let mut removed_any = false;

    for entry in fs::read_dir(cur)? {
        let entry = entry?;
        if is_protected_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            let removed = remove_deleted_entries(primary_root, shadow_root, &path)?;
            if removed {
                removed_any = true;
                if fs::read_dir(&path)?.next().is_none() {
                    fs::remove_dir(&path)?;
                }
            }
        } else if file_type.is_file() {
            let rel = match path.strip_prefix(primary_root) {
                Ok(rel) => rel,
                Err(_) => continue,
            };
            if fs::symlink_metadata(shadow_root.join(rel)).is_err() {
                fs::remove_file(&path)?;
                removed_any = true;
            }
        }
    }

    Ok(removed_any)
}

/// Copies a single file, attempting a copy-on-write reflink clone on Linux and
/// falling back to a plain `fs::copy`. Never creates hard links.
fn copy_file_cow(src: &Path, dst: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(dst).is_ok() {
        // Remove first so existing links/targets are never written through.
        fs::remove_file(dst)?;
    }

    if cfg!(target_os = "linux") {
        let cloned = Command::new("cp")
            .args(["--reflink=auto", "--preserve=mode", "--"])
            .arg(src)
            .arg(dst)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if cloned {
            return Ok(());
        }
        if fs::symlink_metadata(dst).is_ok() {
            let _ = fs::remove_file(dst);
        }
    }

    fs::copy(src, dst).map(|_| ())
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
            copy_file_cow(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
