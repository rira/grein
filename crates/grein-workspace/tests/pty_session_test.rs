use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use grein_workspace::pty_session::live_process_cwd;
use grein_workspace::pty_session::{
    InteractivePtySession, PtyCell, PtyColor, PtyScreenSnapshot,
};

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
fn test_da1_da2_and_osc_color_queries_are_answered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 160, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes(
            b"stty raw -echo; \
              printf '\\033[c'; da1=$(dd bs=1 count=7 2>/dev/null); \
              printf '\\033[0c'; da1z=$(dd bs=1 count=7 2>/dev/null); \
              printf '\\033[>c'; da2=$(dd bs=1 count=10 2>/dev/null); \
              printf '\\033[>0c'; da2z=$(dd bs=1 count=10 2>/dev/null); \
              printf '\\033]10;?\\007'; fg=$(dd bs=1 count=25 2>/dev/null); \
              printf '\\033]11;?\\033\\\\'; bg=$(dd bs=1 count=25 2>/dev/null); \
              stty sane; \
              printf '\\nPROBE_OK=%s|%s|%s|%s|%s|%s\\n' \
              \"$(printf '%s' \"$da1\" | tr '\\033' 'E')\" \
              \"$(printf '%s' \"$da1z\" | tr '\\033' 'E')\" \
              \"$(printf '%s' \"$da2\" | tr '\\033' 'E')\" \
              \"$(printf '%s' \"$da2z\" | tr '\\033' 'E')\" \
              \"$(printf '%s' \"$fg\" | tr '\\033' 'E')\" \
              \"$(printf '%s' \"$bg\" | tr '\\033' 'E')\"\n",
        )
        .expect("run capability probe script");

    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|line| {
            line.trim()
                == "PROBE_OK=E[?62;c|E[?62;c|E[>0;10;1c|E[>0;10;1c|E]10;rgb:e5e5/e5e5/e5e5E\\|E]11;rgb:0000/0000/0000E\\"
        })
    });
}

#[test]
fn test_osc7_live_cwd_tracking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 24, 120, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    assert_eq!(session.snapshot().cwd, None);

    session
        .write_bytes(b"printf '\\033]7;file://localhost/tmp/my%%20dir\\007\\n'\n")
        .expect("emit osc 7 bel");
    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.cwd.as_deref() == Some("/tmp/my dir")
    });
    assert_eq!(snap.cwd.as_deref(), Some("/tmp/my dir"));

    session
        .write_bytes(b"printf '\\033]7;file:///tmp/plain%%2Bpath\\033\\\\\\n'\n")
        .expect("emit osc 7 st");
    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.cwd.as_deref() == Some("/tmp/plain+path")
    });
    assert_eq!(snap.cwd.as_deref(), Some("/tmp/plain+path"));

    session
        .write_bytes(b"printf '\\033]7;file:///tmp/caf%%C3%%A9\\007\\n'\n")
        .expect("emit osc 7 utf8");
    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.cwd.as_deref() == Some("/tmp/café")
    });
    assert_eq!(snap.cwd.as_deref(), Some("/tmp/café"));
}

