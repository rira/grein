#![allow(clippy::needless_borrows_for_generic_args)]

use std::error::Error as _;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use grein_workspace::allocator::{ThreadId, WorktreeAllocator, WorktreeMode, WorkspaceError};
use grein_workspace::diff::compute_directory_diff;
use grein_workspace::environment::{detect_environment, EnvironmentDetector, WorkspaceEnvironment};
use grein_workspace::git::{GitError, GitRepository, MergeResult};
use grein_workspace::pty_session::{InteractivePtySession, PtyScreenSnapshot};
use grein_workspace::shadow::{detect_git_environment, ShadowSnapshot};
use grein_workspace::shell::run_shell_command_with_pty_blocking;
use grein_workspace::teardown::{TeardownAction, TeardownOutcome};
use tempfile::{tempdir, TempDir};

fn init_git_repo() -> TempDir {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "user.email", "test@example.com"]);
    fs::write(root.join("base.txt"), "base\n").expect("write base");
    run(&["add", "."]);
    run(&["commit", "-m", "initial"]);
    temp
}

#[test]
fn allocator_teardown_keep_discard_dirty_autoprune_and_exclude_newline() {
    let repo_dir = init_git_repo();
    let exclude_path = repo_dir.path().join(".git").join("info").join("exclude");
    fs::write(&exclude_path, "*.log").expect("write exclude without trailing newline");

    let repo = GitRepository::discover(repo_dir.path().join(".")).expect("discover");
    let allocator = WorktreeAllocator::new(repo);

    let tid_shared = ThreadId::new("shared-prune");
    assert_eq!(tid_shared.as_str(), "shared-prune");
    assert_eq!(tid_shared.to_string(), "shared-prune");
    let mut shared_session = allocator
        .open_session(tid_shared)
        .expect("open shared");
    assert_eq!(shared_session.branch_name(), "worktree-shared-prune");
    assert!(shared_session
        .worktree_dir()
        .ends_with(".grein/worktrees/shared-prune"));
    assert_eq!(
        shared_session
            .close(TeardownAction::AutoPruneClean)
            .expect("autoprune on shared read"),
        TeardownOutcome::Pruned
    );
    assert_eq!(
        shared_session
            .close(TeardownAction::Discard)
            .expect("discard on shared read"),
        TeardownOutcome::Discarded
    );

    let mut dirty_session = allocator
        .open_session(ThreadId::from("dirty-keep"))
        .expect("open dirty");
    dirty_session
        .transition_to_isolated_worktree()
        .expect("transition 1");
    dirty_session
        .transition_to_isolated_worktree()
        .expect("idempotent transition 2");
    assert_eq!(dirty_session.mode(), WorktreeMode::IsolatedWorktree);

    let exclude_after = fs::read_to_string(&exclude_path).expect("read exclude");
    assert_eq!(exclude_after, "*.log\n.grein/\n");

    let empty_commit_err = dirty_session
        .commit("nothing to commit")
        .expect_err("clean commit must fail");
    assert!(matches!(empty_commit_err, WorkspaceError::GitError(_)));

    let wt_dir = dirty_session.working_directory().to_path_buf();
    fs::write(wt_dir.join("dirty.txt"), "uncommitted\n").expect("dirty file");

    assert_eq!(
        dirty_session
            .close(TeardownAction::AutoPruneClean)
            .expect("autoprune dirty keeps"),
        TeardownOutcome::Kept
    );
    assert!(wt_dir.exists());

    assert_eq!(
        dirty_session
            .close(TeardownAction::Keep)
            .expect("keep action"),
        TeardownOutcome::Kept
    );
    assert!(wt_dir.exists());

    assert_eq!(
        dirty_session
            .close(TeardownAction::Discard)
            .expect("discard dirty isolated"),
        TeardownOutcome::Discarded
    );
    assert!(!wt_dir.exists());

    let io_ws_err: WorkspaceError = std::io::Error::other("disk full").into();
    assert!(io_ws_err.to_string().contains("I/O error:"));
    assert_eq!(WorkspaceError::InvalidPath.to_string(), "Invalid worktree path");
    assert_eq!(
        WorkspaceError::MergeConflict(vec!["a.rs".into(), "b.rs".into()]).to_string(),
        "Merge conflict in files: a.rs, b.rs"
    );
    assert!(WorkspaceError::GitError("bad".into())
        .to_string()
        .contains("Git error:"));
    assert!(io_ws_err.source().is_none());
}

#[test]
fn shell_pty_output_capture_crlf_corner_cases() {
    let dir = tempdir().expect("tempdir");
    let out = run_shell_command_with_pty_blocking(
        dir.path(),
        "printf 'a\\r\\r\\nb\\rc\\r'",
        Duration::from_secs(5),
        1024,
    )
    .expect("run pty command");

    assert_eq!(out.exit_code, 0);
    assert!(!out.timed_out);
    assert_eq!(out.stdout, "a\r\r\nb\rc\r");
}

