use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RepoGitStatus {
    pub branch: Option<String>,
    pub dirty_count: usize,
    pub is_worktree: bool,
}

pub fn inspect_git_status(path: &Path) -> Option<RepoGitStatus> {
    if !path.is_dir() {
        return None;
    }

    let stdout = run_git_readonly(path, &["status", "--porcelain=v1", "-b"])?;
    let mut lines = stdout.lines().peekable();

    let branch = if lines.peek().is_some_and(|first| first.starts_with("## ")) {
        lines
            .next()
            .and_then(|header| parse_branch_header(path, header))
    } else {
        None
    };

    let dirty_count = lines.filter(|line| !line.trim().is_empty()).count();

    let toplevel = run_git_readonly(path, &["rev-parse", "--show-toplevel"])
        .map(|out| out.trim_end_matches(['\r', '\n']).to_string())
        .filter(|out| !out.is_empty())
        .map(PathBuf::from);
    let repo_root = toplevel.as_deref().unwrap_or(path);
    let is_worktree = repo_root.join(".git").is_file();

    Some(RepoGitStatus {
        branch,
        dirty_count,
        is_worktree,
    })
}

fn run_git_readonly(path: &Path, args: &[&str]) -> Option<String> {
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

fn parse_branch_header(path: &Path, header_line: &str) -> Option<String> {
    let header = header_line.strip_prefix("## ")?.trim_end();

    if let Some(branch) = header
        .strip_prefix("No commits yet on ")
        .or_else(|| header.strip_prefix("Initial commit on "))
    {
        return non_empty_trimmed(branch);
    }

    if header == "HEAD (no branch)" {
        let short_sha = run_git_readonly(path, &["rev-parse", "--short", "HEAD"])
            .and_then(|out| non_empty_trimmed(&out))
            .unwrap_or_else(|| "HEAD".to_string());
        return Some(short_sha);
    }

    parse_branch_name(header)
}

fn parse_branch_name(header: &str) -> Option<String> {
    let mut branch = header.split("...").next().unwrap_or(header);
    for marker in [" [ahead ", " [behind ", " [gone]"] {
        if let Some((prefix, _)) = branch.split_once(marker) {
            branch = prefix;
        }
    }
    non_empty_trimmed(branch)
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse_branch_header, parse_branch_name};
    use std::path::Path;

    #[test]
    fn parses_legacy_initial_commit_header() {
        assert_eq!(
            parse_branch_header(Path::new("."), "## Initial commit on master").as_deref(),
            Some("master")
        );
    }

    #[test]
    fn parses_branch_tracking_variants() {
        assert_eq!(
            parse_branch_name("main...origin/main").as_deref(),
            Some("main")
        );
        assert_eq!(
            parse_branch_name("feat/x...origin/feat/x [ahead 2, behind 1]").as_deref(),
            Some("feat/x")
        );
        assert_eq!(
            parse_branch_name("fix/y [behind 3]").as_deref(),
            Some("fix/y")
        );
        assert_eq!(
            parse_branch_name("old-branch...origin/old-branch [gone]").as_deref(),
            Some("old-branch")
        );
        assert_eq!(parse_branch_name("stale [gone]").as_deref(), Some("stale"));
    }
}
