#[cfg(unix)]
use std::fs::File;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TRUNCATED_SUFFIX: &str = "\n...[truncated]";
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const TIMEOUT_DRAIN_WAIT: Duration = Duration::from_millis(25);
const PTY_EXIT_DRAIN_WAIT: Duration = Duration::from_millis(250);
const DEFAULT_LS_COLORS: &str =
    "di=01;34:ln=01;36:pi=33:so=01;35:bd=01;33:cd=01;33:or=01;31:ex=01;32:";
#[cfg(target_os = "macos")]
const DEFAULT_MACOS_LSCOLORS: &str = "ExFxBxDxCxegedabagacad";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandOutput {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

struct OutputCapture {
    limit: usize,
    bytes: Vec<u8>,
    exceeded: bool,
    normalize_crlf: bool,
    pending_cr: bool,
}

impl OutputCapture {
    fn new(limit: usize, normalize_crlf: bool) -> Self {
        Self {
            limit,
            bytes: Vec::new(),
            exceeded: false,
            normalize_crlf,
            pending_cr: false,
        }
    }

    fn push_raw_byte(&mut self, byte: u8) {
        let cap = self.limit.saturating_add(1);
        if self.bytes.len() < cap {
            self.bytes.push(byte);
        } else {
            self.exceeded = true;
        }
    }

    fn push_raw_slice(&mut self, chunk: &[u8]) {
        let cap = self.limit.saturating_add(1);
        if self.bytes.len() < cap {
            let remaining = cap - self.bytes.len();
            let take = remaining.min(chunk.len());
            self.bytes.extend_from_slice(&chunk[..take]);
            if chunk.len() > take {
                self.exceeded = true;
            }
        } else if !chunk.is_empty() {
            self.exceeded = true;
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if !self.normalize_crlf {
            self.push_raw_slice(chunk);
            return;
        }
        for &b in chunk {
            if self.pending_cr {
                if b == b'\n' {
                    self.pending_cr = false;
                    self.push_raw_byte(b'\n');
                } else if b == b'\r' {
                    self.push_raw_byte(b'\r');
                } else {
                    self.pending_cr = false;
                    self.push_raw_byte(b'\r');
                    self.push_raw_byte(b);
                }
            } else if b == b'\r' {
                self.pending_cr = true;
            } else {
                self.push_raw_byte(b);
            }
        }
    }

    fn finalize(&mut self) {
        if self.pending_cr {
            self.pending_cr = false;
            self.push_raw_byte(b'\r');
        }
    }

    fn format_output(&mut self) -> String {
        self.finalize();
        let text = String::from_utf8_lossy(&self.bytes);
        if text.len() <= self.limit && !self.exceeded {
            return text.into_owned();
        }
        let mut end = self.limit.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut out = String::with_capacity(end + TRUNCATED_SUFFIX.len());
        out.push_str(&text[..end]);
        out.push_str(TRUNCATED_SUFFIX);
        out
    }
}

struct PipeReader {
    capture: Arc<Mutex<OutputCapture>>,
    done_rx: mpsc::Receiver<()>,
    handle: Option<JoinHandle<()>>,
}

impl PipeReader {
    fn spawn<R: Read + Send + 'static>(
        pipe: Option<R>,
        limit: usize,
        normalize_crlf: bool,
    ) -> Self {
        let capture = Arc::new(Mutex::new(OutputCapture::new(limit, normalize_crlf)));
        let (done_tx, done_rx) = mpsc::channel();
        let handle = match pipe {
            Some(mut reader) => {
                let capture_ref = Arc::clone(&capture);
                Some(thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Ok(mut guard) = capture_ref.lock() {
                                    guard.push(&buf[..n]);
                                }
                            }
                            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                    let _ = done_tx.send(());
                }))
            }
            None => {
                let _ = done_tx.send(());
                None
            }
        };
        Self {
            capture,
            done_rx,
            handle,
        }
    }

    fn finish_until(mut self, drain_deadline: Option<Instant>) -> String {
        let finished = match drain_deadline {
            None => true,
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                self.done_rx.recv_timeout(wait).is_ok()
            }
        };
        if finished {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
        self.capture
            .lock()
            .map(|mut guard| guard.format_output())
            .unwrap_or_default()
    }
}

fn resolve_shell(shell_override: Option<&str>) -> String {
    #[cfg(unix)]
    {
        crate::pty_session::unix::resolve_shell(shell_override)
    }
    #[cfg(not(unix))]
    {
        if let Some(shell) = shell_override.filter(|s| !s.trim().is_empty()) {
            return shell.to_string();
        }
        if let Ok(shell) = std::env::var("SHELL") {
            if !shell.trim().is_empty() {
                return shell;
            }
        }
        "sh".to_string()
    }
}

fn exit_status_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        crate::pty_session::unix::exit_status_code(status)
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(-1)
    }
}

fn is_posix_shell(shell: &str) -> bool {
    let Some(stem) = Path::new(shell.trim()).file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "sh" | "bash" | "zsh" | "dash" | "ash" | "ksh" | "mksh"
    )
}

fn validate_command(command: &str) -> io::Result<()> {
    if command.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "command must not be empty",
        ));
    }
    Ok(())
}

