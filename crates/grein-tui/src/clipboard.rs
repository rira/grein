use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

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

fn write_osc52_to<W: Write>(writer: &mut W, text: &str, in_tmux: bool) -> std::io::Result<()> {
    let seq = encode_osc52(text, in_tmux);
    writer.write_all(seq.as_bytes())?;
    writer.flush()
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

fn read_from_candidates(
    candidates: &[(&'static str, &'static [&'static str])],
) -> Result<String, String> {
    let mut errors = Vec::new();
    for &(program, args) in candidates {
        match try_clipboard_read_command(program, args) {
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

pub fn copy_to_clipboard(text: &str) -> Result<&'static str, String> {
    let in_tmux = std::env::var_os("TMUX").is_some_and(|v| !v.is_empty());
    let osc_written = if std::io::stdout().is_terminal() {
        let mut stdout = std::io::stdout().lock();
        write_osc52_to(&mut stdout, text, in_tmux).is_ok()
    } else {
        false
    };

    let candidates = native_clipboard_candidates(cfg!(target_os = "macos"), has_wayland_display());

    let mut errors = Vec::new();
    for &(program, args) in candidates {
        match try_clipboard_command(program, args, text) {
            Ok(()) => return Ok(program),
            Err(err) => errors.push(err),
        }
    }

    if osc_written {
        Ok("osc52")
    } else if errors.is_empty() {
        Err("no clipboard backend available".to_string())
    } else {
        Err(errors.join("; "))
    }
}

pub fn read_from_clipboard() -> Result<String, String> {
    let candidates =
        native_clipboard_read_candidates(cfg!(target_os = "macos"), has_wayland_display());
    read_from_candidates(candidates)
}

pub fn read_primary_selection() -> Result<String, String> {
    let candidates =
        native_primary_read_candidates(cfg!(target_os = "macos"), has_wayland_display());
    read_from_candidates(candidates)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestoreGuard {
        prev_path: Option<OsString>,
        prev_wayland: Option<OsString>,
    }

    impl EnvRestoreGuard {
        fn capture_and_set(path: &str, wayland: Option<&str>) -> Self {
            let guard = Self {
                prev_path: std::env::var_os("PATH"),
                prev_wayland: std::env::var_os("WAYLAND_DISPLAY"),
            };
            std::env::set_var("PATH", path);
            match wayland {
                Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
            guard
        }
    }

    impl Drop for EnvRestoreGuard {
        fn drop(&mut self) {
            match &self.prev_path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match &self.prev_wayland {
                Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
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
        write_osc52_to(&mut buf, "abc", false).unwrap();
        assert_eq!(buf, b"\x1b]52;c;YWJj\x07");

        buf.clear();
        write_osc52_to(&mut buf, "abc", true).unwrap();
        assert_eq!(buf, b"\x1bPtmux;\x1b\x1b]52;c;YWJj\x07\x1b\\");
    }

    #[test]
    fn test_native_clipboard_candidates_selection() {
        assert_eq!(native_clipboard_candidates(true, false)[0].0, "pbcopy");
        assert_eq!(native_clipboard_candidates(false, true)[0].0, "wl-copy");
        let x11 = native_clipboard_candidates(false, false);
        assert_eq!(x11[0], ("xclip", &["-selection", "clipboard"][..]));
        assert_eq!(x11[1], ("xsel", &["--clipboard", "--input"][..]));
    }

    #[test]
    fn test_native_clipboard_and_primary_read_candidates_selection() {
        assert_eq!(
            native_clipboard_read_candidates(true, false),
            &[("pbpaste", &[][..])]
        );
        assert_eq!(
            native_clipboard_read_candidates(false, true),
            &[
                ("wl-paste", &["--no-newline"][..]),
                ("xclip", &["-o", "-selection", "clipboard"][..]),
                ("xsel", &["--clipboard", "--output"][..]),
            ]
        );
        assert_eq!(
            native_clipboard_read_candidates(false, false),
            &[
                ("xclip", &["-o", "-selection", "clipboard"][..]),
                ("xsel", &["--clipboard", "--output"][..]),
            ]
        );

        assert_eq!(
            native_primary_read_candidates(true, false),
            &[("pbpaste", &[][..])]
        );
        assert_eq!(
            native_primary_read_candidates(false, true),
            &[
                ("wl-paste", &["--primary", "--no-newline"][..]),
                ("xclip", &["-o", "-selection", "primary"][..]),
                ("xsel", &["--primary", "--output"][..]),
            ]
        );
        assert_eq!(
            native_primary_read_candidates(false, false),
            &[
                ("xclip", &["-o", "-selection", "primary"][..]),
                ("xsel", &["--primary", "--output"][..]),
            ]
        );
    }

    #[test]
    fn test_clipboard_read_command_and_candidate_fallback() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let missing =
            try_clipboard_read_command("/nonexistent/grein-missing-clipboard-tool", &["--flag"]);
        assert!(missing.is_err());

        let failing = try_clipboard_read_command("false", &[]);
        assert!(failing.unwrap_err().contains("exited with"));

        let empty_ok = try_clipboard_read_command("true", &[]);
        assert_eq!(empty_ok, Ok(String::new()));

        let text_ok = try_clipboard_read_command("sh", &["-c", "printf 'line1\\nline2'"]);
        assert_eq!(text_ok, Ok("line1\nline2".to_string()));

        let bad_utf8 = try_clipboard_read_command("sh", &["-c", "printf '\\377'"]);
        assert!(bad_utf8.unwrap_err().contains("utf8"));

        let fallback = read_from_candidates(&[
            ("/nonexistent/grein-missing-clipboard-tool", &[]),
            ("false", &[]),
            ("sh", &["-c", "printf 'from-fallback'"]),
        ]);
        assert_eq!(fallback, Ok("from-fallback".to_string()));

        let all_fail = read_from_candidates(&[("false", &[]), ("/nonexistent/tool", &[])]);
        let err = all_fail.unwrap_err();
        assert!(err.contains("false exited with"));
        assert!(err.contains("/nonexistent/tool"));

        assert_eq!(
            read_from_candidates(&[]),
            Err("no clipboard backend available".to_string())
        );
    }

    #[test]
    fn test_base64_all_byte_values_and_clipboard_command_paths() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let all_bytes: Vec<u8> = (0u8..=255).collect();
        let encoded = encode_base64(&all_bytes);
        assert_eq!(encoded.len(), 344);
        assert!(encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='));
        assert!(encoded.ends_with('='));

        let missing = try_clipboard_command(
            "/nonexistent/grein-missing-clipboard-tool",
            &["--flag"],
            "payload",
        );
        assert!(missing.is_err());

        let failing = try_clipboard_command("false", &[], "payload");
        assert!(failing.is_err());

        let ok = try_clipboard_command("cat", &[], "payload");
        assert!(ok.is_ok());

        let _env = EnvRestoreGuard::capture_and_set("", Some("wayland-test"));

        let res = copy_to_clipboard("hello");
        if std::io::stdout().is_terminal() {
            assert_eq!(res, Ok("osc52"));
        } else {
            let err = res.unwrap_err();
            assert!(err.contains("wl-copy"));
            assert!(err.contains("xclip"));
            assert!(err.contains("xsel"));
        }
    }

    #[test]
    fn test_read_clipboard_and_primary_error_paths_with_empty_path() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        {
            let _env = EnvRestoreGuard::capture_and_set("", Some("wayland-test"));
            let clip_err = read_from_clipboard().unwrap_err();
            let prim_err = read_primary_selection().unwrap_err();
            if cfg!(target_os = "macos") {
                assert!(clip_err.contains("pbpaste"));
                assert!(prim_err.contains("pbpaste"));
            } else {
                assert!(clip_err.contains("wl-paste"));
                assert!(clip_err.contains("xclip"));
                assert!(clip_err.contains("xsel"));
                assert!(prim_err.contains("wl-paste"));
                assert!(prim_err.contains("xclip"));
                assert!(prim_err.contains("xsel"));
            }
        }

        if !cfg!(target_os = "macos") {
            let _env = EnvRestoreGuard::capture_and_set("", None);
            let clip_err = read_from_clipboard().unwrap_err();
            let prim_err = read_primary_selection().unwrap_err();
            assert!(!clip_err.contains("wl-paste"));
            assert!(clip_err.contains("xclip"));
            assert!(clip_err.contains("xsel"));
            assert!(!prim_err.contains("wl-paste"));
            assert!(prim_err.contains("xclip"));
            assert!(prim_err.contains("xsel"));
        }
    }
}
