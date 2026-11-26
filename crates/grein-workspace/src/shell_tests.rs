use super::*;

#[test]
fn test_empty_command_returns_invalid_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = run_shell_command_blocking(dir.path(), "   \n\t ", Duration::from_secs(1), 1024)
        .expect_err("empty command must fail");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn test_captures_stdout_and_stderr_and_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = run_shell_command_blocking(
        dir.path(),
        "printf 'hello stdout\\n'; printf 'hello stderr\\n' >&2; exit 7",
        Duration::from_secs(5),
        4096,
    )
    .expect("shell command runs");

    assert_eq!(
        output.command,
        "printf 'hello stdout\\n'; printf 'hello stderr\\n' >&2; exit 7"
    );
    assert_eq!(output.stdout, "hello stdout\n");
    assert_eq!(output.stderr, "hello stderr\n");
    assert_eq!(output.exit_code, 7);
    assert!(!output.timed_out);
}

#[test]
fn test_respects_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sub = dir.path().join("nested_cwd");
    std::fs::create_dir_all(&sub).expect("create sub");

    let output = run_shell_command_blocking(&sub, "pwd", Duration::from_secs(5), 4096)
        .expect("pwd succeeds");

    let expected = std::fs::canonicalize(&sub).expect("canonical sub");
    let actual = std::fs::canonicalize(output.stdout.trim()).expect("canonical actual");
    assert_eq!(actual, expected);
    assert_eq!(output.exit_code, 0);
    assert!(!output.timed_out);
}

#[test]
fn test_timeout_enforced_and_collects_partial_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let started = Instant::now();
    let output = run_shell_command_blocking(
        dir.path(),
        "printf 'before sleep\\n'; printf 'err before\\n' >&2; sleep 5; printf 'after\\n'",
        Duration::from_millis(80),
        4096,
    )
    .expect("timeout returns output struct");

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "timeout must return promptly without waiting for sleep"
    );
    assert!(output.timed_out);
    assert_eq!(output.exit_code, -1);
    assert!(
        output.stdout.contains("before sleep"),
        "stdout so far should be retained: {:?}",
        output.stdout
    );
    assert!(
        !output.stdout.contains("after"),
        "output after sleep must not appear: {:?}",
        output.stdout
    );
    assert!(
        output.stderr.contains("err before"),
        "stderr so far should be retained: {:?}",
        output.stderr
    );
}

#[test]
fn test_utf8_safe_truncation_on_stdout_and_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = run_shell_command_blocking(
        dir.path(),
        "printf 'ab🦀cd'; printf 'xy🦀zw' >&2",
        Duration::from_secs(5),
        4,
    )
    .expect("truncated command runs");

    assert_eq!(output.stdout, "ab\n...[truncated]");
    assert_eq!(output.stderr, "xy\n...[truncated]");
    assert_eq!(output.exit_code, 0);
    assert!(!output.timed_out);

    let exact =
        run_shell_command_blocking(dir.path(), "printf 'ab🦀cd'", Duration::from_secs(5), 8)
            .expect("exact fit runs");
    assert_eq!(exact.stdout, "ab🦀cd");
}

#[test]
fn test_large_output_beyond_limit_does_not_deadlock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = run_shell_command_blocking(
        dir.path(),
        "awk 'BEGIN { for (i = 0; i < 20000; i++) printf \"0123456789\" }'",
        Duration::from_secs(5),
        32,
    )
    .expect("large output command finishes cleanly");

    assert_eq!(output.exit_code, 0);
    assert!(!output.timed_out);
    assert_eq!(
        output.stdout,
        "01234567890123456789012345678901\n...[truncated]"
    );
}

