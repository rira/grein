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
use grein_workspace::shell::{
    run_shell_command_blocking, run_shell_command_with_pty_and_shell_blocking,
    run_shell_command_with_pty_blocking,
};
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

#[test]
fn shell_pty_and_interactive_session_concurrent_stress_test() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let handles: Vec<_> = (0..6)
        .map(|worker_idx| {
            let cwd = root.clone();
            std::thread::spawn(move || {
                for iter in 0..3 {
                    let expected = format!("worker-{worker_idx}-iter-{iter}");
                    let cmd = format!("printf '{expected}\\n'");
                    let out = run_shell_command_with_pty_blocking(
                        &cwd,
                        &cmd,
                        Duration::from_secs(5),
                        4096,
                    )
                    .expect("concurrent pty command runs");
                    assert_eq!(out.exit_code, 0);
                    assert!(!out.timed_out);
                    assert_eq!(out.stdout, format!("{expected}\n"));

                    let signal_state = Arc::new((Mutex::new(0u64), Condvar::new()));
                    let signal_cb = {
                        let st = Arc::clone(&signal_state);
                        Arc::new(move || {
                            let mut g = st.0.lock().expect("lock");
                            *g += 1;
                            st.1.notify_all();
                        })
                    };
                    let session = InteractivePtySession::spawn(
                        &cwd,
                        24,
                        80,
                        Some("/bin/sh"),
                        signal_cb,
                    )
                    .expect("concurrent interactive pty spawn");
                    session.write_bytes(b"exit 0\n").expect("exit shell");
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut guard = signal_state.0.lock().expect("lock");
                    loop {
                        let snap = session.snapshot();
                        if snap.exited {
                            assert_eq!(snap.exit_code, Some(0));
                            break;
                        }
                        let rem = deadline.saturating_duration_since(Instant::now());
                        assert!(!rem.is_zero(), "timed out waiting for interactive exit");
                        let (next, _) = signal_state
                            .1
                            .wait_timeout(guard, rem.min(Duration::from_millis(25)))
                            .expect("wait");
                        guard = next;
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("worker thread succeeds");
    }
}

#[cfg(unix)]
#[test]
fn shell_pipe_normal_exit_with_orphan_fd_is_bounded_and_cleans_up() {
    let dir = tempdir().expect("tempdir");
    let started = Instant::now();
    let out = run_shell_command_blocking(
        dir.path(),
        "sleep 2 & printf 'ORPHAN_PID=%s\\n' \"$!\"",
        Duration::from_secs(5),
        4096,
    )
    .expect("pipe command returns cleanly");
    let elapsed = started.elapsed();

    assert_eq!(out.exit_code, 0);
    assert!(!out.timed_out);
    assert!(
        elapsed < Duration::from_millis(1200),
        "normal pipe exit with open orphan fd must be bounded by drain wait, took {elapsed:?}"
    );
    let pid: libc::pid_t = out
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("ORPHAN_PID="))
        .expect("orphan pid printed")
        .trim()
        .parse()
        .expect("valid pid");
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(unix)]
#[test]
fn shell_pty_detach_tty_session_eof_signal_and_grandchild_timeout() {
    let dir = tempdir().expect("tempdir");
    let parent_sid = unsafe { libc::getsid(0) };

    let started = Instant::now();
    let eof = run_shell_command_with_pty_and_shell_blocking(
        dir.path(),
        "cat",
        Duration::from_secs(5),
        1024,
        Some("/bin/sh"),
    )
    .expect("cat eof");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(eof.exit_code, 0);
    assert_eq!(eof.stdout, "");

    let tty_out = run_shell_command_with_pty_and_shell_blocking(
        dir.path(),
        "( exec < /dev/tty ) 2>/dev/null && echo HAS_TTY || echo NO_TTY",
        Duration::from_secs(5),
        1024,
        Some("/bin/sh"),
    )
    .expect("tty check");
    assert_eq!(tty_out.stdout.trim(), "NO_TTY");

    #[cfg(target_os = "linux")]
    {
        let stat_out = run_shell_command_with_pty_and_shell_blocking(
            dir.path(),
            "exec cat /proc/self/stat",
            Duration::from_secs(5),
            4096,
            Some("/bin/sh"),
        )
        .expect("proc stat");
        let after = stat_out.stdout.rsplit_once(')').expect("comm end").1;
        let cols: Vec<&str> = after.split_whitespace().collect();
        let pid: libc::pid_t = stat_out
            .stdout
            .split_once('(')
            .expect("comm start")
            .0
            .trim()
            .parse()
            .expect("pid");
        let pgrp: libc::pid_t = cols[2].parse().expect("pgrp");
        let sid: libc::pid_t = cols[3].parse().expect("sid");
        let tty_nr: i64 = cols[4].parse().expect("tty_nr");
        assert_eq!(pid, pgrp);
        assert_eq!(pid, sid);
        assert_eq!(tty_nr, 0);
        assert_ne!(sid, parent_sid);
    }

    #[cfg(target_os = "macos")]
    {
        let ps_out = run_shell_command_with_pty_and_shell_blocking(
            dir.path(),
            "ps -o pid=,pgid=,sid= -p $$",
            Duration::from_secs(5),
            4096,
            Some("/bin/sh"),
        )
        .expect("ps stat");
        let cols: Vec<libc::pid_t> = ps_out
            .stdout
            .split_whitespace()
            .map(|v| v.parse().expect("id"))
            .collect();
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0], cols[1]);
        assert_eq!(cols[0], cols[2]);
        assert_ne!(cols[2], parent_sid);
    }

    let sig = run_shell_command_with_pty_and_shell_blocking(
        dir.path(),
        "kill -9 $$",
        Duration::from_secs(5),
        1024,
        Some("/bin/sh"),
    )
    .expect("kill -9 $$");
    assert!(!sig.timed_out);
    assert_eq!(sig.exit_code, 137);

    let timeout_out = run_shell_command_with_pty_and_shell_blocking(
        dir.path(),
        "sleep 30 & printf 'GC=%s\\n' \"$!\"; wait",
        Duration::from_millis(250),
        4096,
        Some("/bin/sh"),
    )
    .expect("timeout returns output");
    assert!(timeout_out.timed_out);
    assert_eq!(timeout_out.exit_code, -1);
    let gc_pid: libc::pid_t = timeout_out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("GC="))
        .expect("grandchild pid printed")
        .trim()
        .parse()
        .expect("valid gc pid");
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let rc = unsafe { libc::kill(gc_pid, 0) };
        if rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "grandchild {gc_pid} survived timeout kill"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn shell_pty_shell_override_supports_posix_shells_and_stty_size() {
    let dir = tempdir().expect("tempdir");
    let stty_out = run_shell_command_with_pty_and_shell_blocking(
        dir.path(),
        "if command -v stty >/dev/null 2>&1; then stty size <&1; else echo NO_STTY; fi",
        Duration::from_secs(5),
        1024,
        Some("/bin/sh"),
    )
    .expect("stty size command runs");
    assert_eq!(stty_out.exit_code, 0);
    if stty_out.stdout.trim() != "NO_STTY" {
        assert_eq!(stty_out.stdout.trim(), "40 120");
    }

    for candidate in ["bash", "zsh"] {
        if let Some(bin) = std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join(candidate))
                .find(|c| c.is_file())
        }) {
            for shell_arg in [candidate, bin.to_string_lossy().as_ref()] {
                let out = run_shell_command_with_pty_and_shell_blocking(
                    dir.path(),
                    "[[ 1 -eq 1 ]] && echo ok",
                    Duration::from_secs(5),
                    1024,
                    Some(shell_arg),
                )
                .expect("override shell runs");
                assert_eq!(out.exit_code, 0);
                assert_eq!(out.stdout.trim(), "ok");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn shell_respects_user_shell_env_var_isolated() {
    const FLAG: &str = "GREIN_TEST_USER_SHELL_ENV";
    if let Some(expected_shell) = std::env::var_os(FLAG) {
        let expected_shell = expected_shell.to_string_lossy();
        let dir = tempdir().expect("tempdir");
        let cmd = if expected_shell.ends_with("bash") {
            "[[ 1 -eq 1 ]] && printf '%s' \"$0\""
        } else {
            "printf '%s' \"$0\""
        };
        let pty_out =
            run_shell_command_with_pty_blocking(dir.path(), cmd, Duration::from_secs(5), 1024)
                .expect("pty runs with SHELL");
        assert_eq!(pty_out.exit_code, 0);
        assert_eq!(pty_out.stdout.trim(), expected_shell.as_ref());

        let pipe_out =
            run_shell_command_blocking(dir.path(), cmd, Duration::from_secs(5), 1024)
                .expect("pipe runs with SHELL");
        assert_eq!(pipe_out.exit_code, 0);
        assert_eq!(pipe_out.stdout.trim(), expected_shell.as_ref());
        return;
    }

    let target_shell = std::env::var_os("PATH")
        .and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("bash"))
                .find(|c| c.is_file())
        })
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/bin/sh".to_string());

    let exe = std::env::current_exe().expect("test binary");
    let out = Command::new(exe)
        .arg("--exact")
        .arg("shell_respects_user_shell_env_var_isolated")
        .arg("--nocapture")
        .env(FLAG, &target_shell)
        .env("SHELL", &target_shell)
        .output()
        .expect("run isolated SHELL test");
    assert!(
        out.status.success(),
        "isolated SHELL test failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
