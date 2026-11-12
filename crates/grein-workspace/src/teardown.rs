use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeStatus {
    Clean,
    Dirty {
        has_uncommitted_changes: bool,
        has_unmerged_commits: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownAction {
    Keep,
    Discard,
    Merge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeardownOutcome {
    PrunedClean,
    Kept,
    Discarded,
    Merged,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TeardownError {
    ConfirmationRequired(WorktreeStatus),
    MergeConflict { conflicting_files: Vec<String> },
    Git(String),
    Io(String),
}

impl fmt::Display for TeardownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfirmationRequired(status) => {
                write!(f, "Confirmation required for dirty worktree: {:?}", status)
            }
            Self::MergeConflict { conflicting_files } => {
                write!(
                    f,
                    "Merge conflict in files: {}",
                    conflicting_files.join(", ")
                )
            }
            Self::Git(msg) => write!(f, "Git error: {}", msg),
            Self::Io(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for TeardownError {}

fn run_git(cwd: &Path, args: &[&str]) -> Result<std::process::Output, TeardownError> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| TeardownError::Io(e.to_string()))
}

pub fn inspect_worktree_status(
    primary_root: &Path,
    worktree_dir: &Path,
    branch: &str,
) -> Result<WorktreeStatus, TeardownError> {
    let status_out = run_git(worktree_dir, &["status", "--porcelain"])?;
    if !status_out.status.success() {
        return Err(TeardownError::Git(
            String::from_utf8_lossy(&status_out.stderr).to_string(),
        ));
    }
    let status_str = String::from_utf8_lossy(&status_out.stdout);
    let has_uncommitted_changes = !status_str.trim().is_empty();

    let rev_spec = format!("HEAD..{}", branch);
    let rev_out = run_git(primary_root, &["rev-list", "--count", &rev_spec])?;
    let has_unmerged_commits = if rev_out.status.success() {
        let rev_str = String::from_utf8_lossy(&rev_out.stdout);
        let count: u64 = rev_str.trim().parse().unwrap_or(0);
        count > 0
    } else {
        false
    };

    if has_uncommitted_changes || has_unmerged_commits {
        Ok(WorktreeStatus::Dirty {
            has_uncommitted_changes,
            has_unmerged_commits,
        })
    } else {
        Ok(WorktreeStatus::Clean)
    }
}

fn prune_and_delete_branch(
    primary_root: &Path,
    worktree_dir: &Path,
    branch: &str,
) -> Result<(), TeardownError> {
    let wt_str = worktree_dir.to_str().unwrap_or("");
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", wt_str])
        .current_dir(primary_root)
        .output();

    let _ = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(primary_root)
        .output();

    if worktree_dir.exists() {
        let _ = fs::remove_dir_all(worktree_dir);
    }

    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(primary_root)
        .output();

    Ok(())
}

pub fn teardown_worktree(
    primary_root: &Path,
    worktree_dir: &Path,
    branch: &str,
    action: Option<TeardownAction>,
) -> Result<TeardownOutcome, TeardownError> {
    match action {
        None => {
            let status = inspect_worktree_status(primary_root, worktree_dir, branch)?;
            match status {
                WorktreeStatus::Clean => {
                    prune_and_delete_branch(primary_root, worktree_dir, branch)?;
                    Ok(TeardownOutcome::PrunedClean)
                }
                dirty => Err(TeardownError::ConfirmationRequired(dirty)),
            }
        }
        Some(TeardownAction::Keep) => Ok(TeardownOutcome::Kept),
        Some(TeardownAction::Discard) => {
            prune_and_delete_branch(primary_root, worktree_dir, branch)?;
            Ok(TeardownOutcome::Discarded)
        }
        Some(TeardownAction::Merge) => {
            let status = inspect_worktree_status(primary_root, worktree_dir, branch)?;
            if let WorktreeStatus::Dirty {
                has_uncommitted_changes: true,
                ..
            } = status
            {
                let _ = Command::new("git")
                    .args(["add", "-A"])
                    .current_dir(worktree_dir)
                    .output();
                let _ = Command::new("git")
                    .args([
                        "commit",
                        "-m",
                        "Auto-commit uncommitted changes before merge",
                    ])
                    .current_dir(worktree_dir)
                    .output();
            }

            let merge_out = run_git(primary_root, &["merge", "--no-edit", branch])?;
            if merge_out.status.success() {
                prune_and_delete_branch(primary_root, worktree_dir, branch)?;
                Ok(TeardownOutcome::Merged)
            } else {
                let diff_out = run_git(primary_root, &["diff", "--name-only", "--diff-filter=U"])?;
                let mut conflicting_files: Vec<String> = if diff_out.status.success() {
                    String::from_utf8_lossy(&diff_out.stdout)
                        .lines()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                } else {
                    Vec::new()
                };

                if conflicting_files.is_empty() {
                    let status_out = run_git(primary_root, &["status", "--porcelain"])?;
                    if status_out.status.success() {
                        for line in String::from_utf8_lossy(&status_out.stdout).lines() {
                            if line.len() > 3 {
                                let prefix = &line[0..2];
                                if matches!(prefix, "UU" | "AA" | "UD" | "DU" | "AU" | "UA" | "DD")
                                {
                                    conflicting_files.push(line[3..].trim().to_string());
                                }
                            }
                        }
                    }
                }

                let _ = run_git(primary_root, &["merge", "--abort"]);

                if !conflicting_files.is_empty() {
                    Err(TeardownError::MergeConflict { conflicting_files })
                } else {
                    Err(TeardownError::Git(
                        String::from_utf8_lossy(&merge_out.stderr).to_string(),
                    ))
                }
            }
        }
    }
}
