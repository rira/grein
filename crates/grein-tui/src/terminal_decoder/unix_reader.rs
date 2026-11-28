#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyProbeResult {
    pub supported: bool,
    pub flags: Option<u8>,
    pub buffered_bytes: Vec<u8>,
}

#[cfg(unix)]
mod imp {
    use std::collections::VecDeque;
    use std::io::{self, Write};
    use std::os::unix::io::RawFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;
    use std::time::{Duration, Instant};

    use crossterm::event::Event;

    use super::KittyProbeResult;
    use crate::terminal_decoder::parser::{
        DecodedInput, KittyProbeScanner, TerminalInputDecoder,
    };

    const ESC_IDLE_TIMEOUT: Duration = Duration::from_millis(25);
    const KITTY_AND_DA1_PROBE: &[u8] = b"\x1b[?u\x1b[c";

    static SIGWINCH_PENDING: AtomicBool = AtomicBool::new(false);
    static SIGWINCH_INIT: Once = Once::new();

    extern "C" fn handle_sigwinch(_sig: libc::c_int) {
        SIGWINCH_PENDING.store(true, Ordering::Release);
    }

    fn install_sigwinch_handler() {
        SIGWINCH_INIT.call_once(|| unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handle_sigwinch as *const () as usize;
            // Omit SA_RESTART so blocking libc::poll wakes promptly on SIGWINCH.
            action.sa_flags = 0;
            libc::sigemptyset(&mut action.sa_mask);
            let _ = libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut());
        });
    }

    pub struct UnixRawInputReader {
        fd: RawFd,
        decoder: TerminalInputDecoder,
        pending: VecDeque<DecodedInput>,
        esc_pending_since: Option<Instant>,
    }

    impl UnixRawInputReader {
        pub fn new() -> io::Result<Self> {
            install_sigwinch_handler();
            Ok(Self::with_fd(libc::STDIN_FILENO))
        }

        pub fn with_fd(fd: RawFd) -> Self {
            install_sigwinch_handler();
            Self {
                fd,
                decoder: TerminalInputDecoder::new(),
                pending: VecDeque::new(),
                esc_pending_since: None,
            }
        }

        pub fn push_initial_bytes(&mut self, bytes: &[u8]) {
            if bytes.is_empty() {
                return;
            }
            for ev in self.decoder.feed(bytes) {
                self.pending.push_back(ev);
            }
            self.sync_esc_timer();
        }

        pub fn read_next(&mut self, timeout: Duration) -> io::Result<Option<DecodedInput>> {
            self.enqueue_resize_if_pending();
            if let Some(ev) = self.pending.pop_front() {
                return Ok(Some(ev));
            }

            let deadline = Instant::now() + timeout;
            let mut buf = [0u8; 4096];

            loop {
                if let Some(ev) = self.maybe_flush_idle_esc() {
                    return Ok(Some(ev));
                }

                let now = Instant::now();
                let remaining = deadline.saturating_duration_since(now);
                let wait_dur = match self.esc_pending_since {
                    Some(since) => {
                        let until_esc_flush = ESC_IDLE_TIMEOUT.saturating_sub(now - since);
                        remaining.min(until_esc_flush)
                    }
                    None => remaining,
                };
                let timeout_ms = wait_dur.as_millis().min(i32::MAX as u128) as i32;

                let mut pfd = libc::pollfd {
                    fd: self.fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let poll_ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
                if poll_ret < 0 {
                    let err = io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::EINTR) {
                        self.enqueue_resize_if_pending();
                        if let Some(ev) = self.pending.pop_front() {
                            return Ok(Some(ev));
                        }
                        if Instant::now() >= deadline {
                            return Ok(self.maybe_flush_idle_esc());
                        }
                        continue;
                    }
                    return Err(err);
                }

                if poll_ret == 0 {
                    if let Some(ev) = self.maybe_flush_idle_esc() {
                        return Ok(Some(ev));
                    }
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    continue;
                }

                if (pfd.revents & libc::POLLIN) != 0 {
                    let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
                    if n < 0 {
                        let err = io::Error::last_os_error();
                        if err.raw_os_error() == Some(libc::EINTR) {
                            self.enqueue_resize_if_pending();
                            if let Some(ev) = self.pending.pop_front() {
                                return Ok(Some(ev));
                            }
                            continue;
                        }
                        return Err(err);
                    }
                    if n == 0 {
                        return Ok(self.decoder.flush_pending_esc());
                    }
                    for ev in self.decoder.feed(&buf[..n as usize]) {
                        self.pending.push_back(ev);
                    }
                    self.sync_esc_timer();
                    if let Some(ev) = self.pending.pop_front() {
                        return Ok(Some(ev));
                    }
                } else if (pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0 {
                    return Ok(self.decoder.flush_pending_esc());
                }

                if Instant::now() >= deadline {
                    return Ok(self.maybe_flush_idle_esc());
                }
            }
        }

        pub fn probe_kitty_keyboard(timeout_ms: u64) -> io::Result<KittyProbeResult> {
            let mut stdout = io::stdout().lock();
            Self::probe_kitty_keyboard_io(libc::STDIN_FILENO, &mut stdout, timeout_ms)
        }

        pub fn probe_kitty_keyboard_io<W: Write>(
            fd: RawFd,
            writer: &mut W,
            timeout_ms: u64,
        ) -> io::Result<KittyProbeResult> {
            writer.write_all(KITTY_AND_DA1_PROBE)?;
            writer.flush()?;

            let mut scanner = KittyProbeScanner::new();
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            let mut buf = [0u8; 1024];

            while !scanner.da1_seen {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining_ms = deadline
                    .saturating_duration_since(now)
                    .as_millis()
                    .min(i32::MAX as u128) as i32;
                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let poll_ret = unsafe { libc::poll(&mut pfd, 1, remaining_ms) };
                if poll_ret < 0 {
                    let err = io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    return Err(err);
                }
                if poll_ret == 0 {
                    break;
                }
                if (pfd.revents & libc::POLLIN) != 0 {
                    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                    if n < 0 {
                        let err = io::Error::last_os_error();
                        if err.raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }
                        return Err(err);
                    }
                    if n == 0 {
                        break;
                    }
                    scanner.feed(&buf[..n as usize]);
                } else if (pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0 {
                    break;
                }
            }

            let buffered_bytes = scanner.finish();
            Ok(KittyProbeResult {
                supported: scanner.kitty_flags.is_some(),
                flags: scanner.kitty_flags,
                buffered_bytes,
            })
        }

        fn enqueue_resize_if_pending(&mut self) {
            if SIGWINCH_PENDING.swap(false, Ordering::AcqRel) {
                if let Ok((cols, rows)) = crossterm::terminal::size() {
                    self.pending.push_back(DecodedInput {
                        event: Event::Resize(cols, rows),
                        raw_bytes: Vec::new(),
                    });
                }
            }
        }

        fn sync_esc_timer(&mut self) {
            if self.decoder.has_pending_esc() {
                if self.esc_pending_since.is_none() {
                    self.esc_pending_since = Some(Instant::now());
                }
            } else {
                self.esc_pending_since = None;
            }
        }

        fn maybe_flush_idle_esc(&mut self) -> Option<DecodedInput> {
            let since = self.esc_pending_since?;
            if since.elapsed() >= ESC_IDLE_TIMEOUT {
                self.esc_pending_since = None;
                return self.decoder.flush_pending_esc();
            }
            None
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::io;
    use std::time::Duration;

    use super::KittyProbeResult;
    use crate::terminal_decoder::parser::DecodedInput;

    #[derive(Debug, Default)]
    pub struct UnixRawInputReader;

    impl UnixRawInputReader {
        pub fn new() -> io::Result<Self> {
            Ok(Self)
        }

        pub fn push_initial_bytes(&mut self, _bytes: &[u8]) {}

        pub fn read_next(&mut self, timeout: Duration) -> io::Result<Option<DecodedInput>> {
            if crossterm::event::poll(timeout)? {
                let event = crossterm::event::read()?;
                Ok(Some(DecodedInput {
                    event,
                    raw_bytes: Vec::new(),
                }))
            } else {
                Ok(None)
            }
        }

        pub fn probe_kitty_keyboard(_timeout_ms: u64) -> io::Result<KittyProbeResult> {
            let supported = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
            Ok(KittyProbeResult {
                supported,
                flags: supported.then_some(0),
                buffered_bytes: Vec::new(),
            })
        }
    }
}

pub use imp::UnixRawInputReader;
