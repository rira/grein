use std::fs;
use std::path::Path;
use tempfile::tempdir;

// Step 0: Test environment detection for git repository presence and git executable in PATH
use grein_workspace::environment::{
    EnvironmentDetector, GitEnvironmentStatus, WorkspaceEnvironment, detect_environment,
};

#[test]
fn test_detect_environment_in_non_git_directory() {
    let temp = tempdir().expect("failed to create temp dir");
    let non_git_path = temp.path();

    let status = detect_environment(non_git_path);
    assert_eq!(
        status.git_repo_detected, false,
        "Expected non-git directory to report git_repo_detected = false"
    );
    assert_eq!(
        status.environment,
        WorkspaceEnvironment::NonGitDirectory,
        "Expected environment to be NonGitDirectory"
    );
}

#[test]
fn test_detect_environment_in_simulated_git_directory() {
    let temp = tempdir().expect("failed to create temp dir");
    let git_dir = temp.path().join(".git");
    fs::create_dir_all(&git_dir).expect("failed to create mock .git directory");

    let status = detect_environment(temp.path());
    assert_eq!(
        status.git_repo_detected, true,
        "Expected git_repo_detected = true when .git directory exists"
    );
    assert_eq!(
        status.environment,
        WorkspaceEnvironment::GitRepository,
        "Expected environment to be GitRepository"
    );
}

#[test]
fn test_git_binary_availability_check() {
    let detector = EnvironmentDetector::default();
    let is_available = detector.is_git_binary_available();

    // In test runner environment, verify that is_git_binary_available returns a deterministic boolean
    // and aligns with querying PATH for `git` or `git.exe`
    let path_env = std::env::var_os("PATH");
    let expected_available = if let Some(paths) = path_env {
        std::env::split_paths(&paths).any(|dir| {
            dir.join("git").is_file()
                || dir.join("git.exe").is_file()
                || dir.join("git.cmd").is_file()
        })
    } else {
        false
    };

    assert_eq!(
        is_available, expected_available,
        "EnvironmentDetector git binary availability should match PATH search"
    );
}

#[test]
fn test_detect_environment_with_custom_detector_without_git_binary() {
    let temp = tempdir().expect("failed to create temp dir");
    let git_dir = temp.path().join(".git");
    fs::create_dir_all(&git_dir).expect("failed to create mock .git directory");

    // Even if .git folder exists, if git executable is missing, environment should degrade to NonGitFallback
    let custom_detector = EnvironmentDetector::new(false);
    let status = custom_detector.detect(temp.path());

    assert_eq!(status.git_repo_detected, true);
    assert_eq!(status.git_binary_present, false);
    assert_eq!(
        status.environment,
        WorkspaceEnvironment::NonGitDirectory,
        "Missing git binary must fallback to NonGitDirectory environment mode"
    );
}
