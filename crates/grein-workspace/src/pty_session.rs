use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::ffi::{c_char, CStr};
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;

const DEFAULT_SCROLLBACK_LEN: usize = 1000;
const READ_CHUNK_SIZE: usize = 4096;
const MAX_BATCH_BYTES: usize = 65536;
const POLL_TIMEOUT_MS: i32 = 100;
const DROP_SIGHUP_GRACE: Duration = Duration::from_millis(100);
const DROP_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[cfg(unix)]
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

#[cfg(unix)]
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

/// Terminal foreground or background color stored in a snapshot cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PtyColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Single rendered cell within a [`PtyScreenSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PtyCell {
    pub contents: String,
    pub fg: PtyColor,
    pub bg: PtyColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub is_wide_continuation: bool,
}

/// Immutable point-in-time snapshot of the VT100 terminal screen and process state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PtyScreenSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<Vec<PtyCell>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub hide_cursor: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub exited: bool,
    pub exit_code: Option<i32>,
}

impl PtyScreenSnapshot {
    /// Renders the screen grid as plain UTF-8 lines, skipping wide-character
    /// continuation cells and trimming trailing spaces on each row.
    pub fn plain_text(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                let mut line = String::with_capacity(row.len());
                for cell in row {
                    if cell.is_wide_continuation {
                        continue;
                    }
                    if cell.contents.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&cell.contents);
                    }
                }
                line.trim_end_matches(' ').to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct PtyState {
    parser: vt100::Parser,
    exited: bool,
    exit_code: Option<i32>,
}

#[cfg(unix)]
struct ChildProcess {
    pid: libc::pid_t,
    reaped: bool,
    exit_code: Option<i32>,
}

#[cfg(unix)]
impl ChildProcess {
    fn try_wait(&mut self) -> Option<i32> {
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

    fn wait_blocking(&mut self) -> Option<i32> {
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
        shell_override: Option<&str>,
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Self::spawn_unix(cwd, rows, cols, shell_override, on_update)
        }
        #[cfg(not(unix))]
        {
            let _ = (cwd, rows, cols, shell_override, on_update);
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
        on_update: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let shell = resolve_shell(shell_override);

        let (master_fd, slave_fd) = open_pty_pair(rows, cols)?;
        set_nonblocking(master_fd.as_raw_fd())?;
        let (wake_read_fd, wake_write_fd) = create_wake_pipe()?;

        let slave_raw_fd = slave_fd.as_raw_fd();
        let mut cmd = build_pty_command(&shell, cwd);

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
        let state = Arc::new(Mutex::new(PtyState {
            parser: vt100::Parser::new(rows, cols, DEFAULT_SCROLLBACK_LEN),
            exited: false,
            exit_code: None,
        }));
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
            guard.parser.set_size(rows, cols);
        }
        (self.on_update)();
        Ok(())
    }

