use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use grein_workspace::pty_session::{InteractivePtySession, PtyCell, PtyColor, PtyScreenSnapshot};

struct UpdateSignal {
    counter: Mutex<u64>,
    cv: Condvar,
}

impl UpdateSignal {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            counter: Mutex::new(0),
            cv: Condvar::new(),
        })
    }

    fn callback(self: &Arc<Self>) -> Arc<dyn Fn() + Send + Sync> {
        let signal = Arc::clone(self);
        Arc::new(move || {
            let mut guard = signal.counter.lock().expect("signal mutex poisoned");
            *guard = guard.saturating_add(1);
            signal.cv.notify_all();
        })
    }

    fn wait_for(
        &self,
        session: &InteractivePtySession,
        timeout: Duration,
        predicate: impl Fn(&PtyScreenSnapshot) -> bool,
    ) -> PtyScreenSnapshot {
        let deadline = Instant::now() + timeout;
        let mut guard = self.counter.lock().expect("signal mutex poisoned");
        loop {
            let snapshot = session.snapshot();
            if predicate(&snapshot) {
                return snapshot;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!(
                    "timed out waiting for PTY condition; plain_text:\n---\n{}\n---",
                    snapshot.plain_text()
                );
            }
            let (next_guard, _) = self
                .cv
                .wait_timeout(guard, remaining.min(Duration::from_millis(25)))
                .expect("signal condvar poisoned");
            guard = next_guard;
        }
    }
}

#[test]
fn test_spawn_in_temp_dir_renders_pwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let expected_cwd = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
    let expected_str = expected_cwd.to_str().expect("utf8 path").to_string();

    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 160, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    assert!(session.child_pid() > 0);
    session.write_bytes(b"printf '\\n'; pwd\n").expect("write pwd");

    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == expected_str)
    });
    assert!(!snap.exited);
    assert_eq!(snap.exit_code, None);
}

#[test]
fn test_stateful_shell_persists_env_and_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 120, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes(b"export GREIN_PTY_TEST=ok123\n")
        .expect("export env var");
    session
        .write_bytes(b"printf '\\nENV_VAL=%s\\n' \"$GREIN_PTY_TEST\"\n")
        .expect("echo env var");

    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "ENV_VAL=ok123")
    });

    session.write_bytes(b"cd / && printf '\\n' && pwd\n").expect("cd root");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|line| line.trim() == "/")
    });
}

#[test]
fn test_ctrl_c_interrupts_sleep_and_keeps_shell_alive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 120, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes(b"printf '\\nSLEEP_%s\\n' READY && sleep 30\n")
        .expect("start sleep");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "SLEEP_READY")
    });

    std::thread::sleep(Duration::from_millis(80));
    let interrupt_started = Instant::now();
    session.write_bytes(b"\x03").expect("send ctrl-c");
    std::thread::sleep(Duration::from_millis(120));
    session
        .write_bytes(b"printf '\\nAFTER_%s\\n' CTRL_C\n")
        .expect("echo marker after ctrl-c");

    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "AFTER_CTRL_C")
    });

    assert!(
        interrupt_started.elapsed() < Duration::from_secs(5),
        "Ctrl+C must interrupt sleep 30 promptly"
    );
}

#[test]
fn test_dynamic_resize_updates_snapshot_and_tty_winsize() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 80, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    let initial = session.snapshot();
    assert_eq!(initial.rows, 24);
    assert_eq!(initial.cols, 80);

    session.resize(42, 110).expect("resize pty");
    let resized = session.snapshot();
    assert_eq!(resized.rows, 42);
    assert_eq!(resized.cols, 110);
    assert_eq!(resized.cells.len(), 42);
    assert_eq!(resized.cells[0].len(), 110);

    session
        .write_bytes(b"printf '\\n'; stty size\n")
        .expect("query stty size");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|line| line.trim() == "42 110")
    });
}