#[test]
fn test_scrollback_navigation_and_write_snaps_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 10, 80, Some("/bin/sh"), signal.callback())
            .expect("spawn 10-row pty session");

    session
        .write_bytes(
            b"printf '\\n'; i=1; while [ \"$i\" -le 50 ]; do printf 'SEQ_%02d\\n' \"$i\"; i=$((i+1)); done; printf 'SEQ_DONE\\n'\n",
        )
        .expect("flood 50 lines");

    let live = signal.wait_for(&session, Duration::from_secs(5), |s| {
        let text = s.plain_text();
        text.lines().any(|line| line.trim() == "SEQ_50")
            && text.lines().any(|line| line.trim() == "SEQ_DONE")
    });
    assert_eq!(live.scrollback_offset, 0);
    assert!(
        !live.plain_text().lines().any(|line| line.trim() == "SEQ_01"),
        "SEQ_01 should have scrolled off the 10-row live viewport"
    );

    let offset = session.scroll_by(15);
    assert_eq!(offset, 15);
    let scrolled = session.snapshot();
    assert_eq!(scrolled.scrollback_offset, 15);
    let scrolled_text = scrolled.plain_text();
    assert!(
        !scrolled_text.lines().any(|line| line.trim() == "SEQ_50"),
        "live bottom line SEQ_50 should not be visible when scrolled up 15 rows:\n{scrolled_text}"
    );
    assert!(
        scrolled_text.contains("SEQ_3"),
        "older SEQ_3x lines should be visible when scrolled up 15 rows:\n{scrolled_text}"
    );

    let max_offset = session.scroll_by(10_000);
    assert!(max_offset >= 40, "expected >= 40 scrollback rows, got {max_offset}");
    let top = session.snapshot();
    assert_eq!(top.scrollback_offset, max_offset);
    assert!(
        top.plain_text().lines().any(|line| line.trim() == "SEQ_01"),
        "scrolling to top should expose SEQ_01:\n{}",
        top.plain_text()
    );

    let stepped_down = session.scroll_by(-5);
    assert_eq!(stepped_down, max_offset - 5);

    let reset_offset = session.set_scrollback(0);
    assert_eq!(reset_offset, 0);
    let back_live = session.snapshot();
    assert_eq!(back_live.scrollback_offset, 0);
    assert!(
        back_live
            .plain_text()
            .lines()
            .any(|line| line.trim() == "SEQ_50")
    );

    assert_eq!(session.scroll_by(15), 15);
    assert_eq!(session.snapshot().scrollback_offset, 15);

    session.write_bytes(b"\n").expect("write newline while scrolled");
    let snapped = session.snapshot();
    assert_eq!(snapped.scrollback_offset, 0);
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
    assert_eq!(snap.plain_text(), "📦X Y");
}

const SCRUBBED_TERMINAL_ENV_VARS: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "KITTY_WINDOW_ID",
    "KITTY_INSTALLATION_DIR",
    "KITTY_PID",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_BIN_DIR",
    "WEZTERM_PANE",
    "WEZTERM_EXECUTABLE",
    "TMUX",
    "TMUX_PANE",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "WT_SESSION",
];

fn run_isolated_subprocess(test_name: &str, flag: &str, extra_env: &[(&str, &str)]) -> bool {
    if std::env::var_os(flag).is_some() {
        return false;
    }
    let exe = std::env::current_exe().expect("current test executable");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(flag, "1");
    for &(key, value) in extra_env {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("spawn isolated test subprocess");
    assert!(
        output.status.success(),
        "isolated subprocess `{test_name}` failed (status: {:?}):\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn wait_for_prefixed_i32(
    signal: &UpdateSignal,
    session: &InteractivePtySession,
    prefix: &str,
) -> (PtyScreenSnapshot, i32) {
    let snap = signal.wait_for(session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim().starts_with(prefix))
    });
    let line = snap
        .plain_text()
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(prefix))
        .expect("prefixed line")
        .to_string();
    let value = line
        .strip_prefix(prefix)
        .unwrap()
        .parse()
        .expect("numeric suffix");
    (snap, value)
}

fn extract_marked_section<'a>(text: &'a str, begin: &str, end: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|line| line.trim() != begin)
        .skip(1)
        .take_while(|line| line.trim() != end)
        .map(str::trim)
        .collect()
}

