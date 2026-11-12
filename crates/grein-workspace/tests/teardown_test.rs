use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use grein_workspace::teardown::{
    TeardownAction, TeardownError, TeardownOutcome, WorktreeStatus, inspect_worktree_status,
    teardown_worktree,
};
use tempfile::TempDir;

struct TestGitRepo {
    _dir: TempDir,
    root: PathBuf,
}

impl TestGitRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("Failed to create temp directory");
        let root = dir.path().to_path_buf();

        run_cmd(&root, &["git", "init", "-b", "main"]);
        run_cmd(&root, &["git", "config", "user.name", "Eldi Test"]);
        run_cmd(&root, &["git", "config", "user.email", "test@eldi.local"]);

        let init_file = root.join("README.md");
        fs::write(&init_file, "# Primary Workspace\n").expect("Failed to write README.md");
        run_cmd(&root, &["git", "add", "."]);
        run_cmd(&root, &["git", "commit", "-m", "Initial commit"]);

        Self { _dir: dir, root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn create_worktree(&self, name: &str, branch: &str) -> PathBuf {
        let worktree_dir = self.root.join(".grein").join("worktrees").join(name);
        if let Some(parent) = worktree_dir.parent() {
            fs::create_dir_all(parent).expect("Failed to create worktree parent directory");
        }
        run_cmd(
            &self.root,
            &[
                "git",
                "worktree",
                "add",
                "-b",
                branch,
                worktree_dir.to_str().unwrap(),
                "main",
            ],
        );
        worktree_dir
    }
}

fn run_cmd(cwd: &Path, args: &[&str]) {
    let output = Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("Failed to execute command {:?}: {}", args, e));
    if !output.status.success() {
        panic!(
            "Command {:?} failed with status {}: {}\nstderr: {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_inspect_status_clean_worktree() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-clean", "thread-clean-branch");

    let status = inspect_worktree_status(repo.root(), &wt, "thread-clean-branch")
        .expect("Failed to inspect status");

    assert_eq!(status, WorktreeStatus::Clean);
}

#[test]
fn test_inspect_status_dirty_uncommitted_changes() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-uncommitted", "thread-uncommitted-branch");

    fs::write(wt.join("scratch.txt"), "uncommitted content").expect("Failed to write file");

    let status = inspect_worktree_status(repo.root(), &wt, "thread-uncommitted-branch")
        .expect("Failed to inspect status");

    match status {
        WorktreeStatus::Dirty {
            has_uncommitted_changes,
            has_unmerged_commits,
        } => {
            assert!(has_uncommitted_changes);
            assert!(!has_unmerged_commits);
        }
        WorktreeStatus::Clean => panic!("Expected dirty status for uncommitted edits"),
    }
}

#[test]
fn test_inspect_status_dirty_unmerged_commits() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-commits", "thread-commits-branch");

    let test_file = wt.join("feature.rs");
    fs::write(&test_file, "pub fn test() {}").expect("Failed to write file");
    run_cmd(&wt, &["git", "add", "."]);
    run_cmd(&wt, &["git", "commit", "-m", "feature commit"]);

    let status = inspect_worktree_status(repo.root(), &wt, "thread-commits-branch")
        .expect("Failed to inspect status");

    match status {
        WorktreeStatus::Dirty {
            has_uncommitted_changes,
            has_unmerged_commits,
        } => {
            assert!(!has_uncommitted_changes);
            assert!(has_unmerged_commits);
        }
        WorktreeStatus::Clean => panic!("Expected dirty status for committed branch drift"),
    }
}

#[test]
fn test_teardown_clean_worktree_auto_pruned() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-auto-prune", "thread-auto-prune-branch");

    assert!(wt.exists());

    let outcome = teardown_worktree(repo.root(), &wt, "thread-auto-prune-branch", None)
        .expect("Teardown failed on clean worktree");

    assert_eq!(outcome, TeardownOutcome::PrunedClean);
    assert!(
        !wt.exists(),
        "Worktree directory should be deleted after prune"
    );
}

