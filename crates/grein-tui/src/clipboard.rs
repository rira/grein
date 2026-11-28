use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::terminal::providers::default_adapter;
use crate::terminal::{TerminalAdapter, TerminalEnv};

const BASE64_TABLE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode_base64(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(BASE64_TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn encode_osc52(text: &str, in_tmux: bool) -> String {
    let b64 = encode_base64(text.as_bytes());
    if in_tmux {
        format!("\x1bPtmux;\x1b\x1b]52;c;{b64}\x07\x1b\\")
    } else {
        format!("\x1b]52;c;{b64}\x07")
    }
}

fn write_osc52_seq<W: Write>(writer: &mut W, seq: &str) -> Result<(), String> {
    writer
        .write_all(seq.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|err| format!("osc52: {err}"))
}

pub trait ClipboardPort: Send + Sync {
    fn copy(&self, text: &str) -> Result<String, String>;
    fn read_clipboard(&self) -> Result<String, String>;
    fn read_primary(&self) -> Result<String, String>;
}

impl<T: ClipboardPort + ?Sized> ClipboardPort for Arc<T> {
    fn copy(&self, text: &str) -> Result<String, String> {
        (**self).copy(text)
    }

    fn read_clipboard(&self) -> Result<String, String> {
        (**self).read_clipboard()
    }

    fn read_primary(&self) -> Result<String, String> {
        (**self).read_primary()
    }
}

impl<T: ClipboardPort + ?Sized> ClipboardPort for Box<T> {
    fn copy(&self, text: &str) -> Result<String, String> {
        (**self).copy(text)
    }

    fn read_clipboard(&self) -> Result<String, String> {
        (**self).read_clipboard()
    }

    fn read_primary(&self) -> Result<String, String> {
        (**self).read_primary()
    }
}

pub trait ClipboardWriterBackend: Send + Sync {
    fn name(&self) -> &str;
    fn write(&self, text: &str) -> Result<(), String>;
    fn is_fallback(&self) -> bool {
        false
    }
}

pub trait ClipboardReaderBackend: Send + Sync {
    fn name(&self) -> &str;
    fn read(&self) -> Result<String, String>;
}

enum Osc52Target {
    Stdout(Arc<dyn TerminalAdapter>),
    Sink {
        adapter: Arc<dyn TerminalAdapter>,
        sink: Arc<Mutex<Vec<u8>>>,
    },
}

pub struct Osc52Backend {
    target: Osc52Target,
}

impl Osc52Backend {
    pub fn from_env() -> Self {
        Self::from_adapter(default_adapter(&TerminalEnv::from_env()))
    }

    pub fn from_adapter(adapter: Arc<dyn TerminalAdapter>) -> Self {
        Self {
            target: Osc52Target::Stdout(adapter),
        }
    }

    pub fn with_sink(in_tmux: bool, sink: Arc<Mutex<Vec<u8>>>) -> Self {
        let env = if in_tmux {
            TerminalEnv::default().with_tmux("1")
        } else {
            TerminalEnv::default()
        };
        Self::with_adapter_and_sink(default_adapter(&env), sink)
    }

    pub fn with_adapter_and_sink(
        adapter: Arc<dyn TerminalAdapter>,
        sink: Arc<Mutex<Vec<u8>>>,
    ) -> Self {
        Self {
            target: Osc52Target::Sink { adapter, sink },
        }
    }
}

impl Default for Osc52Backend {
    fn default() -> Self {
        Self::from_env()
    }
}

impl ClipboardWriterBackend for Osc52Backend {
    fn name(&self) -> &str {
        "osc52"
    }

    fn write(&self, text: &str) -> Result<(), String> {
        let b64 = encode_base64(text.as_bytes());
        match &self.target {
            Osc52Target::Stdout(adapter) => {
                if !std::io::stdout().is_terminal() {
                    return Err("osc52: stdout is not a terminal".to_string());
                }
                let seq = adapter.encode_osc52(&b64);
                let mut stdout = std::io::stdout().lock();
                write_osc52_seq(&mut stdout, &seq)
            }
            Osc52Target::Sink { adapter, sink } => {
                let seq = adapter.encode_osc52(&b64);
                let mut guard = sink
                    .lock()
                    .map_err(|err| format!("osc52 sink lock: {err}"))?;
                write_osc52_seq(&mut *guard, &seq)
            }
        }
    }

    fn is_fallback(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandClipboardBackend {
    program: String,
    args: Vec<String>,
}

impl CommandClipboardBackend {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

impl ClipboardWriterBackend for CommandClipboardBackend {
    fn name(&self) -> &str {
        &self.program
    }

    fn write(&self, text: &str) -> Result<(), String> {
        let arg_refs: Vec<&str> = self.args.iter().map(String::as_str).collect();
        try_clipboard_command(&self.program, &arg_refs, text)
    }
}

impl ClipboardReaderBackend for CommandClipboardBackend {
    fn name(&self) -> &str {
        &self.program
    }

    fn read(&self) -> Result<String, String> {
        let arg_refs: Vec<&str> = self.args.iter().map(String::as_str).collect();
        try_clipboard_read_command(&self.program, &arg_refs)
    }
}

#[derive(Clone)]
pub struct ChainedClipboardDriver {
    writers: Vec<Arc<dyn ClipboardWriterBackend>>,
    clipboard_readers: Vec<Arc<dyn ClipboardReaderBackend>>,
    primary_readers: Vec<Arc<dyn ClipboardReaderBackend>>,
}

impl ChainedClipboardDriver {
    pub fn new(
        writers: Vec<Arc<dyn ClipboardWriterBackend>>,
        clipboard_readers: Vec<Arc<dyn ClipboardReaderBackend>>,
        primary_readers: Vec<Arc<dyn ClipboardReaderBackend>>,
    ) -> Self {
        Self {
            writers,
            clipboard_readers,
            primary_readers,
        }
    }

    pub fn from_slices(
        writers: &[Arc<dyn ClipboardWriterBackend>],
        clipboard_readers: &[Arc<dyn ClipboardReaderBackend>],
        primary_readers: &[Arc<dyn ClipboardReaderBackend>],
    ) -> Self {
        Self::new(
            writers.to_vec(),
            clipboard_readers.to_vec(),
            primary_readers.to_vec(),
        )
    }

    pub fn for_platform(is_macos: bool, has_wayland: bool) -> Self {
        let mut writers: Vec<Arc<dyn ClipboardWriterBackend>> =
            vec![Arc::new(Osc52Backend::from_env())];
        for &(program, args) in native_clipboard_candidates(is_macos, has_wayland) {
            writers.push(Arc::new(CommandClipboardBackend::new(
                program,
                args.iter().copied(),
            )));
        }

        let clipboard_readers = native_clipboard_read_candidates(is_macos, has_wayland)
            .iter()
            .map(|&(program, args)| {
                Arc::new(CommandClipboardBackend::new(program, args.iter().copied()))
                    as Arc<dyn ClipboardReaderBackend>
            })
            .collect::<Vec<_>>();

        let primary_readers = native_primary_read_candidates(is_macos, has_wayland)
            .iter()
            .map(|&(program, args)| {
                Arc::new(CommandClipboardBackend::new(program, args.iter().copied()))
                    as Arc<dyn ClipboardReaderBackend>
            })
            .collect::<Vec<_>>();

        Self::new(writers, clipboard_readers, primary_readers)
    }

    pub fn writer_names(&self) -> Vec<&str> {
        self.writers.iter().map(|w| w.name()).collect()
    }

    pub fn clipboard_reader_names(&self) -> Vec<&str> {
        self.clipboard_readers.iter().map(|r| r.name()).collect()
    }

    pub fn primary_reader_names(&self) -> Vec<&str> {
        self.primary_readers.iter().map(|r| r.name()).collect()
    }
}

impl Default for ChainedClipboardDriver {
    fn default() -> Self {
        Self::for_platform(cfg!(target_os = "macos"), has_wayland_display())
    }
}

impl ClipboardPort for ChainedClipboardDriver {
    fn copy(&self, text: &str) -> Result<String, String> {
        let mut fallback_success: Option<String> = None;
        let mut fallback_errors = Vec::new();
        let mut errors = Vec::new();

        for writer in &self.writers {
            match writer.write(text) {
                Ok(()) => {
                    if writer.is_fallback() {
                        if fallback_success.is_none() {
                            fallback_success = Some(writer.name().to_string());
                        }
                    } else {
                        return Ok(writer.name().to_string());
                    }
                }
                Err(err) => {
                    if writer.is_fallback() {
                        fallback_errors.push(err);
                    } else {
                        errors.push(err);
                    }
                }
            }
        }

        if let Some(name) = fallback_success {
            Ok(name)
        } else if !errors.is_empty() {
            Err(errors.join("; "))
        } else if !fallback_errors.is_empty() {
            Err(fallback_errors.join("; "))
        } else {
            Err("no clipboard backend available".to_string())
        }
    }

    fn read_clipboard(&self) -> Result<String, String> {
        read_with_backends(&self.clipboard_readers)
    }

    fn read_primary(&self) -> Result<String, String> {
        read_with_backends(&self.primary_readers)
    }
}

fn read_with_backends(readers: &[Arc<dyn ClipboardReaderBackend>]) -> Result<String, String> {
    let mut errors = Vec::new();
    for reader in readers {
        match reader.read() {
            Ok(text) => return Ok(text),
            Err(err) => errors.push(err),
        }
    }

    if errors.is_empty() {
        Err("no clipboard backend available".to_string())
    } else {
        Err(errors.join("; "))
    }
}

fn has_wayland_display() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

fn native_clipboard_candidates(
    is_macos: bool,
    has_wayland: bool,
) -> &'static [(&'static str, &'static [&'static str])] {
    if is_macos {
        &[("pbcopy", &[])]
    } else if has_wayland {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    } else {
        &[
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
}

fn native_clipboard_read_candidates(
    is_macos: bool,
    has_wayland: bool,
) -> &'static [(&'static str, &'static [&'static str])] {
    if is_macos {
        &[("pbpaste", &[])]
    } else if has_wayland {
        &[
            ("wl-paste", &["--no-newline"]),
            ("xclip", &["-o", "-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--output"]),
        ]
    } else {
        &[
            ("xclip", &["-o", "-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--output"]),
        ]
    }
}

fn native_primary_read_candidates(
    is_macos: bool,
    has_wayland: bool,
) -> &'static [(&'static str, &'static [&'static str])] {
    if is_macos {
        &[("pbpaste", &[])]
    } else if has_wayland {
        &[
            ("wl-paste", &["--primary", "--no-newline"]),
            ("xclip", &["-o", "-selection", "primary"]),
            ("xsel", &["--primary", "--output"]),
        ]
    } else {
        &[
            ("xclip", &["-o", "-selection", "primary"]),
            ("xsel", &["--primary", "--output"]),
        ]
    }
}

