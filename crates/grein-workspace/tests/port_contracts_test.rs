use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use grein_core::port::dto::PtyScreenSnapshot;
use grein_workspace::port::{
    GitCliInspector, GitInspectorPort, GitWorktreeDriver, PtyShellExecutor, PtySpawnerPort,
    ShadowCopyIsolator, ShellExecutorPort, TerminalPtyHandle, UnixPtySpawner,
    WorkspaceIsolationPort,
};
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
    fs::write(root.join("README.md"), "# Grein\n").expect("write readme");
    run(&["add", "."]);
    run(&["commit", "-m", "initial"]);
    temp
}

#[test]
fn git_worktree_driver_and_inspector_ports_operate_via_trait_objects() {
    let repo = init_git_repo();
    let repo_canon = fs::canonicalize(repo.path()).expect("canon repo");
    fs::write(repo.path().join("untracked.txt"), "hello\n").expect("write untracked");

    let inspector: Arc<dyn GitInspectorPort> = Arc::new(GitCliInspector::new());
    assert_eq!(
        inspector.resolve_primary_repo_root(repo.path()),
        Some(repo_canon.clone())
    );
    assert_eq!(
        inspector.resolve_worktree_toplevel(repo.path()),
        Some(repo_canon.clone())
    );
    let status = inspector
        .inspect_git_status(repo.path())
        .expect("git status");
    assert_eq!(status.dirty_count, 1);
    assert!(!status.is_worktree);
    let files = inspector.list_workspace_files(repo.path(), 50);
    assert!(files.contains(&"README.md".to_string()));
    assert!(files.contains(&"untracked.txt".to_string()));

    let isolation: Arc<dyn WorkspaceIsolationPort> = Arc::new(GitWorktreeDriver::new());
    let wt_path = isolation
        .open_thread_worktree(repo.path(), "thread-port-1", None)
        .expect("open thread worktree");
    assert!(wt_path.is_dir());
    assert!(wt_path.starts_with(repo_canon.join(".grein").join("worktrees")));
    assert_eq!(
        inspector.resolve_primary_repo_root(&wt_path),
        Some(repo_canon.clone())
    );
    assert_eq!(
        inspector.resolve_worktree_toplevel(&wt_path),
        Some(wt_path.clone())
    );

    // Re-opening from inside the isolated worktree is idempotent.
    let same_wt = isolation
        .open_thread_worktree(repo.path(), "thread-port-1", Some(&wt_path))
        .expect("idempotent reopen");
    assert_eq!(same_wt, wt_path);

    let listed = isolation.list_worktrees(std::slice::from_ref(&repo_canon), Some(&repo_canon));
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|w| w.is_primary && w.path == repo_canon));
    assert!(listed.iter().any(|w| !w.is_primary && w.is_grein_managed && w.path == wt_path));

    fs::write(wt_path.join("README.md"), "# Modified\n").expect("modify worktree");
    let preview = isolation.diff_preview(&wt_path, 50);
    assert!(preview.contains("Modified"), "diff preview:\n{preview}");

    assert!(isolation
        .remove_worktree(&repo_canon, &wt_path, false, true)
        .is_err());
    isolation
        .remove_worktree(&repo_canon, &wt_path, true, true)
        .expect("force remove");
    assert!(!wt_path.exists());

    let prune_msg = isolation
        .prune_worktrees(&repo_canon)
        .expect("prune worktrees");
    assert!(!prune_msg.is_empty());

    let non_git = tempdir().expect("non git tempdir");
    assert!(isolation
        .open_thread_worktree(non_git.path(), "t1", None)
        .is_err());
}

