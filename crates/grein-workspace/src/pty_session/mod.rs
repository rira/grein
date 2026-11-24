#[cfg(any(unix, test))]
mod reader;
#[cfg(any(unix, test))]
mod scanner;
mod state;
#[cfg(test)]
mod tests;
#[cfg(unix)]
mod unix;

use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(unix)]
use std::thread;
use std::thread::JoinHandle;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use self::reader::spawn_reader_thread;
use self::state::PtyState;
#[cfg(unix)]
use self::unix::{
    build_pty_command, create_wake_pipe, open_pty_pair, reset_child_signals, resolve_shell,
    send_signal_to_process_group, set_nonblocking, set_pty_winsize, write_all_to_fd, ChildProcess,
};

pub use state::{PtyCell, PtyColor, PtyMouseEncoding, PtyMouseMode, PtyScreenSnapshot};

#[cfg(unix)]
const DROP_SIGHUP_GRACE: Duration = Duration::from_millis(100);
#[cfg(unix)]
const DROP_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Reads the current working directory of an OS process ID when supported.
pub fn live_process_cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(target_os = "macos")]
    {
        let pid_str = pid.to_string();
        let output = Command::new("lsof")
            .args(["-a", "-d", "cwd", "-p", &pid_str, "-Fn"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .find_map(|line| line.strip_prefix('n'))
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Interactive PTY-backed terminal session with an in-memory VT100 screen parser.
pub struct InteractivePtySession {
    #[cfg(unix)]
    master: Arc<OwnedFd>,
    #[cfg(unix)]
    write_lock: Arc<Mutex<()>>,
    state: Arc<Mutex<PtyState>>,
    #[cfg(unix)]
    child: Arc<Mutex<ChildProcess>>,
    shutdown: Arc<AtomicBool>,
    #[cfg(unix)]
    wake_write: Option<OwnedFd>,
    reader_thread: Option<JoinHandle<()>>,
    on_update: Arc<dyn Fn() + Send + Sync>,
    pid: u32,
}

impl std::fmt::Debug for InteractivePtySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractivePtySession")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl InteractivePtySession {
    /// Spawns an interactive shell in a new PTY session rooted at `cwd`.
    pub fn spawn(
        cwd: &Path,
        rows: u16,
        cols: u16,
        shell: Option<&str>,
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        Self::spawn_with_env(cwd, rows, cols, shell, &[], on_update)
    }

    /// Spawns an interactive shell in a new PTY session rooted at `cwd` with extra environment variables.
    pub fn spawn_with_env(
        cwd: &Path,
        rows: u16,
        cols: u16,
        shell: Option<&str>,
        extra_env: &[(&str, &str)],
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Self::spawn_unix(cwd, rows, cols, shell, extra_env, on_update)
        }
        #[cfg(not(unix))]
        {
            let _ = (cwd, rows, cols, shell, extra_env, on_update);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "InteractivePtySession is only supported on Unix platforms",
            ))
        }
    }

    #[cfg(unix)]
    fn spawn_unix(
        cwd: &Path,
        rows: u16,
        cols: u16,
        shell_override: Option<&str>,
        extra_env: &[(&str, &str)],
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let shell = resolve_shell(shell_override);

        let (master_fd, slave_fd) = open_pty_pair(rows, cols)?;
        set_nonblocking(master_fd.as_raw_fd())?;
        let (wake_read_fd, wake_write_fd) = create_wake_pipe()?;

        let slave_raw_fd = slave_fd.as_raw_fd();
        let mut cmd = build_pty_command(&shell, cwd, extra_env);

        unsafe {
            cmd.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                #[allow(clippy::unnecessary_cast)]
                if libc::ioctl(slave_raw_fd, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let _ = libc::tcsetpgrp(slave_raw_fd, libc::getpid());
                for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
                    if slave_raw_fd == target {
                        if libc::fcntl(target, libc::F_SETFD, 0) < 0 {
                            return Err(io::Error::last_os_error());
                        }
                    } else if libc::dup2(slave_raw_fd, target) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                if slave_raw_fd > 2 {
                    libc::close(slave_raw_fd);
                }
                reset_child_signals()
            });
        }

        let spawn_result = cmd.spawn();
        drop(slave_fd);
        let child = spawn_result?;

        let pid = child.id();
        let pid_t = libc::pid_t::try_from(pid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "child PID overflow"))?;
        drop(child);

        let master = Arc::new(master_fd);
        let write_lock = Arc::new(Mutex::new(()));
        let mut pty_state = PtyState::new(rows, cols);
        pty_state.last_os_cwd = std::fs::canonicalize(cwd)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        let state = Arc::new(Mutex::new(pty_state));
        let child_proc = Arc::new(Mutex::new(ChildProcess {
            pid: pid_t,
            reaped: false,
            exit_code: None,
        }));
        let shutdown = Arc::new(AtomicBool::new(false));

        let reader_thread = spawn_reader_thread(
            Arc::clone(&master),
            wake_read_fd,
            Arc::clone(&write_lock),
            Arc::clone(&state),
            Arc::clone(&child_proc),
            Arc::clone(&shutdown),
            Arc::clone(&on_update),
        )?;

        Ok(Self {
            master,
            write_lock,
            state,
            child: child_proc,
            shutdown,
            wake_write: Some(wake_write_fd),
            reader_thread: Some(reader_thread),
            on_update,
            pid,
        })
    }

    /// Writes raw input bytes (keystrokes, control sequences, or pasted text) to the PTY master.
    pub fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        #[cfg(unix)]
        {
            if !bytes.is_empty() {
                let snapped = {
                    let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    if guard.scrollback_offset > 0 {
                        guard.scrollback_offset = 0;
                        true
                    } else {
                        false
                    }
                };
                if snapped {
                    (self.on_update)();
                }
            }
            write_all_to_fd(
                self.master.as_raw_fd(),
                &self.write_lock,
                &self.shutdown,
                bytes,
            )
        }
        #[cfg(not(unix))]
        {
            let _ = bytes;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "InteractivePtySession is only supported on Unix platforms",
            ))
        }
    }

    /// Resizes both the kernel PTY window dimensions and the VT100 parser screen.
    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        #[cfg(unix)]
        set_pty_winsize(self.master.as_raw_fd(), rows, cols)?;
        {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.resize(rows, cols);
        }
        (self.on_update)();
        Ok(())
    }

    /// Sets the scrollback viewport offset (clamped to available scrollback rows).
    pub fn set_scrollback(&self, offset: usize) -> usize {
        let resolved = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.clamp_scrollback_offset(offset)
        };
        (self.on_update)();
        resolved
    }

    /// Adjusts the scrollback viewport offset by `delta` rows (positive scrolls up).
    pub fn scroll_by(&self, delta: isize) -> usize {
        let resolved = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let next = if delta >= 0 {
                guard
                    .scrollback_offset
                    .saturating_add(usize::try_from(delta).unwrap_or(0))
            } else {
                guard.scrollback_offset.saturating_sub(delta.unsigned_abs())
            };
            guard.clamp_scrollback_offset(next)
        };
        (self.on_update)();
        resolved
    }

    /// Captures a consistent snapshot of the current VT100 screen and exit state.
    pub fn snapshot(&self) -> PtyScreenSnapshot {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot()
    }

    /// Returns the OS process ID of the spawned shell child process.
    pub fn child_pid(&self) -> u32 {
        self.pid
    }

    /// Polls the child process OS working directory as a fallback when OSC 7 is unavailable.
    pub fn poll_os_cwd(&self) -> Option<String> {
        {
            let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.exited {
                return None;
            }
        }

        let raw_cwd = live_process_cwd(self.pid)?;
        let canon = std::fs::canonicalize(raw_cwd).ok()?;
        if !canon.is_dir() {
            return None;
        }
        let canon_str = canon.to_string_lossy().into_owned();

        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.exited || guard.last_os_cwd.as_deref() == Some(canon_str.as_str()) {
            return None;
        }
        guard.last_os_cwd = Some(canon_str.clone());
        guard.current_cwd = Some(canon_str.clone());
        Some(canon_str)
    }
}