#[test]
fn test_zero_max_output_bytes_and_missing_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");

    let empty = run_shell_command_blocking(dir.path(), "true", Duration::from_secs(5), 0)
        .expect("empty output with zero cap succeeds");
    assert_eq!(empty.stdout, "");
    assert_eq!(empty.stderr, "");

    let truncated =
        run_shell_command_blocking(dir.path(), "printf 'abc'", Duration::from_secs(5), 0)
            .expect("non-empty output with zero cap truncates");
    assert_eq!(truncated.stdout, "\n...[truncated]");

    let missing = dir.path().join("does-not-exist");
    let err = run_shell_command_blocking(&missing, "true", Duration::from_secs(5), 1024)
        .expect_err("missing cwd must fail");
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn test_pty_empty_command_and_missing_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = run_shell_command_with_pty_blocking(
        dir.path(),
        "   \n\t ",
        Duration::from_secs(1),
        1024,
    )
    .expect_err("empty command must fail");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

    let missing = dir.path().join("does-not-exist-pty");
    let err =
        run_shell_command_with_pty_blocking(&missing, "true", Duration::from_secs(5), 1024)
            .expect_err("missing cwd must fail");
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn test_pty_captures_separate_stdout_stderr_and_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = run_shell_command_with_pty_blocking(
        dir.path(),
        "printf 'line1\\nline2\\n'; printf 'err1\\n' >&2; exit 7",
        Duration::from_secs(5),
        4096,
    )
    .expect("pty shell command runs");

    assert_eq!(
        output.command,
        "printf 'line1\\nline2\\n'; printf 'err1\\n' >&2; exit 7"
    );
    assert_eq!(output.stdout, "line1\nline2\n");
    assert_eq!(output.stderr, "err1\n");
    assert_eq!(output.exit_code, 7);
    assert!(!output.timed_out);
}

#[test]
fn test_pty_timeout_enforced_and_collects_partial_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let started = Instant::now();
    let output = run_shell_command_with_pty_blocking(
        dir.path(),
        "printf 'before sleep\\n'; printf 'err before\\n' >&2; sleep 5; printf 'after\\n'",
        Duration::from_millis(80),
        4096,
    )
    .expect("pty timeout returns output struct");

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "timeout must return promptly without waiting for sleep"
    );
    assert!(output.timed_out);
    assert_eq!(output.exit_code, -1);
    assert!(
        output.stdout.contains("before sleep"),
        "stdout so far should be retained: {:?}",
        output.stdout
    );
    assert!(
        !output.stdout.contains("after"),
        "output after sleep must not appear: {:?}",
        output.stdout
    );
    assert!(
        output.stderr.contains("err before"),
        "stderr so far should be retained: {:?}",
        output.stderr
    );
}

#[test]
fn test_pty_utf8_safe_truncation_and_large_output_no_deadlock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = run_shell_command_with_pty_blocking(
        dir.path(),
        "printf 'ab🦀cd'; printf 'xy🦀zw' >&2",
        Duration::from_secs(5),
        4,
    )
    .expect("pty truncated command runs");

    assert_eq!(output.stdout, "ab\n...[truncated]");
    assert_eq!(output.stderr, "xy\n...[truncated]");
    assert_eq!(output.exit_code, 0);
    assert!(!output.timed_out);

    let crlf_fit = run_shell_command_with_pty_blocking(
        dir.path(),
        "printf 'hello\\n'",
        Duration::from_secs(5),
        6,
    )
    .expect("normalized newline fits within exact cap");
    assert_eq!(crlf_fit.stdout, "hello\n");

    let large = run_shell_command_with_pty_blocking(
        dir.path(),
        "awk 'BEGIN { for (i = 0; i < 20000; i++) printf \"0123456789\" }'",
        Duration::from_secs(5),
        32,
    )
    .expect("pty large output finishes cleanly");
    assert_eq!(large.exit_code, 0);
    assert!(!large.timed_out);
    assert_eq!(
        large.stdout,
        "01234567890123456789012345678901\n...[truncated]"
    );
}