#[test]
fn shadow_copy_isolator_implements_workspace_isolation_port_for_non_git_dirs() {
    let dir = tempdir().expect("tempdir");
    let root = fs::canonicalize(dir.path()).expect("canon root");
    fs::write(root.join("notes.txt"), "v1\n").expect("write notes");

    let isolator: Arc<dyn WorkspaceIsolationPort> = Arc::new(ShadowCopyIsolator::new());
    let shadow_dir = isolator
        .open_thread_worktree(&root, "thread-shadow-1", None)
        .expect("open shadow");
    assert!(shadow_dir.is_dir());
    assert_eq!(
        fs::read_to_string(shadow_dir.join("notes.txt")).expect("read shadow notes"),
        "v1\n"
    );

    // Idempotent reopen preserves existing shadow edits.
    fs::write(shadow_dir.join("notes.txt"), "v2\n").expect("modify shadow");
    let reopened = isolator
        .open_thread_worktree(&root, "thread-shadow-1", Some(&shadow_dir))
        .expect("reopen shadow");
    assert_eq!(reopened, shadow_dir);
    assert_eq!(
        fs::read_to_string(reopened.join("notes.txt")).expect("read preserved shadow"),
        "v2\n"
    );

    let listed = isolator.list_worktrees(&[root.clone(), shadow_dir.clone()], Some(&root));
    assert_eq!(listed.len(), 2);
    let shadow_entry = listed
        .iter()
        .find(|w| !w.is_primary)
        .expect("shadow entry");
    assert!(shadow_entry.is_grein_managed);
    assert_eq!(shadow_entry.dirty_count, 1);

    let diff = isolator.diff_preview(&shadow_dir, 50);
    assert!(diff.contains("-v1") && diff.contains("+v2"), "diff:\n{diff}");

    assert!(isolator
        .remove_worktree(&root, &shadow_dir, false, false)
        .is_err());
    isolator
        .remove_worktree(&root, &shadow_dir, true, false)
        .expect("force remove shadow");
    assert!(!shadow_dir.exists());

    let pruned = isolator.prune_worktrees(&root).expect("prune shadow");
    assert!(pruned.contains("clean"));
}

#[test]
fn unix_pty_spawner_and_terminal_pty_handle_ports_cover_full_lifecycle() {
    let dir1 = tempdir().expect("dir1");
    let dir2 = tempdir().expect("dir2");
    let canon_dir2 = fs::canonicalize(dir2.path()).expect("canon dir2");

    let signal = Arc::new((Mutex::new(0u64), Condvar::new()));
    let on_update: Arc<dyn Fn() + Send + Sync> = {
        let sig = Arc::clone(&signal);
        Arc::new(move || {
            let mut g = sig.0.lock().expect("lock");
            *g += 1;
            sig.1.notify_all();
        })
    };

    let spawner: Arc<dyn PtySpawnerPort> = Arc::new(UnixPtySpawner::new());
    let handle: Arc<dyn TerminalPtyHandle> = spawner
        .spawn_with_env(
            dir1.path(),
            10,
            80,
            Some("/bin/sh"),
            &[("GREIN_PORT_ENV", "yes")],
            on_update,
        )
        .expect("spawn pty handle");

    assert!(handle.child_pid() > 0);
    assert!(!handle.is_exited());
    assert!(handle.live_cwd().is_some());

    handle
        .write_bytes(b"printf '\\033]2;PortTitle\\007'; i=1; while [ \"$i\" -le 20 ]; do printf 'LINE_%02d\\n' \"$i\"; i=$((i+1)); done; printf 'ENV=%s\\n' \"$GREIN_PORT_ENV\"\n")
        .expect("write script");

    let wait_until = |target: &Arc<dyn TerminalPtyHandle>, pred: &dyn Fn(&PtyScreenSnapshot) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut g = signal.0.lock().expect("lock");
        loop {
            let snap = target.snapshot();
            if pred(&snap) {
                return snap;
            }
            let rem = deadline.saturating_duration_since(Instant::now());
            assert!(!rem.is_zero(), "timed out; screen:\n{}", snap.plain_text());
            let (next, _) = signal
                .1
                .wait_timeout(g, rem.min(Duration::from_millis(25)))
                .expect("wait");
            g = next;
        }
    };

    wait_until(&handle, &|s| {
        s.plain_text().lines().any(|l| l.trim() == "ENV=yes")
    });
    assert_eq!(handle.terminal_title().as_deref(), Some("PortTitle"));

    assert!(handle.scroll(5) > 0);
    assert_eq!(handle.set_scrollback(0), 0);
    handle.resize(16, 100).expect("resize");
    assert_eq!(handle.snapshot().rows, 16);
    assert_eq!(handle.snapshot().cols, 100);

    let restarted = handle
        .restart_in_cwd(&canon_dir2)
        .expect("restart_in_cwd");
    assert_ne!(restarted.child_pid(), handle.child_pid());
    assert_eq!(restarted.snapshot().rows, 16);
    assert_eq!(restarted.snapshot().cols, 100);

    restarted
        .write_bytes(b"printf '\\n'; pwd\n")
        .expect("write pwd");
    let restarted_snap = wait_until(&restarted, &|s| {
        s.plain_text()
            .lines()
            .any(|l| l.trim() == canon_dir2.to_string_lossy())
    });
    assert!(!restarted_snap.exited);
    let _ = restarted.poll_os_cwd();

    restarted.write_bytes(b"exit 0\n").expect("exit shell");
    wait_until(&restarted, &|s| s.exited);
    assert!(restarted.is_exited());
    assert_eq!(restarted.live_cwd(), None);
}

