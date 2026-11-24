#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(unix)]
use std::thread::{self, JoinHandle};
#[cfg(unix)]
use std::time::Duration;

use super::scanner::{ScannerEvent, TerminalScanner};
use super::state::{capture_screen_row, PtyState};
#[cfg(unix)]
use super::unix::{write_all_to_fd, ChildProcess, POLL_TIMEOUT_MS};

#[cfg(unix)]
const READ_CHUNK_SIZE: usize = 4096;
#[cfg(unix)]
const MAX_BATCH_BYTES: usize = 65536;
pub(super) const DA1_REPLY: &[u8] = b"\x1b[?62;c";
pub(super) const DA2_REPLY: &[u8] = b"\x1b[>0;10;1c";
pub(super) const OSC10_FG_REPLY: &[u8] = b"\x1b]10;rgb:e5e5/e5e5/e5e5\x1b\\";
pub(super) const OSC11_BG_REPLY: &[u8] = b"\x1b]11;rgb:0000/0000/0000\x1b\\";

#[cfg(unix)]
pub(super) fn spawn_reader_thread(
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

fn apply_scanner_event(event: ScannerEvent, guard: &mut PtyState, responses: &mut Vec<Vec<u8>>) {
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

pub(super) fn process_pty_bytes(
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
            Some((
                capture_screen_row(guard.parser.screen(), 0, cols),
                guard.parser.screen().row_wrapped(0),
            ))
        } else {
            None
        };

        let event = scanner.feed_byte(byte);
        let candidate_su_rows = match event {
            Some(ScannerEvent::ScrollUp(count)) if can_scroll => {
                let actual = count.min(rows);
                Some(
                    (0..actual)
                        .map(|r| {
                            (
                                capture_screen_row(guard.parser.screen(), r, cols),
                                guard.parser.screen().row_wrapped(r),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            }
            _ => None,
        };

        guard.parser.process(std::slice::from_ref(&byte));
        if guard.parser.screen().alternate_screen() {
            guard.scrollback_offset = 0;
        }

        if let Some((row0, row0_wrapped)) = candidate_row0 {
            let scrolled = candidate_lf_scroll
                || did_wrap_scroll(guard.parser.screen(), rows, cols, prev_pos.1);
            if scrolled {
                guard.push_scrollback_row(row0, row0_wrapped);
            }
        }
        if let Some(su_rows) = candidate_su_rows {
            for (row, wrapped) in su_rows {
                guard.push_scrollback_row(row, wrapped);
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
