use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
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
use std::process::Command;
#[cfg(target_os = "macos")]
use std::process::Stdio;

const DEFAULT_SCROLLBACK_LEN: usize = 1000;
const READ_CHUNK_SIZE: usize = 4096;
const MAX_BATCH_BYTES: usize = 65536;
const POLL_TIMEOUT_MS: i32 = 100;
const DROP_SIGHUP_GRACE: Duration = Duration::from_millis(100);
const DROP_POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_OSC_PAYLOAD_BYTES: usize = 8192;
const MAX_CSI_PARAMS: usize = 16;
const DA1_REPLY: &[u8] = b"\x1b[?62;c";
const DA2_REPLY: &[u8] = b"\x1b[>0;10;1c";
const OSC10_FG_REPLY: &[u8] = b"\x1b]10;rgb:e5e5/e5e5/e5e5\x1b\\";
const OSC11_BG_REPLY: &[u8] = b"\x1b]11;rgb:0000/0000/0000\x1b\\";

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
    #[serde(default)]
    pub scrollback_offset: usize,
    #[serde(default)]
    pub cwd: Option<String>,
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
    scrollback: VecDeque<Vec<PtyCell>>,
    scrollback_offset: usize,
    scroll_top: u16,
    scroll_bottom: u16,
    current_cwd: Option<String>,
    last_os_cwd: Option<String>,
    exited: bool,
    exit_code: Option<i32>,
}