#[test]
fn pty_shell_executor_implements_shell_executor_port() {
    let dir = tempdir().expect("tempdir");
    let executor: Arc<dyn ShellExecutorPort> = Arc::new(PtyShellExecutor::new());
    let res = executor
        .run_shell_command(
            dir.path(),
            "printf 'out-ok\\n'; printf 'err-ok\\n' >&2; exit 3",
            Duration::from_secs(5),
            4096,
        )
        .expect("run shell command");

    assert_eq!(res.stdout, "out-ok\n");
    assert_eq!(res.stderr, "err-ok\n");
    assert_eq!(res.exit_code, 3);
    assert!(!res.timed_out);
}

#[cfg(unix)]
#[test]
fn pty_shell_executor_enforces_parity_contracts() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir(dir.path().join("alpha_dir")).expect("mkdir");
    fs::write(dir.path().join("beta.txt"), "hi\n").expect("write");

    let executor: Arc<dyn ShellExecutorPort> = Arc::new(PtyShellExecutor::new());

    let tty_and_size = executor
        .run_shell_command(
            dir.path(),
            "if command -v stty >/dev/null 2>&1; then stty size <&1; else echo NO_STTY; fi",
            Duration::from_secs(5),
            4096,
        )
        .expect("stty size check");
    assert_eq!(tty_and_size.exit_code, 0);
    if tty_and_size.stdout.trim() != "NO_STTY" {
        assert_eq!(tty_and_size.stdout.trim(), "40 120");
    }

    let ls_pipe = executor
        .run_shell_command(dir.path(), "ls -1 | cat", Duration::from_secs(5), 4096)
        .expect("ls pipe");
    assert_eq!(ls_pipe.exit_code, 0);
    assert!(
        !ls_pipe.stdout.contains("\x1b["),
        "pipe ls had ANSI: {:?}",
        ls_pipe.stdout
    );
    assert!(ls_pipe.stdout.contains("alpha_dir"));
    assert!(ls_pipe.stdout.contains("beta.txt"));

    let eof_res = executor
        .run_shell_command(dir.path(), "cat", Duration::from_secs(5), 1024)
        .expect("cat eof");
    assert_eq!(eof_res.exit_code, 0);
    assert!(!eof_res.timed_out);
    assert_eq!(eof_res.stdout, "");

    let sig_res = executor
        .run_shell_command(
            dir.path(),
            "sh -c 'kill -9 $$'",
            Duration::from_secs(5),
            1024,
        )
        .expect("sigkill exit");
    assert!(!sig_res.timed_out);
    assert_eq!(sig_res.exit_code, 137);

    let timeout_res = executor
        .run_shell_command(
            dir.path(),
            "sh -c 'sleep 30 & printf \"GC=%s\\n\" \"$!\"; wait'",
            Duration::from_millis(250),
            4096,
        )
        .expect("timeout kills grandchild");
    assert!(timeout_res.timed_out);
    assert_eq!(timeout_res.exit_code, -1);
    let gc_pid: libc::pid_t = timeout_res
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("GC="))
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
            "grandchild process {gc_pid} survived session timeout kill"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[allow(dead_code)]
fn assert_workspace_port_object_safety(
    _iso: Arc<dyn WorkspaceIsolationPort>,
    _git: Arc<dyn GitInspectorPort>,
    _spawner: Arc<dyn PtySpawnerPort>,
    _pty: Arc<dyn TerminalPtyHandle>,
    _shell: Arc<dyn ShellExecutorPort>,
    _p: PathBuf,
) {
}