#[test]
fn test_pty_sets_color_env_and_ls_emits_ansi_on_pty_but_not_pipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("subdir")).expect("mkdir");
    std::fs::write(dir.path().join("file.txt"), "hello").expect("write");

    let env_out = run_shell_command_with_pty_blocking(
        dir.path(),
        "printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\\n' \"$TERM\" \"$COLORTERM\" \"$GREIN\" \"$CLICOLOR\" \"$CLICOLOR_FORCE\" \"$FORCE_COLOR\" \"$CARGO_TERM_COLOR\" \"$GIT_PAGER\" \"$PAGER\"; test -n \"$LS_COLORS\" && echo has_ls_colors; test -n \"$LSCOLORS\" && echo has_macos_lscolors; test -t 1 && echo is_tty || echo not_tty",
        Duration::from_secs(5),
        4096,
    )
    .expect("env command runs");
    assert_eq!(env_out.exit_code, 0);
    assert!(
        env_out
            .stdout
            .contains("xterm-256color|truecolor|1|1||1|always|cat|cat"),
        "unexpected env output: {:?}",
        env_out.stdout
    );
    assert!(env_out.stdout.contains("has_ls_colors"));
    #[cfg(target_os = "macos")]
    assert!(env_out.stdout.contains("has_macos_lscolors"));

    let is_tty = env_out.stdout.lines().any(|l| l == "is_tty");
    let ls_out = run_shell_command_with_pty_blocking(
        dir.path(),
        "ls -latr",
        Duration::from_secs(5),
        8192,
    )
    .expect("ls runs");
    assert_eq!(ls_out.exit_code, 0);
    assert!(ls_out.stdout.contains("subdir"));
    assert!(ls_out.stdout.contains("file.txt"));
    if is_tty {
        assert!(
            ls_out.stdout.contains("\x1b["),
            "ls -latr on PTY should emit ANSI color sequences: {:?}",
            ls_out.stdout
        );
    }

    let cat_piped = run_shell_command_with_pty_blocking(
        dir.path(),
        "ls -1 | cat",
        Duration::from_secs(5),
        4096,
    )
    .expect("ls | cat runs");
    assert_eq!(cat_piped.exit_code, 0);
    assert!(
        !cat_piped.stdout.contains("\x1b["),
        "ls | cat had ANSI: {:?}",
        cat_piped.stdout
    );
    assert!(cat_piped.stdout.contains("subdir"));
    assert!(cat_piped.stdout.contains("file.txt"));

    let piped_out = run_shell_command_with_pty_blocking(
        dir.path(),
        "ls -1 | wc -l",
        Duration::from_secs(5),
        4096,
    )
    .expect("piped ls runs");
    assert_eq!(piped_out.exit_code, 0);
    assert!(!piped_out.stdout.contains("\x1b["));
    assert_eq!(piped_out.stdout.trim(), "2");

    let pipe_fallback = run_shell_command_with_color_pipes(
        dir.path(),
        "printf '%s|%s|%s|%s|%s|%s' \"$TERM\" \"$COLORTERM\" \"$GREIN\" \"$CLICOLOR\" \"$CLICOLOR_FORCE\" \"$FORCE_COLOR\"",
        Duration::from_secs(5),
        1024,
    )
    .expect("color pipes fallback runs");
    assert_eq!(
        pipe_fallback.stdout,
        "xterm-256color|truecolor|1|1|1|1"
    );
}

#[test]
fn test_is_posix_shell_and_build_pty_shell_script() {
    for shell in [
        "sh",
        "bash",
        "zsh",
        "dash",
        "ash",
        "ksh",
        "mksh",
        "/bin/zsh",
        "/usr/local/bin/BASH",
    ] {
        assert!(is_posix_shell(shell), "expected POSIX shell: {shell}");
    }
    for shell in ["/usr/bin/fish", "fish", "nushell", "pwsh", "csh", "tcsh", "", "/"] {
        assert!(!is_posix_shell(shell), "expected non-POSIX shell: {shell}");
    }

    assert_eq!(
        build_pty_shell_script("/usr/bin/fish", "ls | cat"),
        "ls | cat"
    );
    let posix_script = build_pty_shell_script("/bin/zsh", "echo hi");
    assert!(posix_script.ends_with("; echo hi"));
    #[cfg(target_os = "linux")]
    assert!(posix_script.contains("ls() { command ls --color=auto \"$@\"; }"));
    #[cfg(target_os = "macos")]
    assert!(!posix_script.contains("ls() {"));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert!(posix_script.contains("grep() { command grep --color=auto \"$@\"; }"));
}

