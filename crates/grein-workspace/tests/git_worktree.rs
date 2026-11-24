#![allow(clippy::needless_borrows_for_generic_args)]

use std::fs;
use std::process::Command;
use tempfile::TempDir;

use grein_workspace::allocator::WorkspaceError;
use grein_workspace::allocator::{ThreadId, WorktreeAllocator, WorktreeMode};
use grein_workspace::environment::{EnvironmentDetector, WorkspaceEnvironment};
use grein_workspace::git::{GitRepository, MergeResult};
use grein_workspace::teardown::{TeardownAction, TeardownOutcome};

fn init_test_git_repo() -> TempDir {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("failed to run git command");
        assert!(status.success(), "git command failed: {:?}", args);
    };

    run(&["init"]);
    run(&["config", "user.name", "Grein Test"]);
    run(&["config", "user.email", "test@grein.dev"]);

    let test_file = root.join("main.rs");
    fs::write(&test_file, "fn main() { println!(\"base\"); }\n")
        .expect("failed to write base file");

    run(&["add", "."]);
    run(&["commit", "-m", "Initial commit"]);

    temp_dir
}

fn git_output(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("failed to run git command");
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn test_nested_subdirectory_detected_as_git_repository() {
    let temp_repo = init_test_git_repo();
    let nested = temp_repo.path().join("a").join("b").join("c");
    fs::create_dir_all(&nested).expect("create nested dirs");

    let status = EnvironmentDetector::new(true).detect(&nested);
    assert_eq!(status.environment, WorkspaceEnvironment::GitRepository);
    assert!(status.git_repo_detected);
}

#[test]
fn test_grein_directory_excluded_from_commits() {
    let temp_repo = init_test_git_repo();
    let repo_root = temp_repo.path();

    let git_repo = GitRepository::discover(repo_root).expect("should discover git root");
    let allocator = WorktreeAllocator::new(git_repo);
    let mut session = allocator
        .open_session(ThreadId::from("exclude-thread-1"))
        .expect("should open session");
    session
        .transition_to_isolated_worktree()
        .expect("should allocate worktree");

    let worktree_dir = session.working_directory().to_path_buf();
    fs::create_dir_all(worktree_dir.join(".grein")).expect("create .grein in worktree");
    fs::write(worktree_dir.join(".grein").join("state.json"), "{}\n").expect("write .grein file");
    fs::write(worktree_dir.join("new_file.rs"), "// new\n").expect("write new file");

    session
        .commit("Add new file")
        .expect("commit should succeed");

    let files = git_output(&worktree_dir, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains("new_file.rs"), "files: {files}");
    assert!(
        !files.contains(".grein"),
        ".grein must not be committed, got: {files}"
    );

    let exclude = fs::read_to_string(repo_root.join(".git").join("info").join("exclude"))
        .expect("read exclude file");
    assert_eq!(
        exclude.lines().filter(|l| l.trim() == ".grein/").count(),
        1,
        "exclude must contain .grein/ exactly once"
    );
}

#[test]
fn test_close_merge_propagates_conflict() {
    let temp_repo = init_test_git_repo();
    let repo_root = temp_repo.path();

    let git_repo = GitRepository::discover(repo_root).expect("should discover git root");
    let allocator = WorktreeAllocator::new(git_repo);
    let mut session = allocator
        .open_session(ThreadId::from("merge-conflict-close"))
        .expect("should open session");
    session
        .transition_to_isolated_worktree()
        .expect("should allocate worktree");

    let worktree_dir = session.working_directory().to_path_buf();
    fs::write(
        worktree_dir.join("main.rs"),
        "fn main() { println!(\"from-worktree\"); }\n",
    )
    .expect("write worktree file");
    session
        .commit("Worktree change")
        .expect("commit should succeed");

    fs::write(
        repo_root.join("main.rs"),
        "fn main() { println!(\"from-primary\"); }\n",
    )
    .expect("write primary file");
    for args in [vec!["add", "."], vec!["commit", "-m", "Primary change"]] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(repo_root)
            .status()
            .expect("git failed");
        assert!(status.success());
    }

    match session.close(TeardownAction::Merge) {
        Err(WorkspaceError::MergeConflict(files)) => {
            assert!(files.iter().any(|f| f.ends_with("main.rs")));
        }
        other => panic!("expected MergeConflict error, got: {:?}", other),
    }
}