#[test]
fn test_pty_scrubs_outer_terminal_env_vars_and_preserves_essential_env() {
    let sentinel_path = "/tmp/grein-pty-path-sentinel";
    let current_path = std::env::var("PATH").expect("PATH must be set in test environment");
    let test_path = format!("{sentinel_path}:{current_path}");
    let mut env_pairs: Vec<(String, String)> = vec![
        ("PATH".to_string(), test_path),
        (
            "GREIN_SENTINEL_PATH".to_string(),
            sentinel_path.to_string(),
        ),
        ("HOME".to_string(), "/tmp/grein-pty-test-home".to_string()),
        ("LANG".to_string(), "C".to_string()),
        ("LC_ALL".to_string(), "C".to_string()),
        (
            "STARSHIP_CONFIG".to_string(),
            "/tmp/grein-test-starship.toml".to_string(),
        ),
        ("TERM".to_string(), "xterm-kitty".to_string()),
        ("COLORTERM".to_string(), "24bit".to_string()),
    ];
    for &key in SCRUBBED_TERMINAL_ENV_VARS {
        env_pairs.push((key.to_string(), format!("outer_{key}")));
    }
    let extra_env: Vec<(&str, &str)> = env_pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if run_isolated_subprocess(
        "test_pty_scrubs_outer_terminal_env_vars_and_preserves_essential_env",
        "GREIN_PTY_ENV_SUBPROCESS",
        &extra_env,
    ) {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("check_env.sh");
    let mut script = String::from("printf '\\n'\n");
    for &key in SCRUBBED_TERMINAL_ENV_VARS {
        script.push_str(&format!(
            "eval \"val=\\${{{key}+SET}}\"\nprintf 'SCRUB_{key}=%s\\n' \"${{val:-UNSET}}\"\n"
        ));
    }
    script.push_str(
        "printf 'PRESERVED_TERM=%s\\n' \"$TERM\"\n\
         printf 'PRESERVED_COLORTERM=%s\\n' \"$COLORTERM\"\n\
         printf 'PRESERVED_STARSHIP=%s\\n' \"$STARSHIP_CONFIG\"\n\
         printf 'PRESERVED_HOME=%s\\n' \"$HOME\"\n\
         printf 'PRESERVED_LANG=%s\\n' \"$LANG\"\n\
         printf 'PRESERVED_LC_ALL=%s\\n' \"$LC_ALL\"\n\
         case \":$PATH:\" in\n\
             *\":$GREIN_SENTINEL_PATH:\"*) printf 'PRESERVED_PATH=OK\\n' ;;\n\
             *) printf 'PRESERVED_PATH=MISMATCH:%s\\n' \"$PATH\" ;;\n\
         esac\n\
         printf 'CHECK_ENV_DONE\\n'\n",
    );
    std::fs::write(&script_path, script).expect("write check_env.sh");

    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 48, 160, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes(b". ./check_env.sh\n")
        .expect("run check_env.sh");

    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "CHECK_ENV_DONE")
    });
    let text = snap.plain_text();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();

    for &key in SCRUBBED_TERMINAL_ENV_VARS {
        let expected = format!("SCRUB_{key}=UNSET");
        assert!(
            lines.contains(&expected.as_str()),
            "expected `{expected}` in PTY output:\n{text}"
        );
    }
    for expected in [
        "PRESERVED_TERM=xterm-256color",
        "PRESERVED_COLORTERM=truecolor",
        "PRESERVED_STARSHIP=/tmp/grein-test-starship.toml",
        "PRESERVED_HOME=/tmp/grein-pty-test-home",
        "PRESERVED_LANG=C",
        "PRESERVED_LC_ALL=C",
        "PRESERVED_PATH=OK",
    ] {
        assert!(
            lines.contains(&expected),
            "expected `{expected}` in PTY output:\n{text}"
        );
    }
}

