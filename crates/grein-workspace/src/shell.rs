#[cfg(unix)]
use std::fs::File;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::{AsRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TRUNCATED_SUFFIX: &str = "\n...[truncated]";
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const TIMEOUT_DRAIN_WAIT: Duration = Duration::from_millis(50);
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
    #[cfg(unix)]
    wake_write: Option<OwnedFd>,
    #[cfg(unix)]
    shutdown: Arc<AtomicBool>,
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
            #[cfg(unix)]
            wake_write: None,
            #[cfg(unix)]
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(unix)]
    fn spawn_pty(
        master_fd: OwnedFd,
        mut keepalive: Option<File>,
        wake_read: OwnedFd,
        wake_write: OwnedFd,
        limit: usize,
    ) -> Self {
        let capture = Arc::new(Mutex::new(OutputCapture::new(limit, true)));
        let (done_tx, done_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let capture_ref = Arc::clone(&capture);
        let shutdown_ref = Arc::clone(&shutdown);
        let handle = Some(thread::spawn(move || {
            let mut wake_read = Some(wake_read);
            let mut buf = [0u8; 4096];
            loop {
                let wake_poll_fd = wake_read.as_ref().map_or(-1, AsRawFd::as_raw_fd);
                let wake_events = if wake_read.is_some() { libc::POLLIN } else { 0 };
                let mut fds = [
                    libc::pollfd {
                        fd: master_fd.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wake_poll_fd,
                        events: wake_events,
                        revents: 0,
                    },
                ];
                let prc = unsafe {
                    libc::poll(
                        fds.as_mut_ptr(),
                        fds.len() as libc::nfds_t,
                        crate::pty_session::unix::POLL_TIMEOUT_MS,
                    )
                };
                if prc < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }

                let mut hit_eof = false;
                loop {
                    let n = unsafe {
                        libc::read(master_fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len())
                    };
                    if n > 0 {
                        if let Ok(mut guard) = capture_ref.lock() {
                            guard.push(&buf[..n as usize]);
                        }
                        continue;
                    }
                    if n == 0 {
                        hit_eof = true;
                        break;
                    }
                    let err = io::Error::last_os_error();
                    match err.kind() {
                        io::ErrorKind::Interrupted => continue,
                        io::ErrorKind::WouldBlock => break,
                        _ => {
                            hit_eof = true;
                            break;
                        }
                    }
                }

                if hit_eof {
                    break;
                }

                if (fds[1].revents
                    & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL))
                    != 0
                {
                    wake_read = None;
                    if keepalive.take().is_some() {
                        continue;
                    }
                }

                if keepalive.is_none()
                    && ((fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0
                        || shutdown_ref.load(Ordering::Acquire))
                {
                    break;
                }
            }
            let _ = done_tx.send(());
        }));
        Self {
            capture,
            done_rx,
            handle,
            wake_write: Some(wake_write),
            shutdown,
        }
    }

    fn signal_child_exited(&mut self) {
        #[cfg(unix)]
        if let Some(wake_write) = self.wake_write.take() {
            let byte = [1u8; 1];
            let _ = unsafe {
                libc::write(wake_write.as_raw_fd(), byte.as_ptr().cast(), byte.len())
            };
            drop(wake_write);
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
        } else {
            #[cfg(unix)]
            {
                self.shutdown.store(true, Ordering::Release);
                self.signal_child_exited();
            }
        }
        self.capture
            .lock()
            .map(|mut guard| guard.format_output())
            .unwrap_or_default()
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            self.shutdown.store(true, Ordering::Release);
            self.signal_child_exited();
        }
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
    mut stdout_reader: PipeReader,
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
        stdout_reader.signal_child_exited();
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
        stdout_reader.signal_child_exited();
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

    let mut child = {
        #[cfg(unix)]
        let _spawn_guard = crate::pty_session::unix::lock_pty_spawn();
        cmd.spawn()?
    };
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
        let (master_fd, slave_keepalive, wake_read, wake_write, mut child) = {
            let spawn_guard = crate::pty_session::unix::lock_pty_spawn();
            let (master_fd, slave_fd) = match crate::pty_session::unix::open_pty_pair(40, 120) {
                Ok(pair) => pair,
                Err(_) => {
                    drop(spawn_guard);
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
            crate::pty_session::unix::set_nonblocking(master_fd.as_raw_fd())?;
            let (wake_read, wake_write) = crate::pty_session::unix::create_wake_pipe()?;
            let slave_file = File::from(slave_fd);
            let slave_keepalive = slave_file.try_clone().ok();
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
            (
                master_fd,
                slave_keepalive,
                wake_read,
                wake_write,
                spawn_result?,
            )
        };

        let stdout_reader = PipeReader::spawn_pty(
            master_fd,
            slave_keepalive,
            wake_read,
            wake_write,
            max_output_bytes,
        );
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
#[path = "shell_tests.rs"]
mod tests;
