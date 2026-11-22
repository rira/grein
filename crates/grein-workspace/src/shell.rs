use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TRUNCATED_SUFFIX: &str = "\n...[truncated]";
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const TIMEOUT_DRAIN_WAIT: Duration = Duration::from_millis(25);
#[cfg(unix)]
const PTY_EXIT_DRAIN_WAIT: Duration = Duration::from_millis(250);
const DEFAULT_LS_COLORS: &str =
    "di=01;34:ln=01;36:pi=33:so=01;35:bd=01;33:cd=01;33:or=01;31:ex=01;32:";

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

#[cfg(unix)]
mod pty {
    use std::ffi::{c_char, c_int, c_ulong, c_ushort, CStr};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::PathBuf;

    const O_RDWR: c_int = 2;
    #[cfg(target_os = "linux")]
    const O_NOCTTY: c_int = 0o400;
    #[cfg(target_os = "macos")]
    const O_NOCTTY: c_int = 0x20000;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    const O_NOCTTY: c_int = 0;

    #[cfg(target_os = "linux")]
    const O_CLOEXEC: c_int = 0o2000000;

    const F_SETFD: c_int = 2;
    const FD_CLOEXEC: c_int = 1;

    #[cfg(all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "x86",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "s390x",
            target_arch = "loongarch64"
        )
    ))]
    const TIOCSWINSZ: c_ulong = 0x5414;

    #[cfg(target_env = "musl")]
    #[allow(dead_code)]
    type IoctlRequest = c_int;
    #[cfg(not(target_env = "musl"))]
    #[allow(dead_code)]
    type IoctlRequest = c_ulong;

    #[repr(C)]
    #[allow(dead_code)]
    struct WinSize {
        ws_row: c_ushort,
        ws_col: c_ushort,
        ws_xpixel: c_ushort,
        ws_ypixel: c_ushort,
    }

    extern "C" {
        fn posix_openpt(flags: c_int) -> c_int;
        fn grantpt(fd: c_int) -> c_int;
        fn unlockpt(fd: c_int) -> c_int;
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        #[cfg(target_os = "linux")]
        fn ptsname_r(fd: c_int, buf: *mut c_char, buflen: usize) -> c_int;
        #[cfg(not(target_os = "linux"))]
        fn ptsname(fd: c_int) -> *const c_char;
        #[cfg(all(
            target_os = "linux",
            any(
                target_arch = "x86_64",
                target_arch = "aarch64",
                target_arch = "x86",
                target_arch = "arm",
                target_arch = "riscv64",
                target_arch = "s390x",
                target_arch = "loongarch64"
            )
        ))]
        fn ioctl(fd: c_int, request: IoctlRequest, ...) -> c_int;
    }

    #[cfg(not(target_os = "linux"))]
    static PTSNAME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn open_master_fd() -> io::Result<OwnedFd> {
        #[cfg(target_os = "linux")]
        let raw_fd = {
            let fd = unsafe { posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC) };
            if fd < 0 && io::Error::last_os_error().kind() == io::ErrorKind::InvalidInput {
                unsafe { posix_openpt(O_RDWR | O_NOCTTY) }
            } else {
                fd
            }
        };
        #[cfg(not(target_os = "linux"))]
        let raw_fd = unsafe { posix_openpt(O_RDWR | O_NOCTTY) };

        if raw_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let _ = unsafe { fcntl(owned.as_raw_fd(), F_SETFD, FD_CLOEXEC) };
        Ok(owned)
    }

    fn slave_pty_path(master_fd: &OwnedFd) -> io::Result<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            let mut buf = [0 as c_char; 256];
            let rc = unsafe { ptsname_r(master_fd.as_raw_fd(), buf.as_mut_ptr(), buf.len()) };
            if rc != 0 {
                return Err(if rc > 0 {
                    io::Error::from_raw_os_error(rc)
                } else {
                    io::Error::last_os_error()
                });
            }
            let path = unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_str()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(PathBuf::from(path))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _guard = PTSNAME_LOCK
                .lock()
                .map_err(|_| io::Error::other("ptsname lock poisoned"))?;
            let ptr = unsafe { ptsname(master_fd.as_raw_fd()) };
            if ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            let path = unsafe { CStr::from_ptr(ptr) }
                .to_str()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(PathBuf::from(path))
        }
    }

    pub fn open_pty_pair() -> io::Result<(File, File)> {
        let master_fd = open_master_fd()?;
        if unsafe { grantpt(master_fd.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { unlockpt(master_fd.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let slave_path = slave_pty_path(&master_fd)?;

        #[cfg(all(
            target_os = "linux",
            any(
                target_arch = "x86_64",
                target_arch = "aarch64",
                target_arch = "x86",
                target_arch = "arm",
                target_arch = "riscv64",
                target_arch = "s390x",
                target_arch = "loongarch64"
            )
        ))]
        {
            let ws = WinSize {
                ws_row: 40,
                ws_col: 120,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let _ = unsafe { ioctl(master_fd.as_raw_fd(), TIOCSWINSZ as IoctlRequest, &ws) };
        }

        let slave_file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NOCTTY)
            .open(&slave_path)?;

        Ok((File::from(master_fd), slave_file))
    }
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

fn apply_color_env(cmd: &mut Command) {
    cmd.env("TERM", "xterm-256color")
        .env("CLICOLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("FORCE_COLOR", "1")
        .env("CARGO_TERM_COLOR", "always")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat");
    if std::env::var_os("LS_COLORS").is_none() {
        cmd.env("LS_COLORS", DEFAULT_LS_COLORS);
    }
}

fn build_pty_shell_script(command: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        format!(
            "ls() {{ command ls --color=auto \"$@\"; }}; grep() {{ command grep --color=auto \"$@\"; }}; {command}"
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        command.to_string()
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
            exit_code: status.code().unwrap_or(-1),
            timed_out: false,
        })
    } else {
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

fn run_shell_command_with_color_pipes(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<ShellCommandOutput> {
    validate_command(command)?;

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_color_env(&mut cmd);

    let mut child = cmd.spawn()?;
    let stdout_reader = PipeReader::spawn(child.stdout.take(), max_output_bytes, false);
    let stderr_reader = PipeReader::spawn(child.stderr.take(), max_output_bytes, false);
    wait_and_collect(
        child,
        command,
        stdout_reader,
        stderr_reader,
        timeout,
        None,
    )
}

pub fn run_shell_command_with_pty_blocking(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<ShellCommandOutput> {
    validate_command(command)?;

    #[cfg(unix)]
    {
        let (master_file, slave_file) = match pty::open_pty_pair() {
            Ok(pair) => pair,
            Err(_) => {
                return run_shell_command_with_color_pipes(
                    cwd,
                    command,
                    timeout,
                    max_output_bytes,
                );
            }
        };

        let script = build_pty_shell_script(command);
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&script)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(slave_file))
            .stderr(Stdio::piped());
        apply_color_env(&mut cmd);

        let mut child = cmd.spawn()?;
        drop(cmd);

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
        run_shell_command_with_color_pipes(cwd, command, timeout, max_output_bytes)
    }
}

pub fn run_shell_command_blocking(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<ShellCommandOutput> {
    validate_command(command)?;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout_reader = PipeReader::spawn(child.stdout.take(), max_output_bytes, false);
    let stderr_reader = PipeReader::spawn(child.stderr.take(), max_output_bytes, false);
    wait_and_collect(
        child,
        command,
        stdout_reader,
        stderr_reader,
        timeout,
        None,
    )
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
            "printf '%s|%s|%s|%s|%s|%s\\n' \"$TERM\" \"$CLICOLOR\" \"$CLICOLOR_FORCE\" \"$FORCE_COLOR\" \"$CARGO_TERM_COLOR\" \"$PAGER\"; test -n \"$LS_COLORS\" && echo has_ls_colors; test -t 1 && echo is_tty || echo not_tty",
            Duration::from_secs(5),
            4096,
        )
        .expect("env command runs");
        assert_eq!(env_out.exit_code, 0);
        assert!(env_out
            .stdout
            .contains("xterm-256color|1|1|1|always|cat"));
        assert!(env_out.stdout.contains("has_ls_colors"));

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
            "printf '%s|%s' \"$TERM\" \"$FORCE_COLOR\"",
            Duration::from_secs(5),
            1024,
        )
        .expect("color pipes fallback runs");
        assert_eq!(pipe_fallback.stdout, "xterm-256color|1");
    }
}