fn apply_color_env(cmd: &mut Command, is_pty: bool) {
    #[cfg(unix)]
    for &var in crate::pty_session::unix::SCRUBBED_TERMINAL_ENV_VARS {
        cmd.env_remove(var);
    }
    cmd.env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("GREIN", "1")
        .env("CLICOLOR", "1")
        .env("FORCE_COLOR", "1")
        .env("CARGO_TERM_COLOR", "always")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat");
    if is_pty {
        cmd.env_remove("CLICOLOR_FORCE");
    } else {
        cmd.env("CLICOLOR_FORCE", "1");
    }
    if std::env::var_os("LS_COLORS").is_none() {
        cmd.env("LS_COLORS", DEFAULT_LS_COLORS);
    }
    #[cfg(target_os = "macos")]
    if std::env::var_os("LSCOLORS").is_none() {
        cmd.env("LSCOLORS", DEFAULT_MACOS_LSCOLORS);
    }
}

fn build_pty_shell_script(shell: &str, command: &str) -> String {
    if !is_posix_shell(shell) {
        return command.to_string();
    }
    #[cfg(target_os = "linux")]
    {
        format!(
            "ls() {{ command ls --color=auto \"$@\"; }}; grep() {{ command grep --color=auto \"$@\"; }}; {command}"
        )
    }
    #[cfg(target_os = "macos")]
    {
        format!("grep() {{ command grep --color=auto \"$@\"; }}; {command}")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        command.to_string()
    }
}

#[cfg(unix)]
fn attach_session_isolation(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            crate::pty_session::unix::reset_child_signals()
        });
    }
}

fn wait_and_collect(
    mut child: Child,
    command: &str,
    stdout_reader: PipeReader,
    stderr_reader: PipeReader,
    timeout: Duration,
    normal_drain_wait: Option<Duration>,
) -> io::Result<ShellCommandOutput> {
    let deadline = Instant::now() + timeout;
    let exit_status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        let now = Instant::now();
        if now >= deadline {
            break None;
        }
        let remaining = deadline.saturating_duration_since(now);
        thread::sleep(remaining.min(POLL_INTERVAL));
    };

    if let Some(status) = exit_status {
        let drain_deadline = normal_drain_wait.map(|wait| Instant::now() + wait);
        Ok(ShellCommandOutput {
            command: command.to_string(),
            stdout: stdout_reader.finish_until(drain_deadline),
            stderr: stderr_reader.finish_until(drain_deadline),
            exit_code: exit_status_code(status),
            timed_out: false,
        })
    } else {
        #[cfg(unix)]
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            if pid > 0 {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        let drain_deadline = Instant::now() + TIMEOUT_DRAIN_WAIT;
        Ok(ShellCommandOutput {
            command: command.to_string(),
            stdout: stdout_reader.finish_until(Some(drain_deadline)),
            stderr: stderr_reader.finish_until(Some(drain_deadline)),
            exit_code: -1,
            timed_out: true,
        })
    }
}

fn run_pipe_command(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
    shell_override: Option<&str>,
    with_color_env: bool,
) -> io::Result<ShellCommandOutput> {
    validate_command(command)?;

    let shell = resolve_shell(shell_override);
    let mut cmd = Command::new(&shell);
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if with_color_env {
        apply_color_env(&mut cmd, false);
    }
    #[cfg(unix)]
    attach_session_isolation(&mut cmd);

    let mut child = cmd.spawn()?;
    let stdout_reader = PipeReader::spawn(child.stdout.take(), max_output_bytes, false);
    let stderr_reader = PipeReader::spawn(child.stderr.take(), max_output_bytes, false);
    wait_and_collect(
        child,
        command,
        stdout_reader,
        stderr_reader,
        timeout,
        Some(PTY_EXIT_DRAIN_WAIT),
    )
}

#[cfg(test)]
fn run_shell_command_with_color_pipes(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<ShellCommandOutput> {
    run_pipe_command(cwd, command, timeout, max_output_bytes, None, true)
}

pub fn run_shell_command_with_pty_and_shell_blocking(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
    shell_override: Option<&str>,
) -> io::Result<ShellCommandOutput> {
    validate_command(command)?;

    #[cfg(unix)]
    {
        let (master_fd, slave_fd) = match crate::pty_session::unix::open_pty_pair(40, 120) {
            Ok(pair) => pair,
            Err(_) => {
                return run_pipe_command(
                    cwd,
                    command,
                    timeout,
                    max_output_bytes,
                    shell_override,
                    true,
                );
            }
        };
        let master_file = File::from(master_fd);
        let slave_file = File::from(slave_fd);

        let shell = resolve_shell(shell_override);
        let script = build_pty_shell_script(&shell, command);
        let mut cmd = Command::new(&shell);
        cmd.arg("-c")
            .arg(&script)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(slave_file))
            .stderr(Stdio::piped());
        apply_color_env(&mut cmd, true);
        attach_session_isolation(&mut cmd);

        let spawn_result = cmd.spawn();
        drop(cmd);
        let mut child = spawn_result?;

        let stdout_reader = PipeReader::spawn(Some(master_file), max_output_bytes, true);
        let stderr_reader = PipeReader::spawn(child.stderr.take(), max_output_bytes, false);
        wait_and_collect(
            child,
            command,
            stdout_reader,
            stderr_reader,
            timeout,
            Some(PTY_EXIT_DRAIN_WAIT),
        )
    }

    #[cfg(not(unix))]
    {
        run_pipe_command(cwd, command, timeout, max_output_bytes, shell_override, true)
    }
}

pub fn run_shell_command_with_pty_blocking(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<ShellCommandOutput> {
    run_shell_command_with_pty_and_shell_blocking(cwd, command, timeout, max_output_bytes, None)
}

pub fn run_shell_command_blocking(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<ShellCommandOutput> {
    run_pipe_command(cwd, command, timeout, max_output_bytes, None, false)
}

#[cfg(test)]
mod tests {
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
}