#[test]
fn diff_added_deleted_non_nul_invalid_utf8_and_shadow_recreate() {
    let temp = tempdir().expect("tempdir");
    let primary = temp.path().join("primary");
    let shadow_dir = temp.path().join("shadow");
    fs::create_dir_all(&primary).expect("mkdir primary");
    fs::create_dir_all(&shadow_dir).expect("mkdir shadow");

    fs::write(primary.join("deleted.txt"), "old1\nold2\n").expect("write deleted");
    fs::write(shadow_dir.join("added.txt"), "new1\n").expect("write added");
    fs::write(primary.join("bad_utf8.dat"), [0xff_u8, 0xfe, 0x80]).expect("write invalid utf8 1");
    fs::write(shadow_dir.join("bad_utf8.dat"), [0xff_u8, 0xfe, 0x81]).expect("write invalid utf8 2");

    let diff = compute_directory_diff(&primary, &shadow_dir).expect("compute diff");
    assert!(diff.contains("--- a/added.txt\n+++ b/added.txt\n@@ -1,0 +1,1 @@\n+new1\n"));
    assert!(
        diff.contains("--- a/deleted.txt\n+++ b/deleted.txt\n@@ -1,2 +1,0 @@\n-old1\n-old2\n")
    );
    assert!(diff.contains("Binary files a/bad_utf8.dat and b/bad_utf8.dat differ\n"));

    let missing_diff =
        compute_directory_diff(&temp.path().join("nope1"), &temp.path().join("nope2"))
            .expect("missing dirs yield empty diff");
    assert!(missing_diff.is_empty());

    let s1 = ShadowSnapshot::create(&primary, "recreate-tid").expect("create shadow 1");
    assert_eq!(s1.primary_path(), primary.as_path());
    let stale_file = s1.shadow_path().join("stale.txt");
    fs::write(&stale_file, "stale").expect("write stale");
    assert!(stale_file.exists());

    let s2 = ShadowSnapshot::create(&primary, "recreate-tid").expect("recreate shadow 2");
    assert!(!stale_file.exists());
    assert!(s2.shadow_path().join("deleted.txt").exists());
    s2.cleanup().expect("cleanup 1");
    s2.cleanup().expect("idempotent cleanup 2");
}

#[test]
fn git_environment_detector_and_git_error_paths() {
    let repo_dir = init_git_repo();
    assert!(detect_git_environment(repo_dir.path()));

    let env_status = detect_environment(repo_dir.path());
    assert!(env_status.git_repo_detected);
    assert_eq!(
        env_status.environment,
        if env_status.git_binary_present {
            WorkspaceEnvironment::GitRepository
        } else {
            WorkspaceEnvironment::NonGitDirectory
        }
    );

    let no_git_bin = EnvironmentDetector::new(false);
    assert!(!no_git_bin.is_git_binary_available());
    let status_no_bin = no_git_bin.detect(repo_dir.path());
    assert!(status_no_bin.git_repo_detected);
    assert!(!status_no_bin.git_binary_present);
    assert_eq!(
        status_no_bin.environment,
        WorkspaceEnvironment::NonGitDirectory
    );

    let nested = repo_dir.path().join("sub").join("deep");
    fs::create_dir_all(&nested).expect("mkdir nested");
    let discovered = GitRepository::discover(&nested).expect("discover from nested");
    assert_eq!(discovered.root(), repo_dir.path());

    let non_git = tempdir().expect("non git tempdir");
    let not_found = GitRepository::discover(non_git.path()).expect_err("non-git must fail");
    assert!(matches!(not_found, GitError::NotFound));
    assert_eq!(not_found.to_string(), "Git repository not found");
    let cmd_failed = GitError::CommandFailed("exit 128".into());
    assert_eq!(cmd_failed.to_string(), "Git command failed: exit 128");
    assert!(cmd_failed.source().is_none());

    assert_ne!(
        MergeResult::Success,
        MergeResult::Conflict {
            conflicting_files: vec!["a.rs".into()]
        }
    );
}

#[test]
fn pty_session_edge_cases_resize_zero_empty_write_and_post_exit_poll() {
    let dir = tempdir().expect("tempdir");
    let signal_state = Arc::new((Mutex::new(0u64), Condvar::new()));
    let signal_cb = {
        let st = Arc::clone(&signal_state);
        Arc::new(move || {
            let mut g = st.0.lock().expect("lock");
            *g += 1;
            st.1.notify_all();
        })
    };

    let session = InteractivePtySession::spawn(dir.path(), 4, 20, Some("/bin/sh"), signal_cb)
        .expect("spawn pty");
    assert!(format!("{session:?}").contains("InteractivePtySession"));

    session.write_bytes(&[]).expect("empty write is noop");
    session.resize(0, 0).expect("resize clamps to 1x1");
    let min_snap = session.snapshot();
    assert_eq!(min_snap.rows, 1);
    assert_eq!(min_snap.cols, 1);
    session.resize(4, 20).expect("restore 4x20 before exit");

    assert_eq!(session.set_scrollback(999), 0);
    assert_eq!(session.scroll_by(-10), 0);

    session.write_bytes(b"exit 0\n").expect("exit shell");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut guard = signal_state.0.lock().expect("lock");
    let exited_snap: PtyScreenSnapshot = loop {
        let snap = session.snapshot();
        if snap.exited {
            break snap;
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        assert!(!rem.is_zero(), "timed out waiting for pty exit");
        let (next, _) = signal_state
            .1
            .wait_timeout(guard, rem.min(Duration::from_millis(25)))
            .expect("wait");
        guard = next;
    };
    assert!(exited_snap.exited);
    assert_eq!(exited_snap.exit_code, Some(0));
    assert_eq!(session.poll_os_cwd(), None);

    let missing_cwd = Path::new("/nonexistent/grein/pty/cwd/path");
    assert!(InteractivePtySession::spawn(
        missing_cwd,
        24,
        80,
        Some("/bin/sh"),
        Arc::new(|| {})
    )
    .is_err());
}