impl PtyState {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            scrollback: VecDeque::new(),
            scrollback_offset: 0,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            current_cwd: None,
            last_os_cwd: None,
            exited: false,
            exit_code: None,
        }
    }

    fn scroll_region_active(&self) -> bool {
        let (rows, _) = self.parser.screen().size();
        self.scroll_top != 0 || self.scroll_bottom != rows.saturating_sub(1)
    }

    fn push_scrollback_row(&mut self, row: Vec<PtyCell>) {
        if self.scrollback.len() >= DEFAULT_SCROLLBACK_LEN {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(row);
        if self.scrollback_offset > 0 {
            self.scrollback_offset = self.scrollback.len().min(self.scrollback_offset + 1);
        }
    }

    fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let (rows, _) = self.parser.screen().size();
        let max_row = rows.saturating_sub(1);
        let top = top.saturating_sub(1);
        let bottom = if bottom == 0 {
            max_row
        } else {
            (bottom - 1).min(max_row)
        };
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = max_row;
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let (old_rows, _) = self.parser.screen().size();
        if self.scroll_bottom == old_rows.saturating_sub(1) {
            self.scroll_bottom = rows.saturating_sub(1);
        }
        if self.scroll_bottom >= rows {
            self.scroll_bottom = rows.saturating_sub(1);
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
        }
        self.parser.set_size(rows, cols);
        self.clamp_scrollback_offset(self.scrollback_offset);
    }

    fn reset(&mut self) {
        let (rows, _) = self.parser.screen().size();
        self.scrollback.clear();
        self.scrollback_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
    }

    fn clamp_scrollback_offset(&mut self, target: usize) -> usize {
        if self.parser.screen().alternate_screen() {
            self.scrollback_offset = 0;
        } else {
            self.scrollback_offset = target.min(self.scrollback.len());
        }
        self.scrollback_offset
    }
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
                guard
                    .scrollback_offset
                    .saturating_sub(delta.unsigned_abs())
            };
            guard.clamp_scrollback_offset(next)
        };
        (self.on_update)();
        resolved
    }

    /// Captures a consistent snapshot of the current VT100 screen and exit state.
    pub fn snapshot(&self) -> PtyScreenSnapshot {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let screen = guard.parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let alternate_screen = screen.alternate_screen();
        let scrollback_offset = if alternate_screen {
            0
        } else {
            guard.scrollback_offset.min(guard.scrollback.len())
        };

        let row_count = usize::from(rows);
        let from_scrollback = scrollback_offset.min(row_count);
        let sb_start = guard.scrollback.len().saturating_sub(scrollback_offset);

        let mut cells = Vec::with_capacity(row_count);
        for idx in 0..from_scrollback {
            if let Some(sb_row) = guard.scrollback.get(sb_start + idx) {
                cells.push(fit_row_to_cols(sb_row, cols));
            }
        }
        let live_rows = row_count.saturating_sub(from_scrollback);
        for r in 0..rows {
            if usize::from(r) >= live_rows {
                break;
            }
            cells.push(capture_screen_row(screen, r, cols));
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
            alternate_screen,
            exited: guard.exited,
            exit_code: guard.exit_code,
            scrollback_offset,
            cwd: guard.current_cwd.clone(),
        }
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

fn capture_screen_row(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<PtyCell> {
    let mut row_cells = Vec::with_capacity(usize::from(cols));
    for c in 0..cols {
        let cell = match screen.cell(row, c) {
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
    row_cells
}

fn fit_row_to_cols(row: &[PtyCell], cols: u16) -> Vec<PtyCell> {
    let target_len = usize::from(cols);
    if row.len() == target_len {
        return row.to_vec();
    }
    let mut fitted = Vec::with_capacity(target_len);
    if row.len() < target_len {
        fitted.extend_from_slice(row);
        fitted.resize_with(target_len, PtyCell::default);
    } else {
        fitted.extend_from_slice(&row[..target_len]);
        if target_len > 0
            && row
                .get(target_len)
                .is_some_and(|next| next.is_wide_continuation)
        {
            fitted[target_len - 1].contents.clear();
        }
    }
    fitted
}

#[cfg(any(unix, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ScanState {
    #[default]
    Ground,
    Utf8 {
        remaining: u8,
    },
    Esc,
    EscIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    OscEsc,
    DcsString,
    DcsEsc,
}

#[cfg(any(unix, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ScannerEvent {
    DeviceStatus,
    CursorPosition,
    DecCursorPosition,
    PrimaryDeviceAttributes,
    SecondaryDeviceAttributes,
    OscForegroundColorQuery,
    OscBackgroundColorQuery,
    OscCwd(String),
    ScrollRegion { top: u16, bottom: u16 },
    ScrollUp(u16),
    Reset,
}

#[cfg(any(unix, test))]
#[derive(Default)]
struct TerminalScanner {
    state: ScanState,
    csi_prefix: Option<u8>,
    csi_params: [u16; MAX_CSI_PARAMS],
    csi_param_len: usize,
    csi_current_param: Option<u16>,
    csi_has_intermediate: bool,
    csi_ignore: bool,
    osc_buf: Vec<u8>,
    osc_overflow: bool,
}

#[cfg(any(unix, test))]
impl TerminalScanner {
    fn executes_c0(&self) -> bool {
        matches!(
            self.state,
            ScanState::Ground
                | ScanState::Esc
                | ScanState::EscIntermediate
                | ScanState::CsiEntry
                | ScanState::CsiParam
                | ScanState::CsiIntermediate
                | ScanState::CsiIgnore
                | ScanState::OscEsc
                | ScanState::DcsEsc
        )
    }

    fn will_print_byte(&self, byte: u8) -> bool {
        match self.state {
            ScanState::Ground => (0x20..=0x7e).contains(&byte),
            ScanState::Utf8 { remaining } => remaining == 1 || !(0x80..=0xbf).contains(&byte),
            _ => false,
        }
    }

    fn reset_csi(&mut self) {
        self.csi_prefix = None;
        self.csi_param_len = 0;
        self.csi_current_param = None;
        self.csi_has_intermediate = false;
        self.csi_ignore = false;
    }

    fn reset_osc(&mut self) {
        self.osc_buf.clear();
        self.osc_overflow = false;
    }

    fn push_csi_param(&mut self, value: u16) {
        if self.csi_param_len < MAX_CSI_PARAMS {
            self.csi_params[self.csi_param_len] = value;
            self.csi_param_len += 1;
        } else {
            self.csi_ignore = true;
        }
    }

    fn finish_csi(&mut self, final_byte: u8) -> Option<ScannerEvent> {
        if self.csi_current_param.is_some() || self.csi_param_len > 0 {
            let val = self.csi_current_param.take().unwrap_or(0);
            self.push_csi_param(val);
        }
        self.state = ScanState::Ground;
        if self.csi_ignore || self.csi_has_intermediate {
            return None;
        }
        let params = &self.csi_params[..self.csi_param_len];
        match (self.csi_prefix, final_byte, params) {
            (None, b'n', &[5]) => Some(ScannerEvent::DeviceStatus),
            (None, b'n', &[6]) => Some(ScannerEvent::CursorPosition),
            (Some(b'?'), b'n', &[6]) => Some(ScannerEvent::DecCursorPosition),
            (None, b'c', &[] | &[0]) => Some(ScannerEvent::PrimaryDeviceAttributes),
            (Some(b'>'), b'c', &[] | &[0]) => Some(ScannerEvent::SecondaryDeviceAttributes),
            (None, b'r', _) => Some(ScannerEvent::ScrollRegion {
                top: params.first().copied().unwrap_or(0),
                bottom: params.get(1).copied().unwrap_or(0),
            }),
            (None, b'S', _) => Some(ScannerEvent::ScrollUp(
                params.first().copied().unwrap_or(0).max(1),
            )),
            _ => None,
        }
    }

    fn finish_osc(&mut self) -> Option<ScannerEvent> {
        let event = if self.osc_overflow {
            None
        } else {
            parse_osc_payload(&self.osc_buf)
        };
        self.reset_osc();
        self.state = ScanState::Ground;
        event
    }

    fn handle_esc_byte(&mut self, byte: u8) -> Option<ScannerEvent> {
        match byte {
            0x1b => {
                self.state = ScanState::Esc;
                None
            }
            0x18 | 0x1a => {
                self.state = ScanState::Ground;
                None
            }
            0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => None,
            b'[' => {
                self.reset_csi();
                self.state = ScanState::CsiEntry;
                None
            }
            b']' => {
                self.reset_osc();
                self.state = ScanState::OscString;
                None
            }
            b'P' | b'X' | b'^' | b'_' => {
                self.state = ScanState::DcsString;
                None
            }
            0x20..=0x2f => {
                self.state = ScanState::EscIntermediate;
                None
            }
            b'c' => {
                self.state = ScanState::Ground;
                Some(ScannerEvent::Reset)
            }
            _ => {
                self.state = ScanState::Ground;
                None
            }
        }
    }

    fn feed_byte(&mut self, byte: u8) -> Option<ScannerEvent> {
        if let ScanState::Utf8 { remaining } = self.state {
            if (0x80..=0xbf).contains(&byte) && remaining > 1 {
                self.state = ScanState::Utf8 {
                    remaining: remaining - 1,
                };
            } else {
                self.state = ScanState::Ground;
            }
            return None;
        }

        match self.state {
            ScanState::Ground => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0xc2..=0xdf => {
                    self.state = ScanState::Utf8 { remaining: 1 };
                    None
                }
                0xe0..=0xef => {
                    self.state = ScanState::Utf8 { remaining: 2 };
                    None
                }
                0xf0..=0xf4 => {
                    self.state = ScanState::Utf8 { remaining: 3 };
                    None
                }
                _ => None,
            },
            ScanState::Utf8 { .. } => unreachable!(),
            ScanState::Esc => self.handle_esc_byte(byte),
            ScanState::EscIntermediate => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x20..=0x2f | 0x7f => None,
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::CsiEntry | ScanState::CsiParam => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => None,
                b'0'..=b'9' => {
                    let digit = u16::from(byte - b'0');
                    let next = self
                        .csi_current_param
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit);
                    self.csi_current_param = Some(next);
                    self.state = ScanState::CsiParam;
                    None
                }
                b';' => {
                    let val = self.csi_current_param.take().unwrap_or(0);
                    self.push_csi_param(val);
                    self.state = ScanState::CsiParam;
                    None
                }
                b':' => {
                    self.csi_ignore = true;
                    self.state = ScanState::CsiParam;
                    None
                }
                0x3c..=0x3f => {
                    if self.state == ScanState::CsiEntry {
                        self.csi_prefix = Some(byte);
                        self.state = ScanState::CsiParam;
                    } else {
                        self.csi_ignore = true;
                        self.state = ScanState::CsiIgnore;
                    }
                    None
                }
                0x20..=0x2f => {
                    self.csi_has_intermediate = true;
                    self.state = ScanState::CsiIntermediate;
                    None
                }
                0x40..=0x7e => self.finish_csi(byte),
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::CsiIntermediate => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x20..=0x2f | 0x7f => None,
                0x30..=0x3f => {
                    self.state = ScanState::CsiIgnore;
                    None
                }
                0x40..=0x7e => {
                    self.state = ScanState::Ground;
                    None
                }
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::CsiIgnore => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a | 0x40..=0x7e => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x3f | 0x7f => None,
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::OscString => match byte {
                0x07 => self.finish_osc(),
                0x1b => {
                    self.state = ScanState::OscEsc;
                    None
                }
                0x18 | 0x1a => {
                    self.reset_osc();
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f => None,
                _ => {
                    if !self.osc_overflow {
                        if self.osc_buf.len() < MAX_OSC_PAYLOAD_BYTES {
                            self.osc_buf.push(byte);
                        } else {
                            self.osc_overflow = true;
                        }
                    }
                    None
                }
            },
            ScanState::OscEsc => {
                if byte == b'\\' {
                    self.finish_osc()
                } else {
                    self.reset_osc();
                    self.handle_esc_byte(byte)
                }
            }
            ScanState::DcsString => match byte {
                0x1b => {
                    self.state = ScanState::DcsEsc;
                    None
                }
                0x18 | 0x1a | 0x9c => {
                    self.state = ScanState::Ground;
                    None
                }
                _ => None,
            },
            ScanState::DcsEsc => {
                if byte == b'\\' {
                    self.state = ScanState::Ground;
                    None
                } else {
                    self.handle_esc_byte(byte)
                }
            }
        }
    }
}

#[cfg(any(unix, test))]
fn parse_osc_payload(payload: &[u8]) -> Option<ScannerEvent> {
    let sep = payload.iter().position(|&b| b == b';')?;
    let code = &payload[..sep];
    let value = &payload[sep + 1..];
    match code {
        b"10" if value == b"?" => Some(ScannerEvent::OscForegroundColorQuery),
        b"11" if value == b"?" => Some(ScannerEvent::OscBackgroundColorQuery),
        b"7" => parse_osc7_cwd(value).map(ScannerEvent::OscCwd),
        _ => None,
    }
}

#[cfg(any(unix, test))]
fn parse_osc7_cwd(value: &[u8]) -> Option<String> {
    const FILE_SCHEME: &[u8] = b"file://";
    if value.len() < FILE_SCHEME.len()
        || !value[..FILE_SCHEME.len()].eq_ignore_ascii_case(FILE_SCHEME)
    {
        return None;
    }
    let after_scheme = &value[FILE_SCHEME.len()..];
    let slash_pos = after_scheme.iter().position(|&b| b == b'/')?;
    percent_decode_utf8(&after_scheme[slash_pos..])
}

#[cfg(any(unix, test))]
fn percent_decode_utf8(bytes: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    if out.contains(&0) {
        return None;
    }
    String::from_utf8(out).ok()
}

#[cfg(any(unix, test))]
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
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
fn build_pty_command(shell: &str, cwd: &Path, extra_env: &[(&str, &str)]) -> Command {
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
            let mut scanner = TerminalScanner::default();
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
                            &mut scanner,
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

#[cfg(any(unix, test))]
fn did_wrap_scroll(screen: &vt100::Screen, rows: u16, cols: u16, prev_col: u16) -> bool {
    let (new_row, new_col) = screen.cursor_position();
    if new_row != rows - 1 {
        return false;
    }
    if prev_col >= cols {
        new_col < prev_col || (cols == 1 && new_col == 1)
    } else {
        new_col == 2
            && screen
                .cell(rows - 1, 1)
                .is_some_and(|c| c.is_wide_continuation())
            && (cols > 3 || new_col < prev_col)
    }
}

#[cfg(any(unix, test))]
fn apply_scanner_event(
    event: ScannerEvent,
    guard: &mut PtyState,
    responses: &mut Vec<Vec<u8>>,
) {
    match event {
        ScannerEvent::DeviceStatus => {
            responses.push(b"\x1b[0n".to_vec());
        }
        ScannerEvent::CursorPosition => {
            let (row, col) = guard.parser.screen().cursor_position();
            responses.push(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
        }
        ScannerEvent::DecCursorPosition => {
            let (row, col) = guard.parser.screen().cursor_position();
            responses.push(format!("\x1b[?{};{}R", row + 1, col + 1).into_bytes());
        }
        ScannerEvent::PrimaryDeviceAttributes => {
            responses.push(DA1_REPLY.to_vec());
        }
        ScannerEvent::SecondaryDeviceAttributes => {
            responses.push(DA2_REPLY.to_vec());
        }
        ScannerEvent::OscForegroundColorQuery => {
            responses.push(OSC10_FG_REPLY.to_vec());
        }
        ScannerEvent::OscBackgroundColorQuery => {
            responses.push(OSC11_BG_REPLY.to_vec());
        }
        ScannerEvent::OscCwd(cwd) => {
            guard.current_cwd = Some(cwd);
        }
        ScannerEvent::ScrollRegion { top, bottom } => {
            guard.set_scroll_region(top, bottom);
        }
        ScannerEvent::ScrollUp(_) => {}
        ScannerEvent::Reset => {
            guard.reset();
        }
    }
}

#[cfg(any(unix, test))]
fn process_pty_bytes(
    data: &[u8],
    scanner: &mut TerminalScanner,
    guard: &mut PtyState,
    responses: &mut Vec<Vec<u8>>,
) {
    for &byte in data {
        let (rows, cols) = guard.parser.screen().size();
        let alt = guard.parser.screen().alternate_screen();
        let can_scroll = !alt && !guard.scroll_region_active() && rows > 0 && cols > 0;
        let prev_pos = guard.parser.screen().cursor_position();

        let candidate_lf_scroll = can_scroll
            && scanner.executes_c0()
            && matches!(byte, 0x0a..=0x0c)
            && prev_pos.0 == rows - 1;
        let candidate_wrap_scroll = can_scroll
            && scanner.will_print_byte(byte)
            && prev_pos.0 == rows - 1
            && prev_pos.1 >= cols.saturating_sub(2);
        let candidate_row0 = if candidate_lf_scroll || candidate_wrap_scroll {
            Some(capture_screen_row(guard.parser.screen(), 0, cols))
        } else {
            None
        };

        let event = scanner.feed_byte(byte);
        let candidate_su_rows = match event {
            Some(ScannerEvent::ScrollUp(count)) if can_scroll => {
                let actual = count.min(rows);
                Some(
                    (0..actual)
                        .map(|r| capture_screen_row(guard.parser.screen(), r, cols))
                        .collect::<Vec<_>>(),
                )
            }
            _ => None,
        };

        guard.parser.process(std::slice::from_ref(&byte));
        if guard.parser.screen().alternate_screen() {
            guard.scrollback_offset = 0;
        }

        if let Some(row0) = candidate_row0 {
            let scrolled = candidate_lf_scroll
                || did_wrap_scroll(guard.parser.screen(), rows, cols, prev_pos.1);
            if scrolled {
                guard.push_scrollback_row(row0);
            }
        }
        if let Some(su_rows) = candidate_su_rows {
            for row in su_rows {
                guard.push_scrollback_row(row);
            }
        }
        if let Some(ev) = event {
            apply_scanner_event(ev, guard, responses);
        }
    }
}

#[cfg(unix)]
fn process_pty_chunk(
    data: &[u8],
    scanner: &mut TerminalScanner,
    state: &Mutex<PtyState>,
    master_fd: RawFd,
    write_lock: &Mutex<()>,
    shutdown: &AtomicBool,
    on_update: &Arc<dyn Fn() + Send + Sync>,
) {
    let mut responses: Vec<Vec<u8>> = Vec::new();
    {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        process_pty_bytes(data, scanner, &mut guard, &mut responses);
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
    fn test_scanner_handles_split_and_back_to_back_queries() {
        let mut scanner = TerminalScanner::default();
        let stream = b"abc\x1b[6n\x1b[5n\x1b[38;2;1;2;3m\x1b[?6n\x1b[c\x1b[0c\x1b[>c\x1b[>0c\x1b]10;?\x07\x1b]11;?\x1b\\";
        let mut detected = Vec::new();
        for &byte in stream {
            if let Some(event) = scanner.feed_byte(byte) {
                detected.push(event);
            }
        }
        assert_eq!(
            detected,
            vec![
                ScannerEvent::CursorPosition,
                ScannerEvent::DeviceStatus,
                ScannerEvent::DecCursorPosition,
                ScannerEvent::PrimaryDeviceAttributes,
                ScannerEvent::PrimaryDeviceAttributes,
                ScannerEvent::SecondaryDeviceAttributes,
                ScannerEvent::SecondaryDeviceAttributes,
                ScannerEvent::OscForegroundColorQuery,
                ScannerEvent::OscBackgroundColorQuery,
            ]
        );
    }

    #[test]
    fn test_scanner_ignores_non_zero_da_and_malformed_sequences() {
        let mut scanner = TerminalScanner::default();
        let stream = b"\x1b[1c\x1b[>1c\x1b[?c\x1b[?5n\x1b]10;rgb:11/22/33\x07\x1b]7;http://localhost/tmp\x07\x1b]7;file://localhost\x07\x1b[5n";
        let mut detected = Vec::new();
        for &byte in stream {
            if let Some(event) = scanner.feed_byte(byte) {
                detected.push(event);
            }
        }
        assert_eq!(detected, vec![ScannerEvent::DeviceStatus]);
    }

    #[test]
    fn test_scanner_recovers_from_interrupted_escape_and_osc_sequences() {
        let mut scanner = TerminalScanner::default();
        let stream = b"\x1b[6\x1b[5n\x1b]7;file:///unterminated\x1b[>0c\x1b]7;file:///tmp/next\x1b\\";
        let mut detected = Vec::new();
        for &byte in stream {
            if let Some(event) = scanner.feed_byte(byte) {
                detected.push(event);
            }
        }
        assert_eq!(
            detected,
            vec![
                ScannerEvent::DeviceStatus,
                ScannerEvent::SecondaryDeviceAttributes,
                ScannerEvent::OscCwd("/tmp/next".to_string()),
            ]
        );
    }

    #[test]
    fn test_scanner_bounds_oversized_osc_payload_and_recovers() {
        let mut scanner = TerminalScanner::default();
        let mut stream = b"\x1b]7;file:///".to_vec();
        stream.resize(stream.len() + MAX_OSC_PAYLOAD_BYTES + 256, b'a');
        stream.extend_from_slice(b"\x07\x1b]7;file:///tmp/recovered\x07\x1b[c");

        let mut detected = Vec::new();
        for &byte in &stream {
            if let Some(event) = scanner.feed_byte(byte) {
                detected.push(event);
            }
        }
        assert_eq!(scanner.osc_buf.len(), 0);
        assert!(!scanner.osc_overflow);
        assert_eq!(
            detected,
            vec![
                ScannerEvent::OscCwd("/tmp/recovered".to_string()),
                ScannerEvent::PrimaryDeviceAttributes,
            ]
        );
    }

    #[test]
    fn test_parse_osc7_cwd_variants() {
        assert_eq!(
            parse_osc7_cwd(b"file://localhost/tmp/my%20dir"),
            Some("/tmp/my dir".to_string())
        );
        assert_eq!(
            parse_osc7_cwd(b"file:///tmp/plain%2Bpath"),
            Some("/tmp/plain+path".to_string())
        );
        assert_eq!(
            parse_osc7_cwd(b"file://devbox.local/tmp/plus+kept/caf%C3%A9"),
            Some("/tmp/plus+kept/café".to_string())
        );
        assert_eq!(parse_osc7_cwd(b"file:///tmp/bad%FFutf8"), None);
        assert_eq!(parse_osc7_cwd(b"file:///tmp/nul%00byte"), None);
        assert_eq!(parse_osc7_cwd(b"file://localhost"), None);
        assert_eq!(parse_osc7_cwd(b"https://localhost/tmp"), None);
    }

    #[test]
    fn test_pty_snapshot_serde_defaults_new_fields() {
        let json = r#"{
            "rows": 24,
            "cols": 80,
            "cells": [],
            "cursor_row": 0,
            "cursor_col": 0,
            "hide_cursor": false,
            "application_cursor": false,
            "bracketed_paste": false,
            "alternate_screen": false,
            "exited": false,
            "exit_code": null
        }"#;
        let snap: PtyScreenSnapshot = serde_json::from_str(json).expect("deserialize legacy snapshot");
        assert_eq!(snap.scrollback_offset, 0);
        assert_eq!(snap.cwd, None);
    }

    #[test]
    fn test_fit_row_to_cols_handles_padding_and_split_wide_char() {
        let row = vec![
            PtyCell {
                contents: "📦".to_string(),
                ..PtyCell::default()
            },
            PtyCell {
                is_wide_continuation: true,
                ..PtyCell::default()
            },
            PtyCell {
                contents: "A".to_string(),
                ..PtyCell::default()
            },
        ];

        let wider = fit_row_to_cols(&row, 5);
        assert_eq!(wider.len(), 5);
        assert_eq!(wider[0].contents, "📦");
        assert_eq!(wider[2].contents, "A");
        assert_eq!(wider[4], PtyCell::default());

        let split_wide = fit_row_to_cols(&row, 1);
        assert_eq!(split_wide.len(), 1);
        assert_eq!(split_wide[0].contents, "");
    }

    #[test]
    fn test_process_pty_bytes_split_chunks_and_scrollback_edge_cases() {
        let mut scanner = TerminalScanner::default();
        let mut state = PtyState::new(4, 10);
        let mut responses = Vec::new();

        // Split probe sequences across single-byte boundaries
        let probe_stream =
            b"\x1b[2;3H\x1b[6n\x1b[?6n\x1b[c\x1b[>0c\x1b]10;?\x07\x1b]11;?\x1b\\\x1b]7;file:///tmp/a%20b\x1b\\";
        for chunk in probe_stream.chunks(2) {
            process_pty_bytes(chunk, &mut scanner, &mut state, &mut responses);
        }
        assert_eq!(
            responses,
            vec![
                b"\x1b[2;3R".to_vec(),
                b"\x1b[?2;3R".to_vec(),
                DA1_REPLY.to_vec(),
                DA2_REPLY.to_vec(),
                OSC10_FG_REPLY.to_vec(),
                OSC11_BG_REPLY.to_vec(),
            ]
        );
        assert_eq!(state.current_cwd.as_deref(), Some("/tmp/a b"));

        // Wrap-scroll + LF-scroll + CSI S scrollback capture
        process_pty_bytes(b"\x1b[1;1H", &mut scanner, &mut state, &mut responses);
        for i in 1..=8 {
            let line = format!("L{i:02}\n");
            process_pty_bytes(line.as_bytes(), &mut scanner, &mut state, &mut responses);
        }
        assert!(state.scrollback.len() >= 5);

        // Pin viewport while new line scrolls in
        assert_eq!(state.clamp_scrollback_offset(3), 3);
        let pinned_first = state.scrollback[state.scrollback.len() - 3][0].contents.clone();
        process_pty_bytes(b"NEW\n", &mut scanner, &mut state, &mut responses);
        assert_eq!(state.scrollback_offset, 4);
        assert_eq!(
            state.scrollback[state.scrollback.len() - state.scrollback_offset][0].contents,
            pinned_first
        );

        // Scroll region active (DECSTBM) does not push into scrollback
        let before_region = state.scrollback.len();
        process_pty_bytes(
            b"\x1b[2;3r\x1b[3;1HA\nB\nC\n\x1b[r",
            &mut scanner,
            &mut state,
            &mut responses,
        );
        assert_eq!(state.scrollback.len(), before_region);

        // Alternate screen forces offset to 0 without erasing saved scrollback
        process_pty_bytes(b"\x1b[?1049hALT\n\n\n\n\n", &mut scanner, &mut state, &mut responses);
        assert_eq!(state.scrollback_offset, 0);
        assert_eq!(state.clamp_scrollback_offset(5), 0);
        assert_eq!(state.scrollback.len(), before_region);
        process_pty_bytes(b"\x1b[?1049l", &mut scanner, &mut state, &mut responses);
        assert_eq!(state.clamp_scrollback_offset(5), 5);

        // Line wrap + CSI S scroll into scrollback cleanly
        let before_wrap_and_su = state.scrollback.len();
        process_pty_bytes(b"\x1b[4;1H1234567890WRAP", &mut scanner, &mut state, &mut responses);
        assert_eq!(state.scrollback.len(), before_wrap_and_su + 1);
        process_pty_bytes(b"\x1b[2S", &mut scanner, &mut state, &mut responses);
        assert_eq!(state.scrollback.len(), before_wrap_and_su + 3);

        // Full reset clears scrollback
        process_pty_bytes(b"\x1bc", &mut scanner, &mut state, &mut responses);
        assert!(state.scrollback.is_empty());
        assert_eq!(state.scrollback_offset, 0);
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

        let cmd = build_pty_command("/bin/sh", Path::new("/tmp"), &[("GREIN_THREAD_ID", "t1")]);
        let envs: HashMap<&OsStr, Option<&OsStr>> = cmd.get_envs().collect();

        assert_eq!(
            envs.get(OsStr::new("TERM")),
            Some(&Some(OsStr::new("xterm-256color")))
        );
        assert_eq!(
            envs.get(OsStr::new("COLORTERM")),
            Some(&Some(OsStr::new("truecolor")))
        );
        assert_eq!(envs.get(OsStr::new("GREIN")), Some(&Some(OsStr::new("1"))));
        assert_eq!(
            envs.get(OsStr::new("GREIN_THREAD_ID")),
            Some(&Some(OsStr::new("t1")))
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