#[test]
fn test_truecolor_rgb_and_wide_utf8_emoji_cells() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 120, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes("printf '\\033[38;2;255;100;50m📦XY\\033[0m\\n'\n".as_bytes())
        .expect("write truecolor wide utf8");

    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
        find_colored_emoji_cell(s).is_some()
    });

    let (row_idx, col_idx) = find_colored_emoji_cell(&snap).expect("colored emoji cell");
    let row = &snap.cells[row_idx];
    assert_eq!(row[col_idx].contents, "📦");
    assert_eq!(row[col_idx].fg, PtyColor::Rgb(255, 100, 50));
    assert!(!row[col_idx].is_wide_continuation);

    assert!(
        row[col_idx + 1].is_wide_continuation,
        "cell after wide emoji must be wide continuation: {:?}",
        row[col_idx + 1]
    );
    assert_eq!(row[col_idx + 2].contents, "X");
    assert_eq!(row[col_idx + 2].fg, PtyColor::Rgb(255, 100, 50));
    assert!(!row[col_idx + 2].is_wide_continuation);

    assert_eq!(row[col_idx + 3].contents, "Y");
    assert_eq!(row[col_idx + 3].fg, PtyColor::Rgb(255, 100, 50));
    assert!(!row[col_idx + 3].is_wide_continuation);

    assert!(
        snap.plain_text().contains("📦XY"),
        "plain_text should contain 📦XY without wide-char gap: {}",
        snap.plain_text()
    );
}

#[test]
fn test_drop_kills_process_group_and_reaps_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 120, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    let pid = session.child_pid();
    session
        .write_bytes(b"printf '\\nSLEEP_GROUP_%s\\n' ACTIVE && sleep 60\n")
        .expect("start long sleep");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "SLEEP_GROUP_ACTIVE")
    });
    std::thread::sleep(Duration::from_millis(50));

    drop(session);

    let pid_i32 = pid as libc::pid_t;
    let rc_pid = unsafe { libc::kill(pid_i32, 0) };
    let err_pid = std::io::Error::last_os_error();
    assert_eq!(rc_pid, -1, "child PID {pid_i32} must be reaped on drop");
    assert_eq!(err_pid.raw_os_error(), Some(libc::ESRCH));

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let rc_pgid = unsafe { libc::kill(-pid_i32, 0) };
        if rc_pgid == -1 {
            let err_pgid = std::io::Error::last_os_error();
            assert_eq!(err_pgid.raw_os_error(), Some(libc::ESRCH));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "process group -{pid_i32} still has live processes after Drop"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_shell_exit_updates_snapshot_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 120, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session.write_bytes(b"exit 42\n").expect("exit shell");
    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| s.exited);
    assert!(snap.exited);
    assert_eq!(snap.exit_code, Some(42));
}

#[test]
fn test_dsr_cursor_and_status_queries_are_answered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 120, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes(
            b"stty raw -echo; printf '\\033[5n'; r5=$(dd bs=4 count=1 2>/dev/null); printf '\\033[2;4H\\033[6n'; r6=$(dd bs=6 count=1 2>/dev/null); stty sane; printf 'DSR5=%s DSR6=%s\\n' \"$(printf '%s' \"$r5\" | tr '\\033' 'E')\" \"$(printf '%s' \"$r6\" | tr '\\033' 'E')\"\n",
        )
        .expect("run dsr query script");

    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text().contains("DSR5=E[0n DSR6=E[2;4R")
    });
}

#[test]
fn test_plain_text_skips_wide_continuations_and_trims_trailing_spaces() {
    let snap = PtyScreenSnapshot {
        rows: 2,
        cols: 6,
        cells: vec![
            vec![
                PtyCell {
                    contents: "📦".to_string(),
                    ..PtyCell::default()
                },
                PtyCell {
                    contents: String::new(),
                    is_wide_continuation: true,
                    ..PtyCell::default()
                },
                PtyCell {
                    contents: "X".to_string(),
                    ..PtyCell::default()
                },
                PtyCell::default(),
                PtyCell {
                    contents: "Y".to_string(),
                    ..PtyCell::default()
                },
                PtyCell::default(),
            ],
            vec![PtyCell::default(); 6],
        ],
        ..PtyScreenSnapshot::default()
    };
    assert_eq!(snap.plain_text(), "📦X Y\n");
}

fn find_colored_emoji_cell(snapshot: &PtyScreenSnapshot) -> Option<(usize, usize)> {
    for (r, row) in snapshot.cells.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if cell.contents == "📦" && cell.fg == PtyColor::Rgb(255, 100, 50) {
                return Some((r, c));
            }
        }
    }
    None
}
