use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::allocator::WorkspaceError;
use crate::git_status::inspect_git_status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWorktree {
    pub repo_root: PathBuf,
    pub repo_name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head_short: Option<String>,
    pub is_primary: bool,
    pub is_detached: bool,
    pub is_grein_managed: bool,
    pub dirty_count: usize,
}

#[derive(Default)]
struct RawWorktreeBlock {
    path: Option<PathBuf>,
    head: Option<String>,
    branch: Option<String>,
    is_detached: bool,
    is_bare: bool,
    is_prunable: bool,
}

pub fn resolve_primary_repo_root(path: &Path) -> Option<PathBuf> {
    let stdout = run_git_readonly(path, &["worktree", "list", "--porcelain"])?;
    let primary_dir = stdout
        .lines()
        .find_map(|line| line.strip_prefix("worktree "))
        .filter(|dir| !dir.is_empty())?;
    fs::canonicalize(Path::new(primary_dir)).ok()
}

pub fn resolve_worktree_toplevel(path: &Path) -> Option<PathBuf> {
    let stdout = run_git_readonly(path, &["rev-parse", "--show-toplevel"])?;
    let toplevel = stdout.trim_end_matches(['\r', '\n']);
    if toplevel.is_empty() {
        return None;
    }
    fs::canonicalize(Path::new(toplevel)).ok()
}

pub fn current_branch_name(path: &Path) -> Option<String> {
    let stdout = run_git_readonly(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = stdout.trim();
    (!branch.is_empty()).then(|| branch.to_string())
}

pub fn sanitize_worktree_dir_slug(branch: &str) -> String {
    let mapped: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let mut no_dotdot = String::with_capacity(mapped.len());
    let mut chars = mapped.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '.' && chars.peek() == Some(&'.') {
            while chars.peek() == Some(&'.') {
                chars.next();
            }
            no_dotdot.push('-');
        } else {
            no_dotdot.push(c);
        }
    }

    let mut collapsed = String::with_capacity(no_dotdot.len());
    let mut prev_dash = false;
    for c in no_dotdot.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push('-');
                prev_dash = true;
            }
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }

    let trimmed = collapsed.trim_matches(['.', '-']);
    if trimmed.is_empty() {
        "worktree".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn list_repo_worktrees(repo_hint: &Path) -> Vec<DiscoveredWorktree> {
    let Some(stdout) = run_git_readonly(repo_hint, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };

    let blocks = parse_porcelain_blocks(&stdout);
    let Some(primary_block_path) = blocks.first().and_then(|block| block.path.as_deref()) else {
        return Vec::new();
    };
    let Ok(repo_root) = fs::canonicalize(primary_block_path) else {
        return Vec::new();
    };
    let repo_name = path_basename(&repo_root);
    let grein_worktrees_dir = repo_root.join(".grein").join("worktrees");

    let mut worktrees = Vec::with_capacity(blocks.len());
    for (idx, block) in blocks.into_iter().enumerate() {
        let Some(entry_path) = block.path else {
            continue;
        };
        if block.is_prunable || !entry_path.is_dir() {
            continue;
        }
        let Ok(canonical_path) = fs::canonicalize(&entry_path) else {
            continue;
        };

        let is_primary = idx == 0 || (!block.is_bare && canonical_path.join(".git").is_dir());
        let head_short = block
            .head
            .as_deref()
            .map(|h| h.chars().take(7).collect::<String>())
            .filter(|h| !h.is_empty());
        let branch = block.branch;
        let is_grein_managed = !is_primary
            && (canonical_path.starts_with(&grein_worktrees_dir)
                || branch.as_deref().is_some_and(|b| {
                    b.starts_with("worktree-") || b.ends_with("-wt") || b.contains("-wt-")
                }));
        let dirty_count = inspect_git_status(&entry_path)
            .map(|status| status.dirty_count)
            .unwrap_or(0);

        worktrees.push(DiscoveredWorktree {
            repo_root: repo_root.clone(),
            repo_name: repo_name.clone(),
            path: canonical_path,
            branch,
            head_short,
            is_primary,
            is_detached: block.is_detached,
            is_grein_managed,
            dirty_count,
        });
    }

    worktrees
}

pub fn list_worktrees_across_paths(
    paths: &[PathBuf],
    preferred_repo_hint: Option<&Path>,
) -> Vec<DiscoveredWorktree> {
    let preferred_root = preferred_repo_hint.and_then(resolve_primary_repo_root);

    let mut seen_roots = HashSet::new();
    for path in paths.iter().map(PathBuf::as_path).chain(preferred_repo_hint) {
        if let Some(root) = resolve_primary_repo_root(path) {
            seen_roots.insert(root);
        }
    }

    let mut all_worktrees: Vec<DiscoveredWorktree> = seen_roots
        .into_iter()
        .flat_map(|root| list_repo_worktrees(&root))
        .collect();

    all_worktrees.sort_by(|a, b| {
        worktree_sort_key(a, preferred_root.as_deref())
            .cmp(&worktree_sort_key(b, preferred_root.as_deref()))
    });

    all_worktrees
}

pub fn remove_worktree(
    repo_root: &Path,
    target_path: &Path,
    force: bool,
    delete_branch: bool,
) -> Result<Option<String>, WorkspaceError> {
    let primary_root = resolve_primary_repo_root(repo_root).ok_or_else(|| {
        WorkspaceError::GitError(format!("Not a git repository: {}", repo_root.display()))
    })?;

    let canonical_target = fs::canonicalize(target_path).map_err(|_| {
        WorkspaceError::GitError(format!(
            "Unknown or missing worktree path: {}",
            target_path.display()
        ))
    })?;

    let entry = list_repo_worktrees(&primary_root)
        .into_iter()
        .find(|wt| wt.path == canonical_target)
        .ok_or_else(|| {
            WorkspaceError::GitError(format!(
                "Path is not a registered worktree of {}: {}",
                primary_root.display(),
                canonical_target.display()
            ))
        })?;

    if entry.is_primary {
        return Err(WorkspaceError::GitError(
            "Cannot remove the primary worktree".to_string(),
        ));
    }

    let mut remove_args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        remove_args.push(OsStr::new("--force"));
    }
    remove_args.push(entry.path.as_os_str());
    run_git_mutating(&primary_root, &remove_args)?;

    if delete_branch && entry.is_grein_managed {
        if let Some(ref branch_name) = entry.branch {
            let delete_args = [
                OsStr::new("branch"),
                OsStr::new("-D"),
                OsStr::new(branch_name.as_str()),
            ];
            run_git_mutating(&primary_root, &delete_args)?;
            return Ok(Some(branch_name.clone()));
        }
    }

    Ok(None)
}