fn try_clipboard_command(program: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("{program}: {err}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|err| format!("{program} stdin: {err}"))?;
    }

    let status = child.wait().map_err(|err| format!("{program} wait: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

fn try_clipboard_read_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;

    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|err| format!("{program} utf8: {err}"))
    } else {
        Err(format!("{program} exited with {}", output.status))
    }
}

pub fn copy_to_clipboard(text: &str) -> Result<String, String> {
    ChainedClipboardDriver::default().copy(text)
}

pub fn read_from_clipboard() -> Result<String, String> {
    ChainedClipboardDriver::default().read_clipboard()
}

pub fn read_primary_selection() -> Result<String, String> {
    ChainedClipboardDriver::default().read_primary()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticWriter {
        name: &'static str,
        result: Result<(), String>,
        recorded: Arc<Mutex<Vec<String>>>,
        fallback: bool,
    }

    impl StaticWriter {
        fn ok(
            name: &'static str,
            recorded: Arc<Mutex<Vec<String>>>,
        ) -> Arc<dyn ClipboardWriterBackend> {
            Arc::new(Self {
                name,
                result: Ok(()),
                recorded,
                fallback: false,
            })
        }

        fn err(
            name: &'static str,
            msg: &str,
            recorded: Arc<Mutex<Vec<String>>>,
        ) -> Arc<dyn ClipboardWriterBackend> {
            Arc::new(Self {
                name,
                result: Err(format!("{name}: {msg}")),
                recorded,
                fallback: false,
            })
        }
    }

    impl ClipboardWriterBackend for StaticWriter {
        fn name(&self) -> &str {
            self.name
        }

        fn write(&self, text: &str) -> Result<(), String> {
            if let Ok(mut guard) = self.recorded.lock() {
                guard.push(format!("{}:{text}", self.name));
            }
            self.result.clone()
        }

        fn is_fallback(&self) -> bool {
            self.fallback
        }
    }

    struct StaticReader {
        name: &'static str,
        result: Result<String, String>,
    }

    impl StaticReader {
        fn ok(name: &'static str, text: &str) -> Arc<dyn ClipboardReaderBackend> {
            Arc::new(Self {
                name,
                result: Ok(text.to_string()),
            })
        }

        fn err(name: &'static str, msg: &str) -> Arc<dyn ClipboardReaderBackend> {
            Arc::new(Self {
                name,
                result: Err(format!("{name}: {msg}")),
            })
        }
    }

    impl ClipboardReaderBackend for StaticReader {
        fn name(&self) -> &str {
            self.name
        }

        fn read(&self) -> Result<String, String> {
            self.result.clone()
        }
    }

    #[test]
    fn test_base64_rfc4648_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_write_osc52_to_writer() {
        let mut buf = Vec::new();
        write_osc52_seq(&mut buf, &encode_osc52("abc", false)).unwrap();
        assert_eq!(buf, b"\x1b]52;c;YWJj\x07");

        buf.clear();
        write_osc52_seq(&mut buf, &encode_osc52("abc", true)).unwrap();
        assert_eq!(buf, b"\x1bPtmux;\x1b\x1b]52;c;YWJj\x07\x1b\\");
    }

    #[test]
    fn test_for_platform_registers_expected_backend_chains() {
        let mac = ChainedClipboardDriver::for_platform(true, false);
        assert_eq!(mac.writer_names(), vec!["osc52", "pbcopy"]);
        assert_eq!(mac.clipboard_reader_names(), vec!["pbpaste"]);
        assert_eq!(mac.primary_reader_names(), vec!["pbpaste"]);

        let wayland = ChainedClipboardDriver::for_platform(false, true);
        assert_eq!(
            wayland.writer_names(),
            vec!["osc52", "wl-copy", "xclip", "xsel"]
        );
        assert_eq!(
            wayland.clipboard_reader_names(),
            vec!["wl-paste", "xclip", "xsel"]
        );
        assert_eq!(
            wayland.primary_reader_names(),
            vec!["wl-paste", "xclip", "xsel"]
        );

        let x11 = ChainedClipboardDriver::for_platform(false, false);
        assert_eq!(x11.writer_names(), vec!["osc52", "xclip", "xsel"]);
        assert_eq!(x11.clipboard_reader_names(), vec!["xclip", "xsel"]);
        assert_eq!(x11.primary_reader_names(), vec!["xclip", "xsel"]);
    }

    #[test]
    fn test_chained_driver_copy_prefers_native_over_osc52_and_falls_back_cleanly() {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let driver = ChainedClipboardDriver::new(
            vec![
                Arc::new(Osc52Backend::with_sink(false, Arc::clone(&sink)))
                    as Arc<dyn ClipboardWriterBackend>,
                StaticWriter::err("wl-copy", "missing", Arc::clone(&recorded)),
                StaticWriter::ok("xclip", Arc::clone(&recorded)),
                StaticWriter::ok("xsel", Arc::clone(&recorded)),
            ],
            vec![],
            vec![],
        );

        assert_eq!(driver.copy("hello"), Ok("xclip".to_string()));
        assert_eq!(*sink.lock().unwrap(), b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(
            *recorded.lock().unwrap(),
            vec!["wl-copy:hello".to_string(), "xclip:hello".to_string()]
        );

        let osc_sink = Arc::new(Mutex::new(Vec::new()));
        let osc_only_fallback = ChainedClipboardDriver::new(
            vec![
                Arc::new(Osc52Backend::with_sink(true, Arc::clone(&osc_sink)))
                    as Arc<dyn ClipboardWriterBackend>,
                StaticWriter::err("wl-copy", "missing", Arc::new(Mutex::new(Vec::new()))),
            ],
            vec![],
            vec![],
        );
        assert_eq!(osc_only_fallback.copy("abc"), Ok("osc52".to_string()));
        assert_eq!(
            *osc_sink.lock().unwrap(),
            b"\x1bPtmux;\x1b\x1b]52;c;YWJj\x07\x1b\\"
        );

        let empty = ChainedClipboardDriver::new(vec![], vec![], vec![]);
        assert_eq!(
            empty.copy("x"),
            Err("no clipboard backend available".to_string())
        );
        assert_eq!(
            empty.read_clipboard(),
            Err("no clipboard backend available".to_string())
        );
        assert_eq!(
            empty.read_primary(),
            Err("no clipboard backend available".to_string())
        );
    }

    #[test]
    fn test_chained_driver_injected_readers_fallback_and_error_aggregation() {
        let driver = ChainedClipboardDriver::new(
            vec![
                StaticWriter::err("wl-copy", "absent", Arc::new(Mutex::new(Vec::new()))),
                StaticWriter::err("xclip", "absent", Arc::new(Mutex::new(Vec::new()))),
            ],
            vec![
                StaticReader::err("wl-paste", "absent"),
                StaticReader::ok("xclip", "clip-text"),
            ],
            vec![
                StaticReader::err("wl-paste", "absent"),
                StaticReader::err("xclip", "empty"),
            ],
        );

        assert_eq!(
            driver.copy("payload"),
            Err("wl-copy: absent; xclip: absent".to_string())
        );
        assert_eq!(driver.read_clipboard(), Ok("clip-text".to_string()));
        assert_eq!(
            driver.read_primary(),
            Err("wl-paste: absent; xclip: empty".to_string())
        );
    }

    #[test]
    fn test_command_clipboard_backend_read_and_write() {
        let missing =
            CommandClipboardBackend::new("/nonexistent/grein-missing-clipboard-tool", ["--flag"]);
        assert!(missing.read().is_err());
        assert!(missing.write("payload").is_err());

        let failing = CommandClipboardBackend::new("false", std::iter::empty::<&str>());
        assert!(failing.read().unwrap_err().contains("exited with"));
        assert!(failing.write("payload").is_err());

        let empty_ok = CommandClipboardBackend::new("true", std::iter::empty::<&str>());
        assert_eq!(empty_ok.read(), Ok(String::new()));

        let text_ok = CommandClipboardBackend::new("sh", ["-c", "printf 'line1\\nline2'"]);
        assert_eq!(text_ok.read(), Ok("line1\nline2".to_string()));

        let bad_utf8 = CommandClipboardBackend::new("sh", ["-c", "printf '\\377'"]);
        assert!(bad_utf8.read().unwrap_err().contains("utf8"));

        let cat_ok = CommandClipboardBackend::new("cat", std::iter::empty::<&str>());
        assert!(cat_ok.write("payload").is_ok());

        let all_bytes: Vec<u8> = (0u8..=255).collect();
        let encoded = encode_base64(&all_bytes);
        assert_eq!(encoded.len(), 344);
        assert!(encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='));
        assert!(encoded.ends_with('='));
    }
}