#[cfg(unix)]
#[test]
fn test_pty_child_resets_signal_mask_and_dispositions() {
    if run_isolated_subprocess(
        "test_pty_child_resets_signal_mask_and_dispositions",
        "GREIN_PTY_SIGNAL_SUBPROCESS",
        &[],
    ) {
        return;
    }

    const SIGNALS_TO_POISON: &[libc::c_int] = &[
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGCHLD,
        libc::SIGPIPE,
    ];

    unsafe {
        let mut block_set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        assert_eq!(libc::sigemptyset(block_set.as_mut_ptr()), 0);
        let mut block_set = block_set.assume_init();
        for &sig in SIGNALS_TO_POISON {
            assert_eq!(libc::sigaddset(&mut block_set, sig), 0);
        }
        assert_eq!(
            libc::sigprocmask(libc::SIG_BLOCK, &block_set, std::ptr::null_mut()),
            0
        );
        for &sig in SIGNALS_TO_POISON {
            assert_ne!(libc::signal(sig, libc::SIG_IGN), libc::SIG_ERR);
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir.path(), 40, 140, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes(
            b"sh -c 'kill -PIPE $$; echo SURVIVED_PIPE'; printf '\\nPIPE_RC=%s\\n' \"$?\"\n",
        )
        .expect("run SIGPIPE check");
    let (snap, pipe_rc) = wait_for_prefixed_i32(&signal, &session, "PIPE_RC=");
    assert_eq!(
        pipe_rc,
        128 + libc::SIGPIPE,
        "SIGPIPE must terminate child with 128+SIGPIPE, got {pipe_rc} in:\n{}",
        snap.plain_text()
    );
    assert!(
        !snap
            .plain_text()
            .lines()
            .any(|line| line.trim() == "SURVIVED_PIPE"),
        "child survived SIGPIPE:\n{}",
        snap.plain_text()
    );

    session
        .write_bytes(b"ulimit -c 0 && printf '\\nQUIT_SLEEP_%s\\n' READY && sleep 30\n")
        .expect("start sleep for SIGQUIT");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "QUIT_SLEEP_READY")
    });
    std::thread::sleep(Duration::from_millis(80));
    session.write_bytes(b"\x1c").expect("send ctrl-backslash");
    std::thread::sleep(Duration::from_millis(120));
    session
        .write_bytes(b"printf '\\nQUIT_RC=%s\\n' \"$?\"\n")
        .expect("check SIGQUIT exit status");
    let (snap, quit_rc) = wait_for_prefixed_i32(&signal, &session, "QUIT_RC=");
    assert_eq!(
        quit_rc,
        128 + libc::SIGQUIT,
        "SIGQUIT must terminate child with 128+SIGQUIT, got {quit_rc} in:\n{}",
        snap.plain_text()
    );

    session
        .write_bytes(b"printf '\\nTRAP_%s\\n' BEGIN; sh -c 'trap'; printf '\\nTRAP_%s\\n' END\n")
        .expect("run trap check");
    let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|line| line.trim() == "TRAP_END")
    });
    let text = snap.plain_text();
    let trap_section = extract_marked_section(&text, "TRAP_BEGIN", "TRAP_END").join("\n");
    assert!(
        !trap_section.contains("trap -- ''"),
        "unexpected inherited ignored signals in `sh -c trap`:\n{trap_section}"
    );

    #[cfg(target_os = "linux")]
    {
        session
            .write_bytes(
                b"printf '\\nPROC_%s\\n' BEGIN; grep -E '^(SigBlk|SigIgn):' /proc/self/status; printf '\\nPROC_%s\\n' END\n",
            )
            .expect("check /proc/self/status SigBlk and SigIgn");
        let snap = signal.wait_for(&session, Duration::from_secs(5), |s| {
            s.plain_text().lines().any(|line| line.trim() == "PROC_END")
        });
        let text = snap.plain_text();
        let proc_section = extract_marked_section(&text, "PROC_BEGIN", "PROC_END");
        assert!(
            proc_section
                .iter()
                .any(|line| line.starts_with("SigBlk:") && line.ends_with("0000000000000000")),
            "expected empty SigBlk mask in PTY child:\n{text}"
        );
        assert!(
            proc_section
                .iter()
                .any(|line| line.starts_with("SigIgn:") && line.ends_with("0000000000000000")),
            "expected empty SigIgn mask in PTY child:\n{text}"
        );
    }

    session
        .write_bytes(b"printf '\\nSIG_SLEEP_%s\\n' READY && sleep 30\n")
        .expect("start sleep");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "SIG_SLEEP_READY")
    });
    std::thread::sleep(Duration::from_millis(80));
    session.write_bytes(b"\x03").expect("send ctrl-c");
    std::thread::sleep(Duration::from_millis(120));
    session
        .write_bytes(b"printf '\\nSIG_AFTER_%s\\n' CTRL_C\n")
        .expect("marker after ctrl-c");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "SIG_AFTER_CTRL_C")
    });
}

#[test]
fn test_spawn_and_spawn_with_env_inject_grein_and_custom_vars() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Default spawn injects GREIN=1
    let signal_default = UpdateSignal::new();
    let session_default = InteractivePtySession::spawn(
        dir.path(),
        24,
        160,
        Some("/bin/sh"),
        signal_default.callback(),
    )
    .expect("spawn default pty session");

    session_default
        .write_bytes(b"printf '\\nDEF_GREIN=%s\\n' \"${GREIN:-UNSET}\"\n")
        .expect("print default GREIN");
    signal_default.wait_for(&session_default, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "DEF_GREIN=1")
    });

    // spawn_with_env injects GREIN=1 plus custom pairs
    let signal_custom = UpdateSignal::new();
    let session_custom = InteractivePtySession::spawn_with_env(
        dir.path(),
        24,
        160,
        Some("/bin/sh"),
        &[
            ("GREIN_THREAD_ID", "thread-99"),
            ("GREIN_WORKTREE", "/tmp/worktree-99"),
        ],
        signal_custom.callback(),
    )
    .expect("spawn_with_env pty session");

    session_custom
        .write_bytes(
            b"printf '\\nCUSTOM=%s|%s|%s\\n' \"${GREIN:-UNSET}\" \"${GREIN_THREAD_ID:-UNSET}\" \"${GREIN_WORKTREE:-UNSET}\"\n",
        )
        .expect("print custom env vars");
    signal_custom.wait_for(&session_custom, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "CUSTOM=1|thread-99|/tmp/worktree-99")
    });
}