#[test]
fn test_teardown_dirty_worktree_requires_confirmation() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-need-confirm", "thread-need-confirm-branch");

    fs::write(wt.join("dirty.txt"), "pending work").expect("Failed to write file");

    let result = teardown_worktree(repo.root(), &wt, "thread-need-confirm-branch", None);

    match result {
        Err(TeardownError::ConfirmationRequired(status)) => {
            assert_eq!(
                status,
                WorktreeStatus::Dirty {
                    has_uncommitted_changes: true,
                    has_unmerged_commits: false,
                }
            );
        }
        other => panic!("Expected ConfirmationRequired error, got: {:?}", other),
    }
}

#[test]
fn test_teardown_dirty_worktree_action_keep() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-keep", "thread-keep-branch");

    fs::write(wt.join("kept.txt"), "keep me").expect("Failed to write file");

    let outcome = teardown_worktree(
        repo.root(),
        &wt,
        "thread-keep-branch",
        Some(TeardownAction::Keep),
    )
    .expect("Failed to keep dirty worktree");

    assert_eq!(outcome, TeardownOutcome::Kept);
    assert!(
        wt.exists(),
        "Worktree directory should remain intact on Keep"
    );
}

#[test]
fn test_teardown_dirty_worktree_action_discard() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-discard", "thread-discard-branch");

    fs::write(wt.join("discard.txt"), "throw away").expect("Failed to write file");

    let outcome = teardown_worktree(
        repo.root(),
        &wt,
        "thread-discard-branch",
        Some(TeardownAction::Discard),
    )
    .expect("Failed to discard dirty worktree");

    assert_eq!(outcome, TeardownOutcome::Discarded);
    assert!(
        !wt.exists(),
        "Worktree directory should be deleted on Discard"
    );
}

#[test]
fn test_teardown_dirty_worktree_action_merge_success() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-merge", "thread-merge-branch");

    let file_path = wt.join("merged_file.txt");
    fs::write(&file_path, "clean merge content").expect("Failed to write file");
    run_cmd(&wt, &["git", "add", "."]);
    run_cmd(&wt, &["git", "commit", "-m", "feature to merge"]);

    let outcome = teardown_worktree(
        repo.root(),
        &wt,
        "thread-merge-branch",
        Some(TeardownAction::Merge),
    )
    .expect("Failed to merge worktree branch");

    assert_eq!(outcome, TeardownOutcome::Merged);
    assert!(
        !wt.exists(),
        "Worktree directory should be pruned after successful merge"
    );

    let primary_merged_file = repo.root().join("merged_file.txt");
    assert!(
        primary_merged_file.exists(),
        "Merged file must exist in primary repo"
    );
    let content = fs::read_to_string(&primary_merged_file).expect("Failed to read merged file");
    assert_eq!(content, "clean merge content");
}

#[test]
fn test_teardown_dirty_worktree_action_merge_conflict() {
    let repo = TestGitRepo::new();
    let wt = repo.create_worktree("thread-conflict", "thread-conflict-branch");

    let primary_readme = repo.root().join("README.md");
    fs::write(&primary_readme, "# Conflict from Main\n").expect("Failed to write main README");
    run_cmd(repo.root(), &["git", "add", "."]);
    run_cmd(
        repo.root(),
        &["git", "commit", "-m", "Main conflicting edit"],
    );

    let wt_readme = wt.join("README.md");
    fs::write(&wt_readme, "# Conflict from Branch\n").expect("Failed to write worktree README");
    run_cmd(&wt, &["git", "add", "."]);
    run_cmd(&wt, &["git", "commit", "-m", "Branch conflicting edit"]);

    let result = teardown_worktree(
        repo.root(),
        &wt,
        "thread-conflict-branch",
        Some(TeardownAction::Merge),
    );

    match result {
        Err(TeardownError::MergeConflict { conflicting_files }) => {
            assert!(
                !conflicting_files.is_empty(),
                "Should report conflicting files"
            );
            assert!(conflicting_files.iter().any(|f| f.contains("README.md")));
        }
        other => panic!("Expected MergeConflict error, got: {:?}", other),
    }
}