#[test]
fn test_close_merge_success_cleans_up_worktree() {
    let temp_repo = init_test_git_repo();
    let repo_root = temp_repo.path();

    let git_repo = GitRepository::discover(repo_root).expect("should discover git root");
    let allocator = WorktreeAllocator::new(git_repo);
    let mut session = allocator
        .open_session(ThreadId::from("merge-success-close"))
        .expect("should open session");
    session
        .transition_to_isolated_worktree()
        .expect("should allocate worktree");

    let worktree_dir = session.working_directory().to_path_buf();
    fs::write(worktree_dir.join("added.rs"), "// added\n").expect("write worktree file");
    session.commit("Add file").expect("commit should succeed");

    let outcome = session
        .close(TeardownAction::Merge)
        .expect("merge close should succeed");
    assert_eq!(outcome, TeardownOutcome::Merged);
    assert!(!worktree_dir.exists(), "worktree must be removed");
    assert!(
        repo_root.join("added.rs").exists(),
        "merged file in primary"
    );

    let branches = git_output(repo_root, &["branch", "--list"]);
    assert!(
        !branches.contains("merge-success-close"),
        "branch must be removed, got: {branches}"
    );
}

#[test]
fn test_shared_read_transition_to_lazy_allocation() {
    let temp_repo = init_test_git_repo();
    let repo_root = temp_repo.path();

    let git_repo = GitRepository::discover(repo_root).expect("should discover git root");
    let allocator = WorktreeAllocator::new(git_repo);

    let thread_id = ThreadId::from("thread-42");
    let mut session = allocator
        .open_session(thread_id.clone())
        .expect("should create initial session");

    assert_eq!(session.mode(), WorktreeMode::SharedRead);
    let worktree_dir = repo_root.join(".grein").join("worktrees").join("thread-42");
    assert!(
        !worktree_dir.exists(),
        "worktree directory should not exist in shared read mode"
    );

    assert_eq!(session.working_directory(), repo_root);

    session
        .transition_to_isolated_worktree()
        .expect("should transition to isolated worktree");

    assert_eq!(session.mode(), WorktreeMode::IsolatedWorktree);
    assert!(
        worktree_dir.exists(),
        "isolated worktree directory should be dynamically created"
    );
    assert_eq!(session.working_directory(), &worktree_dir);

    let isolated_file = worktree_dir.join("main.rs");
    fs::write(&isolated_file, "fn main() { println!(\"isolated\"); }\n")
        .expect("should write to isolated worktree");

    let base_content =
        fs::read_to_string(repo_root.join("main.rs")).expect("should read base file");
    assert!(base_content.contains("base"));
}

#[test]
fn test_clean_teardown_pruning() {
    let temp_repo = init_test_git_repo();
    let repo_root = temp_repo.path();

    let git_repo = GitRepository::discover(repo_root).expect("should discover git root");
    let allocator = WorktreeAllocator::new(git_repo);

    let thread_id = ThreadId::from("clean-thread-101");
    let mut session = allocator
        .open_session(thread_id.clone())
        .expect("should open session");

    session
        .transition_to_isolated_worktree()
        .expect("should allocate worktree");

    let worktree_dir = repo_root
        .join(".grein")
        .join("worktrees")
        .join("clean-thread-101");
    assert!(
        worktree_dir.exists(),
        "worktree directory must exist prior to teardown"
    );

    let outcome = session
        .close(TeardownAction::AutoPruneClean)
        .expect("closing clean session should succeed");

    assert_eq!(outcome, TeardownOutcome::Pruned);
    assert!(
        !worktree_dir.exists(),
        "clean worktree directory must be removed after auto-prune"
    );
}

#[test]
fn test_merge_conflict_detection() {
    let temp_repo = init_test_git_repo();
    let repo_root = temp_repo.path();

    let git_repo = GitRepository::discover(repo_root).expect("should discover git root");
    let allocator = WorktreeAllocator::new(git_repo.clone());

    let thread_id = ThreadId::from("conflict-branch-505");
    let mut session = allocator
        .open_session(thread_id.clone())
        .expect("should open session");

    session
        .transition_to_isolated_worktree()
        .expect("should allocate worktree");

    let worktree_dir = repo_root
        .join(".grein")
        .join("worktrees")
        .join("conflict-branch-505");

    fs::write(
        worktree_dir.join("main.rs"),
        "fn main() { println!(\"from-worktree\"); }\n",
    )
    .expect("write worktree file");
    session
        .commit("Worktree conflicting commit")
        .expect("commit in worktree should succeed");

    fs::write(
        repo_root.join("main.rs"),
        "fn main() { println!(\"from-base-workspace\"); }\n",
    )
    .expect("write primary file");

    let status = Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .status()
        .expect("git add failed");
    assert!(status.success());

    let status = Command::new("git")
        .args(["commit", "-m", "Conflicting base commit"])
        .current_dir(repo_root)
        .status()
        .expect("git commit failed");
    assert!(status.success());

    let merge_result = session.merge_to_primary();
    match merge_result {
        Ok(MergeResult::Conflict { conflicting_files }) => {
            assert!(conflicting_files.iter().any(|f| f.ends_with("main.rs")));
        }
        other => panic!("expected conflict merge result, got: {:?}", other),
    }
}