fn parse_porcelain_blocks(stdout: &str) -> Vec<RawWorktreeBlock> {
    let mut blocks = Vec::new();
    let mut current = RawWorktreeBlock::default();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            if current.path.is_some() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }

        if let Some(dir) = line.strip_prefix("worktree ") {
            current.path = Some(PathBuf::from(dir));
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            current.head = Some(head.trim().to_string());
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            let trimmed = branch_ref.trim();
            let stripped = trimmed.strip_prefix("refs/heads/").unwrap_or(trimmed);
            current.branch = (!stripped.is_empty()).then(|| stripped.to_string());
        } else if line == "detached" {
            current.is_detached = true;
        } else if line == "bare" {
            current.is_bare = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
            current.is_prunable = true;
        }
    }

    if current.path.is_some() {
        blocks.push(current);
    }

    blocks
}

fn worktree_sort_key<'a>(
    wt: &'a DiscoveredWorktree,
    preferred_root: Option<&Path>,
) -> (bool, String, &'a PathBuf, bool, String, &'a PathBuf) {
    let is_not_preferred = preferred_root != Some(wt.repo_root.as_path());
    let repo_name_lower = wt.repo_name.to_ascii_lowercase();
    let branch_or_basename = wt
        .branch
        .as_deref()
        .filter(|b| !b.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path_basename(&wt.path));

    (
        is_not_preferred,
        repo_name_lower,
        &wt.repo_root,
        !wt.is_primary,
        branch_or_basename.to_ascii_lowercase(),
        &wt.path,
    )
}

fn path_basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn run_git_readonly(path: &Path, args: &[&str]) -> Option<String> {
    if !path.is_dir() {
        return None;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_git_mutating(repo_root: &Path, args: &[&OsStr]) -> Result<(), WorkspaceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let msg = if stderr.is_empty() {
        format!("git command failed with status {}", output.status)
    } else {
        stderr
    };
    Err(WorkspaceError::GitError(msg))
}
