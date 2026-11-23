use std::fs;
use std::path::Path;
use std::process::Command;

use grein_workspace::list_workspace_files;
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

    fs::write(root.join("README.md"), "hello\n").expect("write README");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "Initial commit"]);

    temp_dir
}

#[test]
fn lists_tracked_and_untracked_files_excluding_gitignore() {
    let repo = init_git_repo();
    let root = repo.path();

    fs::write(root.join(".gitignore"), "*.log\ntarget/\n.grein/\n").expect("write .gitignore");
    fs::create_dir_all(root.join("src")).expect("create src dir");
    fs::write(root.join("src/lib.rs"), "pub fn ok() {}\n").expect("write src/lib.rs");
    run_git(root, &["add", ".gitignore", "src/lib.rs"]);
    run_git(root, &["commit", "-m", "Add source and gitignore"]);

    fs::create_dir_all(root.join("notes")).expect("create notes dir");
    fs::write(root.join("notes/todo.md"), "- task\n").expect("write untracked note");
    fs::write(root.join("debug.log"), "ignored\n").expect("write ignored log");
    fs::create_dir_all(root.join("target/debug")).expect("create target dir");
    fs::write(root.join("target/debug/bin"), "ignored\n").expect("write ignored target file");
    fs::create_dir_all(root.join(".grein")).expect("create .grein dir");
    fs::write(root.join(".grein/state.db"), "ignored\n").expect("write ignored .grein file");

    let files = list_workspace_files(root, 100);
    assert_eq!(
        files,
        vec![
            ".gitignore".to_string(),
            "README.md".to_string(),
            "notes/todo.md".to_string(),
            "src/lib.rs".to_string(),
        ]
    );
}

#[test]
fn enforces_max_entries_cap() {
    let repo = init_git_repo();
    let root = repo.path();

    fs::write(root.join("a.txt"), "a\n").expect("write a");
    fs::write(root.join("b.txt"), "b\n").expect("write b");
    fs::write(root.join("c.txt"), "c\n").expect("write c");

    assert!(list_workspace_files(root, 0).is_empty());

    let limited = list_workspace_files(root, 2);
    assert_eq!(limited, vec!["README.md".to_string(), "a.txt".to_string()]);
}

#[test]
fn non_git_missing_and_file_paths_return_empty_vec() {
    let plain_dir = TempDir::new().expect("create plain temp dir");
    fs::write(plain_dir.path().join("visible.txt"), "plain\n").expect("write file");
    fs::create_dir_all(plain_dir.path().join(".grein")).expect("create .grein dir");
    fs::write(plain_dir.path().join(".grein/db"), "secret\n").expect("write .grein file");

    assert!(list_workspace_files(plain_dir.path(), 100).is_empty());
    assert!(list_workspace_files(&plain_dir.path().join("missing"), 100).is_empty());
    assert!(list_workspace_files(&plain_dir.path().join("visible.txt"), 100).is_empty());
}

#[test]
fn subdirectory_lists_relative_paths() {
    let repo = init_git_repo();
    let root = repo.path();

    fs::create_dir_all(root.join("crates/app")).expect("create crates/app");
    fs::write(root.join("crates/app/main.rs"), "fn main() {}\n").expect("write main.rs");
    fs::write(root.join("crates/app/lib.rs"), "pub fn x() {}\n").expect("write lib.rs");

    let sub_files = list_workspace_files(&root.join("crates"), 100);
    assert_eq!(
        sub_files,
        vec!["app/lib.rs".to_string(), "app/main.rs".to_string()]
    );
}

#[test]
fn deduplicates_conflicted_merge_stage_entries() {
    let repo = init_git_repo();
    let root = repo.path();

    let base_branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"]);

    run_git(root, &["checkout", "-b", "side"]);
    fs::write(root.join("README.md"), "side\n").expect("write side README");
    run_git(root, &["commit", "-am", "Side change"]);

    run_git(root, &["checkout", &base_branch]);
    fs::write(root.join("README.md"), "main\n").expect("write main README");
    run_git(root, &["commit", "-am", "Main change"]);

    let merge = Command::new("git")
        .args(["merge", "side"])
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("execute git merge");
    assert!(!merge.status.success(), "expected merge conflict");

    assert_eq!(
        list_workspace_files(root, 100),
        vec!["README.md".to_string()]
    );
}
