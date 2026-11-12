use grein_workspace::git::{GitError, SharedWorkspace, discover_repository_root};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_discover_repository_root_from_root() {
    let temp = tempdir().expect("failed to create temp dir");
    let git_dir = temp.path().join(".git");
    fs::create_dir(&git_dir).expect("failed to create .git directory");

    let root = discover_repository_root(temp.path()).expect("should discover repo root");
    assert_eq!(root, temp.path());
}

#[test]
fn test_discover_repository_root_from_nested_subdirectory() {
    let temp = tempdir().expect("failed to create temp dir");
    let git_dir = temp.path().join(".git");
    fs::create_dir(&git_dir).expect("failed to create .git directory");

    let nested = temp
        .path()
        .join("crates")
        .join("grein-workspace")
        .join("src");
    fs::create_dir_all(&nested).expect("failed to create nested dirs");

    let root =
        discover_repository_root(&nested).expect("should discover repo root from nested path");
    assert_eq!(root, temp.path());
}

#[test]
fn test_discover_repository_root_with_git_worktree_file() {
    let temp = tempdir().expect("failed to create temp dir");
    let git_file = temp.path().join(".git");
    fs::write(&git_file, "gitdir: /path/to/parent/.git/worktrees/child")
        .expect("failed to write .git file");

    let root = discover_repository_root(temp.path()).expect("should recognize .git file pointer");
    assert_eq!(root, temp.path());
}

#[test]
fn test_discover_repository_root_not_a_git_repository() {
    let temp = tempdir().expect("failed to create temp dir");
    let nested = temp.path().join("orphan_dir");
    fs::create_dir_all(&nested).expect("failed to create orphan dir");

    let result = discover_repository_root(&nested);
    match result {
        Err(GitError::NotAGitRepository { path }) => {
            assert_eq!(path, nested);
        }
        other => panic!("expected GitError::NotAGitRepository, got {:?}", other),
    }
}

#[test]
fn test_shared_read_only_workspace_detection() {
    let temp = tempdir().expect("failed to create temp dir");
    let git_dir = temp.path().join(".git");
    fs::create_dir(&git_dir).expect("failed to create .git directory");

    let workspace = SharedWorkspace::detect(temp.path()).expect("should create shared workspace");
    assert_eq!(workspace.root_path(), temp.path());
    assert!(
        workspace.is_read_only(),
        "initial workspace state must be shared read-only"
    );
    assert!(
        !workspace.is_isolated(),
        "shared workspace must not be marked as isolated worktree"
    );
}