    /// Captures a consistent snapshot of the current VT100 screen and exit state.
    pub fn snapshot(&self) -> PtyScreenSnapshot {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let screen = guard.parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();

        let mut cells = Vec::with_capacity(usize::from(rows));
        for r in 0..rows {
            let mut row_cells = Vec::with_capacity(usize::from(cols));
            for c in 0..cols {
                let cell = match screen.cell(r, c) {
                    Some(vt_cell) => PtyCell {
                        contents: vt_cell.contents(),
                        fg: map_vt100_color(vt_cell.fgcolor()),
                        bg: map_vt100_color(vt_cell.bgcolor()),
                        bold: vt_cell.bold(),
                        italic: vt_cell.italic(),
                        underline: vt_cell.underline(),
                        inverse: vt_cell.inverse(),
                        is_wide_continuation: vt_cell.is_wide_continuation(),
                    },
                    None => PtyCell::default(),
                };
                row_cells.push(cell);
            }
            cells.push(row_cells);
        }

        PtyScreenSnapshot {
            rows,
            cols,
            cells,
            cursor_row,
            cursor_col,
            hide_cursor: screen.hide_cursor(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            alternate_screen: screen.alternate_screen(),
            exited: guard.exited,
            exit_code: guard.exit_code,
        }
    }

    /// Returns the OS process ID of the spawned shell child process.
    pub fn child_pid(&self) -> u32 {
        self.pid
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DsrState {
    #[default]
    Idle,
    Esc,
    Csi,
    Csi5,
    Csi6,
    CsiQ,
    CsiQ6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DsrQuery {
    DeviceStatus,
    CursorPosition,
    DecCursorPosition,
}

#[derive(Default)]
struct DsrScanner {
    state: DsrState,
}

impl DsrScanner {
    fn feed_byte(&mut self, byte: u8) -> Option<DsrQuery> {
        match (self.state, byte) {
            (_, 0x1b) => {
                self.state = DsrState::Esc;
                None
            }
            (DsrState::Esc, b'[') => {
                self.state = DsrState::Csi;
                None
            }
            (DsrState::Csi, b'5') => {
                self.state = DsrState::Csi5;
                None
            }
            (DsrState::Csi, b'6') => {
                self.state = DsrState::Csi6;
                None
            }
            (DsrState::Csi, b'?') => {
                self.state = DsrState::CsiQ;
                None
            }
            (DsrState::CsiQ, b'6') => {
                self.state = DsrState::CsiQ6;
                None
            }
            (DsrState::Csi5, b'n') => {
                self.state = DsrState::Idle;
                Some(DsrQuery::DeviceStatus)
            }
            (DsrState::Csi6, b'n') => {
                self.state = DsrState::Idle;
                Some(DsrQuery::CursorPosition)
            }
            (DsrState::CsiQ6, b'n') => {
                self.state = DsrState::Idle;
                Some(DsrQuery::DecCursorPosition)
            }
            _ => {
                self.state = DsrState::Idle;
                None
            }
        }
    }
}

fn map_vt100_color(color: vt100::Color) -> PtyColor {
    match color {
        vt100::Color::Default => PtyColor::Default,
        vt100::Color::Idx(idx) => PtyColor::Indexed(idx),
        vt100::Color::Rgb(r, g, b) => PtyColor::Rgb(r, g, b),
    }
}

#[cfg(unix)]
fn resolve_shell(shell_override: Option<&str>) -> String {
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

#[cfg(unix)]
fn build_pty_command(shell: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(shell);
    cmd.current_dir(cwd)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    for &var in SCRUBBED_TERMINAL_ENV_VARS {
        cmd.env_remove(var);
    }
    cmd
}

#[cfg(unix)]
unsafe fn reset_child_signals() -> io::Result<()> {
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

#[cfg(unix)]
fn set_cloexec(fd: RawFd) -> io::Result<()> {
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
fn set_pty_winsize(fd: RawFd, rows: u16, cols: u16) -> io::Result<()> {
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

#[cfg(unix)]
fn open_pty_pair(rows: u16, cols: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let master_fd = open_master_fd()?;
    if unsafe { libc::grantpt(master_fd.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::unlockpt(master_fd.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let slave_path = slave_pty_path(&master_fd)?;
    set_pty_winsize(master_fd.as_raw_fd(), rows, cols)?;

    let slave_file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(&slave_path)?;
    let slave_fd = OwnedFd::from(slave_file);
    set_cloexec(slave_fd.as_raw_fd())?;

    Ok((master_fd, slave_fd))
}

#[cfg(unix)]
fn create_wake_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
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

#[cfg(unix)]
fn send_signal_to_process_group(pid: libc::pid_t, sig: libc::c_int) {
    let pgid_rc = unsafe { libc::kill(-pid, sig) };
    if pgid_rc < 0 {
        let _ = unsafe { libc::kill(pid, sig) };
    }
}

#[cfg(unix)]
fn decode_wait_status(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        -1
    }
}

#[cfg(unix)]
fn write_all_to_fd(
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

#[cfg(unix)]
fn spawn_reader_thread(
    master: Arc<OwnedFd>,
    wake_read: OwnedFd,
    write_lock: Arc<Mutex<()>>,
    state: Arc<Mutex<PtyState>>,
    child: Arc<Mutex<ChildProcess>>,
    shutdown: Arc<AtomicBool>,
    on_update: Arc<dyn Fn() + Send + Sync>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("grein-pty-reader".to_string())
        .spawn(move || {
            let mut dsr = DsrScanner::default();
            let mut read_buf = [0u8; READ_CHUNK_SIZE];
            let mut batch = Vec::with_capacity(READ_CHUNK_SIZE);

            loop {
                if shutdown.load(Ordering::Acquire) {
                    break;
                }

                let mut fds = [
                    libc::pollfd {
                        fd: master.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wake_read.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];

                let prc = unsafe { libc::poll(fds.as_mut_ptr(), 2, POLL_TIMEOUT_MS) };
                if prc < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }

                if (fds[1].revents
                    & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL))
                    != 0
                    || shutdown.load(Ordering::Acquire)
                {
                    break;
                }

                let mut hit_eof = false;
                if (fds[0].revents & libc::POLLIN) != 0 {
                    batch.clear();
                    while batch.len() < MAX_BATCH_BYTES {
                        let n = unsafe {
                            libc::read(
                                master.as_raw_fd(),
                                read_buf.as_mut_ptr().cast(),
                                read_buf.len(),
                            )
                        };
                        if n > 0 {
                            batch.extend_from_slice(&read_buf[..n as usize]);
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

                    if !batch.is_empty() {
                        process_pty_chunk(
                            &batch,
                            &mut dsr,
                            &state,
                            master.as_raw_fd(),
                            &write_lock,
                            &shutdown,
                            &on_update,
                        );
                    }
                }

                if (fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0
                    && (fds[0].revents & libc::POLLIN) == 0
                {
                    hit_eof = true;
                }

                if hit_eof {
                    reap_child_on_eof(&child, &state, &shutdown, &on_update);
                    break;
                }

                if prc == 0 {
                    let maybe_exit = {
                        let mut child_guard = child.lock().unwrap_or_else(PoisonError::into_inner);
                        child_guard.try_wait().map(|code| (true, Some(code)))
                    };
                    if let Some((exited, exit_code)) = maybe_exit {
                        mark_exited(&state, exited, exit_code, &on_update);
                    }
                }
            }
        })
}

#[cfg(unix)]
fn process_pty_chunk(
    data: &[u8],
    dsr: &mut DsrScanner,
    state: &Mutex<PtyState>,
    master_fd: RawFd,
    write_lock: &Mutex<()>,
    shutdown: &AtomicBool,
    on_update: &Arc<dyn Fn() + Send + Sync>,
) {
    let mut responses: Vec<Vec<u8>> = Vec::new();
    {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut start = 0usize;
        for (idx, &byte) in data.iter().enumerate() {
            if let Some(query) = dsr.feed_byte(byte) {
                guard.parser.process(&data[start..=idx]);
                start = idx + 1;
                let (row, col) = guard.parser.screen().cursor_position();
                let reply = match query {
                    DsrQuery::DeviceStatus => b"\x1b[0n".to_vec(),
                    DsrQuery::CursorPosition => {
                        format!("\x1b[{};{}R", row + 1, col + 1).into_bytes()
                    }
                    DsrQuery::DecCursorPosition => {
                        format!("\x1b[?{};{}R", row + 1, col + 1).into_bytes()
                    }
                };
                responses.push(reply);
            }
        }
        if start < data.len() {
            guard.parser.process(&data[start..]);
        }
    }

    for reply in responses {
        let _ = write_all_to_fd(master_fd, write_lock, shutdown, &reply);
    }
    on_update();
}

#[cfg(unix)]
fn reap_child_on_eof(
    child: &Mutex<ChildProcess>,
    state: &Mutex<PtyState>,
    shutdown: &AtomicBool,
    on_update: &Arc<dyn Fn() + Send + Sync>,
) {
    let mut exit_code = None;
    for _ in 0..50 {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        let (reaped, code) = {
            let mut guard = child.lock().unwrap_or_else(PoisonError::into_inner);
            let code = guard.try_wait();
            (guard.reaped, code)
        };
        if reaped {
            exit_code = code;
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }

    if !shutdown.load(Ordering::Acquire) {
        mark_exited(state, true, exit_code, on_update);
    }
}

#[cfg(unix)]
fn mark_exited(
    state: &Mutex<PtyState>,
    exited: bool,
    exit_code: Option<i32>,
    on_update: &Arc<dyn Fn() + Send + Sync>,
) {
    let changed = {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        let prev_exited = guard.exited;
        let prev_code = guard.exit_code;
        guard.exited = exited;
        if exit_code.is_some() {
            guard.exit_code = exit_code;
        }
        guard.exited != prev_exited || guard.exit_code != prev_code
    };
    if changed {
        on_update();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dsr_scanner_handles_split_and_back_to_back_queries() {
        let mut scanner = DsrScanner::default();
        let stream = b"abc\x1b[6n\x1b[5n\x1b[38;2;1;2;3m\x1b[?6n";
        let mut detected = Vec::new();
        for &byte in stream {
            if let Some(query) = scanner.feed_byte(byte) {
                detected.push(query);
            }
        }
        assert_eq!(
            detected,
            vec![
                DsrQuery::CursorPosition,
                DsrQuery::DeviceStatus,
                DsrQuery::DecCursorPosition,
            ]
        );
    }

    #[test]
    fn test_dsr_scanner_recovers_from_interrupted_escape_sequence() {
        let mut scanner = DsrScanner::default();
        let stream = b"\x1b[6\x1b[5n";
        let mut detected = Vec::new();
        for &byte in stream {
            if let Some(query) = scanner.feed_byte(byte) {
                detected.push(query);
            }
        }
        assert_eq!(detected, vec![DsrQuery::DeviceStatus]);
    }

    #[test]
    fn test_map_vt100_color_variants() {
        assert_eq!(map_vt100_color(vt100::Color::Default), PtyColor::Default);
        assert_eq!(
            map_vt100_color(vt100::Color::Idx(196)),
            PtyColor::Indexed(196)
        );
        assert_eq!(
            map_vt100_color(vt100::Color::Rgb(10, 20, 30)),
            PtyColor::Rgb(10, 20, 30)
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_shell_prefers_non_empty_override() {
        assert_eq!(resolve_shell(Some("/bin/sh")), "/bin/sh");
        assert!(!resolve_shell(Some("   ")).is_empty());
        assert!(!resolve_shell(None).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_build_pty_command_scrubs_outer_terminal_env_and_sets_term() {
        use std::collections::HashMap;
        use std::ffi::OsStr;

        let cmd = build_pty_command("/bin/sh", Path::new("/tmp"));
        let envs: HashMap<&OsStr, Option<&OsStr>> = cmd.get_envs().collect();

        assert_eq!(
            envs.get(OsStr::new("TERM")),
            Some(&Some(OsStr::new("xterm-256color")))
        );
        assert_eq!(
            envs.get(OsStr::new("COLORTERM")),
            Some(&Some(OsStr::new("truecolor")))
        );
        for &var in SCRUBBED_TERMINAL_ENV_VARS {
            assert_eq!(
                envs.get(OsStr::new(var)),
                Some(&None),
                "expected {var} to be marked removed via env_remove"
            );
        }
        assert!(!envs.contains_key(OsStr::new("PATH")));
        assert!(!envs.contains_key(OsStr::new("HOME")));
        assert!(!envs.contains_key(OsStr::new("STARSHIP_CONFIG")));
    }
}
