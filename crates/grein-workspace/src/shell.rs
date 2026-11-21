use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TRUNCATED_SUFFIX: &str = "\n...[truncated]";
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const TIMEOUT_DRAIN_WAIT: Duration = Duration::from_millis(25);

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
}

impl OutputCapture {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            bytes: Vec::new(),
            exceeded: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
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

    fn format_output(&self) -> String {
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
    fn spawn<R: Read + Send + 'static>(pipe: Option<R>, limit: usize) -> Self {
        let capture = Arc::new(Mutex::new(OutputCapture::new(limit)));
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
            .map(|guard| guard.format_output())
            .unwrap_or_default()
    }
}

pub fn run_shell_command_blocking(
    cwd: &Path,
    command: &str,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<ShellCommandOutput> {
    if command.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "command must not be empty",
        ));
    }

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout_reader = PipeReader::spawn(child.stdout.take(), max_output_bytes);
    let stderr_reader = PipeReader::spawn(child.stderr.take(), max_output_bytes);

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
        Ok(ShellCommandOutput {
            command: command.to_string(),
            stdout: stdout_reader.finish_until(None),
            stderr: stderr_reader.finish_until(None),
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
}
