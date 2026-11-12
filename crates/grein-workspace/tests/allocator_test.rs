use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

use grein_workspace::allocator::{
    LazyWorktreeAllocator, ThreadId, WorktreeAllocationError, WorktreeState,
};

fn init_test_git_repo() -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let repo_root = temp_dir.path().to_path_buf();

    let run_git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&repo_root)
            .output()
            .expect("failed to run git command");
        assert!(
            output.status.success(),
            "git command failed: {:?}, stderr: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Eldi Test"]);
    run_git(&["config", "user.email", "test@eldi.dev"]);

    let initial_file = repo_root.join("README.md");
    fs::write(&initial_file, "# Grein Workspace Test\n").expect("failed to write initial file");

    run_git(&["add", "."]);
    run_git(&["commit", "-m", "Initial commit"]);

    (temp_dir, repo_root)
}

#[test]
fn test_lazy_allocator_remains_shared_until_first_mutation() {
    let (_temp, repo_root) = init_test_git_repo();
    let allocator = LazyWorktreeAllocator::new(repo_root.clone());
    let thread_id = ThreadId::new("thread-42");

    // Before any mutating call, worktree state is Shared
    assert_eq!(
        allocator.state_for(&thread_id),
        WorktreeState::SharedInspection
    );
    assert!(!allocator.is_allocated(&thread_id));

    // Working path defaults to base repository root
    let effective_path = allocator
        .resolve_path_for_read(&thread_id)
        .expect("read path resolution failed");
    assert_eq!(effective_path, repo_root);

    // Target worktree directory must NOT exist yet
    let expected_worktree_dir = repo_root.join(".grein").join("worktrees").join("thread-42");
    assert!(!expected_worktree_dir.exists());
}

#[test]
fn test_lazy_allocator_allocates_isolated_worktree_on_first_mutation() {
    let (_temp, repo_root) = init_test_git_repo();
    let mut allocator = LazyWorktreeAllocator::new(repo_root.clone());
    let thread_id = ThreadId::new("thread-101");

    let worktree_dir = allocator
        .ensure_allocated_for_mutation(&thread_id)
        .expect("failed to allocate worktree for mutation");

    let expected_path = repo_root
        .join(".grein")
        .join("worktrees")
        .join("thread-101");
    assert_eq!(worktree_dir, expected_path);
    assert!(worktree_dir.exists());
    assert!(worktree_dir.is_dir());

    // State transitions to Isolated
    assert_eq!(
        allocator.state_for(&thread_id),
        WorktreeState::IsolatedWorktree(expected_path.clone())
    );
    assert!(allocator.is_allocated(&thread_id));

    // Verification: It is a valid Git worktree with an isolated branch
    let git_branch_output = Command::new("git")
        .args(&["branch", "--show-current"])
        .current_dir(&expected_path)
        .output()
        .expect("failed to run git in worktree");
    assert!(git_branch_output.status.success());
    let branch_name = String::from_utf8_lossy(&git_branch_output.stdout)
        .trim()
        .to_string();
    assert_eq!(branch_name, "grein/thread-101");

    // Verification: Read path now points to isolated worktree
    let effective_path = allocator
        .resolve_path_for_read(&thread_id)
        .expect("read path resolution failed");
    assert_eq!(effective_path, expected_path);
}

#[test]
fn test_allocator_is_idempotent_on_subsequent_mutations() {
    let (_temp, repo_root) = init_test_git_repo();
    let mut allocator = LazyWorktreeAllocator::new(repo_root.clone());
    let thread_id = ThreadId::new("thread-idempotent");

    let first_path = allocator
        .ensure_allocated_for_mutation(&thread_id)
        .expect("first allocation failed");

    let second_path = allocator
        .ensure_allocated_for_mutation(&thread_id)
        .expect("second allocation failed");

    assert_eq!(first_path, second_path);
}

#[test]
fn test_allocator_fails_if_root_is_not_git_repo() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let not_git_root = temp_dir.path().to_path_buf();
    let mut allocator = LazyWorktreeAllocator::new(not_git_root);
    let thread_id = ThreadId::new("thread-err");

    let result = allocator.ensure_allocated_for_mutation(&thread_id);
    match result {
        Err(WorktreeAllocationError::NotAGitRepository(_)) => (),
        other => panic!("expected NotAGitRepository error, got {:?}", other),
    }
}
