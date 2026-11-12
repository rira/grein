use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Represents an ephemeral shadow snapshot of a workspace directory.
#[derive(Debug, Clone)]
pub struct ShadowSnapshot {
    primary_path: PathBuf,
    shadow_path: PathBuf,
}

impl ShadowSnapshot {
    /// Creates a shadow snapshot of the primary directory inside `shadow_root/thread_id`.
    pub fn create(
        primary_path: impl AsRef<Path>,
        shadow_root: impl AsRef<Path>,
        thread_id: &str,
    ) -> io::Result<Self> {
        let primary_path = primary_path.as_ref().to_path_buf();
        let shadow_path = shadow_root.as_ref().join(thread_id);

        if shadow_path.exists() {
            fs::remove_dir_all(&shadow_path)?;
        }
        fs::create_dir_all(&shadow_path)?;

        copy_dir_all(&primary_path, &shadow_path)?;

        Ok(Self {
            primary_path,
            shadow_path,
        })
    }

    /// Returns the path to the shadow snapshot directory.
    pub fn path(&self) -> &Path {
        &self.shadow_path
    }

    /// Returns the path to the primary workspace directory.
    pub fn primary_path(&self) -> &Path {
        &self.primary_path
    }

    /// Safely merges verified modifications from the shadow snapshot back to the primary directory.
    pub fn merge_back(&self) -> io::Result<()> {
        remove_missing_from_target(&self.primary_path, &self.shadow_path)?;
        copy_dir_all(&self.shadow_path, &self.primary_path)?;
        Ok(())
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            if let Some(parent) = dst_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn remove_missing_from_target(primary: &Path, shadow: &Path) -> io::Result<()> {
    if !primary.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(primary)? {
        let entry = entry?;
        let primary_entry = entry.path();
        let file_name = entry.file_name();
        let shadow_entry = shadow.join(file_name);

        if !shadow_entry.exists() {
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                fs::remove_dir_all(&primary_entry)?;
            } else {
                fs::remove_file(&primary_entry)?;
            }
        } else {
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                remove_missing_from_target(&primary_entry, &shadow_entry)?;
            }
        }
    }
    Ok(())
}