#[cfg(unix)]
#[test]
fn test_pty_and_pipe_scrub_terminal_env_vars_isolated() {
    const FLAG: &str = "GREIN_SHELL_ENV_SUBPROCESS";
    if std::env::var_os(FLAG).is_none() {
        let exe = std::env::current_exe().expect("test binary");
        let mut cmd = Command::new(exe);
        cmd.arg("--exact")
            .arg("shell::tests::test_pty_and_pipe_scrub_terminal_env_vars_isolated")
            .arg("--nocapture")
            .env(FLAG, "1")
            .env("CLICOLOR_FORCE", "1")
            .env("GREIN_KEEP_TEST", "yes");
        for &var in crate::pty_session::unix::SCRUBBED_TERMINAL_ENV_VARS {
            cmd.env(var, format!("outer_{var}"));
        }
        let out = cmd.output().expect("run isolated test");
        assert!(
            out.status.success(),
            "isolated test failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let mut script = String::new();
    for &var in crate::pty_session::unix::SCRUBBED_TERMINAL_ENV_VARS {
        script.push_str(&format!(
            "eval \"v=\\${{{var}+SET}}\"; printf '{var}=%s\\n' \"${{v:-UNSET}}\"; "
        ));
    }
    script.push_str(
        "printf 'KEEP=%s|TERM=%s|COLORTERM=%s|GREIN=%s|CF=%s\\n' \"$GREIN_KEEP_TEST\" \"$TERM\" \"$COLORTERM\" \"$GREIN\" \"${CLICOLOR_FORCE:-UNSET}\"",
    );

    let pty_out = run_shell_command_with_pty_and_shell_blocking(
        dir.path(),
        &script,
        Duration::from_secs(5),
        8192,
        Some("/bin/sh"),
    )
    .expect("pty env check");
    let pipe_out = run_pipe_command(
        dir.path(),
        &script,
        Duration::from_secs(5),
        8192,
        Some("/bin/sh"),
        true,
    )
    .expect("pipe env check");

    for (label, out, expected_cf) in [
        ("pty", &pty_out, "UNSET"),
        ("pipe", &pipe_out, "1"),
    ] {
        assert_eq!(out.exit_code, 0, "{label} failed: {}", out.stderr);
        for &var in crate::pty_session::unix::SCRUBBED_TERMINAL_ENV_VARS {
            let needle = format!("{var}=UNSET");
            assert!(
                out.stdout.lines().any(|l| l.trim() == needle),
                "{label} did not scrub {var}: {:?}",
                out.stdout
            );
        }
        let expected_tail =
            format!("KEEP=yes|TERM=xterm-256color|COLORTERM=truecolor|GREIN=1|CF={expected_cf}");
        assert!(
            out.stdout.lines().any(|l| l.trim() == expected_tail),
            "{label} env mismatch: {:?}",
            out.stdout
        );
    }
}

#[cfg(unix)]
#[test]
fn test_pipe_command_session_isolation_signal_exit_and_grandchild_kill() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent_sid = unsafe { libc::getsid(0) };

    let sig = run_pipe_command(
        dir.path(),
        "kill -9 $$",
        Duration::from_secs(5),
        1024,
        Some("/bin/sh"),
        false,
    )
    .expect("pipe kill -9");
    assert!(!sig.timed_out);
    assert_eq!(sig.exit_code, 137);

    let tty = run_pipe_command(
        dir.path(),
        "( exec < /dev/tty ) 2>/dev/null && echo HAS_TTY || echo NO_TTY",
        Duration::from_secs(5),
        1024,
        Some("/bin/sh"),
        false,
    )
    .expect("pipe tty check");
    assert_eq!(tty.stdout.trim(), "NO_TTY");
    assert_ne!(parent_sid, 0);

    let timeout_out = run_pipe_command(
        dir.path(),
        "sleep 30 & printf 'GC=%s\\n' \"$!\"; wait",
        Duration::from_millis(250),
        4096,
        Some("/bin/sh"),
        false,
    )
    .expect("pipe timeout");
    assert!(timeout_out.timed_out);
    assert_eq!(timeout_out.exit_code, -1);
    let gc_pid: libc::pid_t = timeout_out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("GC="))
        .expect("gc pid")
        .trim()
        .parse()
        .expect("valid pid");
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let rc = unsafe { libc::kill(gc_pid, 0) };
        if rc == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            break;
        }
        assert!(Instant::now() < deadline, "grandchild {gc_pid} survived");
        thread::sleep(Duration::from_millis(5));
    }
}
