use std::fs;
use std::path::Path;
use std::process::Command;

use grein_workspace::git_status::{inspect_git_status, RepoGitStatus};
use tempfile::TempDir;

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("failed to execute git");
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_git_repo() -> TempDir {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    run_git(root, &["init"]);
    run_git(root, &["config", "user.name", "Grein Test"]);
    run_git(root, &["config", "user.email", "test@grein.dev"]);

    fs::write(root.join("README.md"), "hello\n").expect("write initial file");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "Initial commit"]);

    temp_dir
}

#[test]
fn clean_git_repo_reports_branch_and_zero_dirty() {
    let repo = init_git_repo();
    let status = inspect_git_status(repo.path()).expect("expected git status for repo");

    assert!(
        matches!(status.branch.as_deref(), Some("main" | "master")),
        "unexpected branch: {:?}",
        status.branch
    );
    assert_eq!(status.dirty_count, 0);
    assert!(!status.is_worktree);
}

#[test]
fn modified_and_untracked_files_increase_dirty_count() {
    let repo = init_git_repo();
    fs::write(repo.path().join("README.md"), "modified\n").expect("modify tracked file");
    fs::write(repo.path().join("untracked.txt"), "new\n").expect("create untracked file");

    let status = inspect_git_status(repo.path()).expect("expected git status for dirty repo");

    assert_eq!(status.dirty_count, 2);
    assert!(!status.is_worktree);
}

#[test]
fn non_git_and_missing_paths_return_none() {
    let plain_dir = TempDir::new().expect("create plain temp dir");
    assert_eq!(inspect_git_status(plain_dir.path()), None);

    let missing = plain_dir.path().join("does-not-exist");
    assert_eq!(inspect_git_status(&missing), None);

    let file_path = plain_dir.path().join("not_a_dir.txt");
    fs::write(&file_path, "plain file\n").expect("write plain file");
    assert_eq!(inspect_git_status(&file_path), None);
}

#[test]
fn linked_worktree_reports_worktree_flag_and_isolated_dirty_count() {
    let repo = init_git_repo();
    let wt_parent = TempDir::new().expect("create worktree parent dir");
    let wt_path = wt_parent.path().join("wt-1");
    let wt_str = wt_path.to_str().expect("utf8 path");

    run_git(repo.path(), &["worktree", "add", "-b", "wt-branch", wt_str]);

    let clean_wt = inspect_git_status(&wt_path).expect("expected git status for linked worktree");
    assert_eq!(
        clean_wt,
        RepoGitStatus {
            branch: Some("wt-branch".to_string()),
            dirty_count: 0,
            is_worktree: true,
        }
    );

    fs::write(wt_path.join("worktree-only.txt"), "dirty\n").expect("write dirty file in worktree");

    let dirty_wt = inspect_git_status(&wt_path).expect("expected dirty status for linked worktree");
    assert_eq!(
        dirty_wt,
        RepoGitStatus {
            branch: Some("wt-branch".to_string()),
            dirty_count: 1,
            is_worktree: true,
        }
    );

    let main_repo = inspect_git_status(repo.path()).expect("expected status for main repo");
    assert_eq!(main_repo.dirty_count, 0);
    assert!(!main_repo.is_worktree);

    let nested = wt_path.join("nested").join("subdir");
    fs::create_dir_all(&nested).expect("create nested subdir in worktree");
    let nested_wt = inspect_git_status(&nested).expect("expected status for worktree subdir");
    assert!(nested_wt.is_worktree);
    assert_eq!(nested_wt.branch.as_deref(), Some("wt-branch"));
}

#[test]
fn detached_head_reports_short_sha() {
    let repo = init_git_repo();
    run_git(repo.path(), &["checkout", "--detach", "HEAD"]);
    let expected_sha = run_git(repo.path(), &["rev-parse", "--short", "HEAD"]);
    assert!(!expected_sha.is_empty());

    let status = inspect_git_status(repo.path()).expect("expected git status for detached HEAD");
    assert_eq!(status.branch, Some(expected_sha));
    assert_eq!(status.dirty_count, 0);
    assert!(!status.is_worktree);
}

#[test]
fn unborn_repo_reports_initial_branch_name() {
    let temp_dir = TempDir::new().expect("create temp dir");
    run_git(temp_dir.path(), &["init", "-b", "feature/unborn"]);

    let status = inspect_git_status(temp_dir.path()).expect("expected git status for unborn repo");
    assert_eq!(status.branch.as_deref(), Some("feature/unborn"));
    assert_eq!(status.dirty_count, 0);
    assert!(!status.is_worktree);
}

#[test]
fn tracked_branch_strips_upstream_and_ahead_suffix() {
    let remote = TempDir::new().expect("create bare remote dir");
    run_git(remote.path(), &["init", "--bare"]);

    let repo = init_git_repo();
    let initial_branch = run_git(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]);
    let remote_str = remote.path().to_str().expect("utf8 remote path");

    run_git(repo.path(), &["remote", "add", "origin", remote_str]);
    run_git(repo.path(), &["push", "-u", "origin", &initial_branch]);

    fs::write(repo.path().join("ahead.txt"), "ahead\n").expect("write ahead file");
    run_git(repo.path(), &["add", "."]);
    run_git(repo.path(), &["commit", "-m", "Ahead commit"]);

    let status = inspect_git_status(repo.path()).expect("expected status for tracked branch");
    assert_eq!(status.branch.as_deref(), Some(initial_branch.as_str()));
    assert_eq!(status.dirty_count, 0);
}

#[test]
fn repo_git_status_supports_serde_roundtrip() {
    let original = RepoGitStatus {
        branch: Some("feature/test".to_string()),
        dirty_count: 3,
        is_worktree: true,
    };
    let json = serde_json::to_string(&original).expect("serialize RepoGitStatus");
    let decoded: RepoGitStatus = serde_json::from_str(&json).expect("deserialize RepoGitStatus");
    assert_eq!(decoded, original);
}