#[cfg(target_os = "linux")]
#[test]
fn test_live_process_cwd_reads_proc_and_handles_missing_pid() {
    let current = std::fs::canonicalize(std::env::current_dir().expect("current_dir"))
        .expect("canonicalize current_dir");
    let live = live_process_cwd(std::process::id()).expect("live_process_cwd for self");
    assert_eq!(std::fs::canonicalize(live).expect("canon live"), current);
    assert_eq!(live_process_cwd(u32::MAX), None);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn test_poll_os_cwd_fallback_detects_cd_without_osc7() {
    let dir1 = tempfile::tempdir().expect("tempdir 1");
    let dir2 = tempfile::tempdir().expect("tempdir 2");
    let canon_dir2 = std::fs::canonicalize(dir2.path())
        .expect("canonicalize dir2")
        .to_string_lossy()
        .into_owned();

    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir1.path(), 24, 160, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    assert_eq!(session.snapshot().cwd, None);
    assert_eq!(session.poll_os_cwd(), None);

    let cd_cmd = format!("cd '{}' && printf '\\nCD_DONE_OK\\n'\n", dir2.path().display());
    session.write_bytes(cd_cmd.as_bytes()).expect("write cd");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|line| line.trim() == "CD_DONE_OK")
    });

    assert_eq!(
        session.snapshot().cwd,
        None,
        "snapshot().cwd should still be None without OSC 7 before poll_os_cwd"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let polled = loop {
        if let Some(cwd) = session.poll_os_cwd() {
            break cwd;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for poll_os_cwd to detect cd to {canon_dir2}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(polled, canon_dir2);
    assert_eq!(session.snapshot().cwd.as_deref(), Some(canon_dir2.as_str()));
    assert_eq!(session.poll_os_cwd(), None);

    session.write_bytes(b"exit 0\n").expect("exit shell");
    signal.wait_for(&session, Duration::from_secs(5), |s| s.exited);
    assert_eq!(session.poll_os_cwd(), None);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn test_poll_os_cwd_preserves_osc7_until_actual_cd() {
    let dir1 = tempfile::tempdir().expect("tempdir 1");
    let dir2 = tempfile::tempdir().expect("tempdir 2");
    let canon_dir2 = std::fs::canonicalize(dir2.path())
        .expect("canonicalize dir2")
        .to_string_lossy()
        .into_owned();

    let signal = UpdateSignal::new();
    let session =
        InteractivePtySession::spawn(dir1.path(), 24, 160, Some("/bin/sh"), signal.callback())
            .expect("spawn pty session");

    session
        .write_bytes(b"printf '\\033]7;file:///tmp/osc7-virtual\\007\\n'\n")
        .expect("emit osc7");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.cwd.as_deref() == Some("/tmp/osc7-virtual")
    });
    assert_eq!(session.poll_os_cwd(), None);
    assert_eq!(
        session.snapshot().cwd.as_deref(),
        Some("/tmp/osc7-virtual")
    );

    let cd_cmd = format!("cd '{}' && printf '\\nREAL_CD\\n'\n", dir2.path().display());
    session.write_bytes(cd_cmd.as_bytes()).expect("write cd");
    signal.wait_for(&session, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|line| line.trim() == "REAL_CD")
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let polled = loop {
        if let Some(cwd) = session.poll_os_cwd() {
            break cwd;
        }
        assert!(Instant::now() < deadline, "timed out waiting for poll_os_cwd");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(polled, canon_dir2);
    assert_eq!(session.snapshot().cwd.as_deref(), Some(canon_dir2.as_str()));
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
