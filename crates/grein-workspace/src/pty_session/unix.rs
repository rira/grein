use std::ffi::{c_char, CStr};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub(super) const POLL_TIMEOUT_MS: i32 = 100;

pub(crate) const SCRUBBED_TERMINAL_ENV_VARS: &[&str] = &[
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

const RESET_CHILD_SIGNALS: &[libc::c_int] = &[
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
    libc::SIGCHLD,
    libc::SIGPIPE,
];

#[cfg(all(unix, not(target_os = "linux")))]
static PTSNAME_LOCK: Mutex<()> = Mutex::new(());

pub(super) struct ChildProcess {
    pub(super) pid: libc::pid_t,
    pub(super) reaped: bool,
    pub(super) exit_code: Option<i32>,
}

impl ChildProcess {
    pub(super) fn try_wait(&mut self) -> Option<i32> {
        if self.reaped {
            return self.exit_code;
        }
        let mut status: libc::c_int = 0;
        loop {
            let rc = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if rc == self.pid {
                let code = decode_wait_status(status);
                self.reaped = true;
                self.exit_code = Some(code);
                return Some(code);
            }
            if rc == 0 {
                return None;
            }
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if err.raw_os_error() == Some(libc::ECHILD) {
                self.reaped = true;
            }
            return self.exit_code;
        }
    }

    pub(super) fn wait_blocking(&mut self) -> Option<i32> {
        if self.reaped {
            return self.exit_code;
        }
        let mut status: libc::c_int = 0;
        loop {
            let rc = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if rc == self.pid {
                let code = decode_wait_status(status);
                self.reaped = true;
                self.exit_code = Some(code);
                return Some(code);
            }
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if err.raw_os_error() == Some(libc::ECHILD) {
                self.reaped = true;
            }
            return self.exit_code;
        }
    }
}

pub(crate) fn resolve_shell(shell_override: Option<&str>) -> String {
    if let Some(shell) = shell_override.filter(|s| !s.trim().is_empty()) {
        return shell.to_string();
    }
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.trim().is_empty() {
            return shell;
        }
    }
    "/bin/sh".to_string()
}

pub(super) fn build_pty_command(shell: &str, cwd: &Path, extra_env: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(shell);
    cmd.current_dir(cwd)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("GREIN", "1");
    for &var in SCRUBBED_TERMINAL_ENV_VARS {
        cmd.env_remove(var);
    }
    for &(key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd
}

pub(crate) unsafe fn reset_child_signals() -> io::Result<()> {
    let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    if libc::sigemptyset(set.as_mut_ptr()) < 0 {
        return Err(io::Error::last_os_error());
    }
    let set = set.assume_init();
    if libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) < 0 {
        return Err(io::Error::last_os_error());
    }
    for &sig in RESET_CHILD_SIGNALS {
        if libc::signal(sig, libc::SIG_DFL) == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open_master_fd() -> io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    let raw_fd = {
        let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
        if fd < 0 && io::Error::last_os_error().kind() == io::ErrorKind::InvalidInput {
            unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) }
        } else {
            fd
        }
    };
    #[cfg(not(target_os = "linux"))]
    let raw_fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };

    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    set_cloexec(owned.as_raw_fd())?;
    Ok(owned)
}

fn slave_pty_path(master_fd: &OwnedFd) -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let mut buf = [0 as c_char; 256];
        let rc = unsafe { libc::ptsname_r(master_fd.as_raw_fd(), buf.as_mut_ptr(), buf.len()) };
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
        let ptr = unsafe { libc::ptsname(master_fd.as_raw_fd()) };
        if ptr.is_null() {
            return Err(io::Error::last_os_error());
        }
        let path = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(PathBuf::from(path))
    }
}

pub(super) fn set_pty_winsize(fd: RawFd, rows: u16, cols: u16) -> io::Result<()> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    #[allow(clippy::unnecessary_cast)]
    let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn open_pty_pair(rows: u16, cols: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let master_fd = open_master_fd()?;
    if unsafe { libc::grantpt(master_fd.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::unlockpt(master_fd.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let slave_path = slave_pty_path(&master_fd)?;

    let slave_file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(&slave_path)?;
    let slave_fd = OwnedFd::from(slave_file);
    set_cloexec(slave_fd.as_raw_fd())?;
    set_pty_winsize(slave_fd.as_raw_fd(), rows, cols)?;

    Ok((master_fd, slave_fd))
}

pub(super) fn create_wake_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    set_cloexec(read_fd.as_raw_fd())?;
    set_cloexec(write_fd.as_raw_fd())?;
    set_nonblocking(read_fd.as_raw_fd())?;
    set_nonblocking(write_fd.as_raw_fd())?;
    Ok((read_fd, write_fd))
}

pub(super) fn send_signal_to_process_group(pid: libc::pid_t, sig: libc::c_int) {
    let pgid_rc = unsafe { libc::kill(-pid, sig) };
    if pgid_rc < 0 {
        let _ = unsafe { libc::kill(pid, sig) };
    }
}

fn decode_wait_status(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        -1
    }
}

pub(crate) fn exit_status_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        code
    } else if let Some(sig) = status.signal() {
        128 + sig
    } else {
        -1
    }
}

pub(super) fn write_all_to_fd(
    fd: RawFd,
    write_lock: &Mutex<()>,
    shutdown: &AtomicBool,
    bytes: &[u8],
) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let _guard = write_lock
        .lock()
        .map_err(|_| io::Error::other("pty write mutex poisoned"))?;
    let mut offset = 0usize;
    while offset < bytes.len() {
        if shutdown.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pty session is shutting down",
            ));
        }
        let slice = &bytes[offset..];
        let written = unsafe { libc::write(fd, slice.as_ptr().cast(), slice.len()) };
        if written > 0 {
            offset += written as usize;
            continue;
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "wrote zero bytes to pty master",
            ));
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if err.kind() == io::ErrorKind::WouldBlock {
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            let prc = unsafe { libc::poll(&mut pfd, 1, POLL_TIMEOUT_MS) };
            if prc < 0 {
                let perr = io::Error::last_os_error();
                if perr.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(perr);
            }
            if prc > 0 && (pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "pty master closed during write",
                ));
            }
            continue;
        }
        return Err(err);
    }
    Ok(())
}