impl Drop for InteractivePtySession {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);

        #[cfg(unix)]
        {
            if let Some(wake_write) = self.wake_write.take() {
                let byte = [1u8; 1];
                let _ = unsafe {
                    libc::write(wake_write.as_raw_fd(), byte.as_ptr().cast(), byte.len())
                };
                drop(wake_write);
            }

            {
                let fg_pgid = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
                let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
                if !child.reaped && child.try_wait().is_none() {
                    send_signal_to_process_group(child.pid, libc::SIGHUP);
                    if fg_pgid > 1 && fg_pgid != child.pid {
                        send_signal_to_process_group(fg_pgid, libc::SIGHUP);
                    }

                    let deadline = Instant::now() + DROP_SIGHUP_GRACE;
                    while Instant::now() < deadline {
                        if child.try_wait().is_some() || child.reaped {
                            break;
                        }
                        thread::sleep(DROP_POLL_INTERVAL);
                    }

                    if !child.reaped && child.try_wait().is_none() {
                        send_signal_to_process_group(child.pid, libc::SIGKILL);
                        if fg_pgid > 1 && fg_pgid != child.pid {
                            send_signal_to_process_group(fg_pgid, libc::SIGKILL);
                        }
                        let _ = child.wait_blocking();
                    }
                }
            }
        }

        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}
