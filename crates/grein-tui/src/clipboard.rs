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

pub fn copy_to_clipboard(text: &str) -> Result<&'static str, String> {
    let in_tmux = std::env::var_os("TMUX").is_some_and(|v| !v.is_empty());
    let osc_written = if std::io::stdout().is_terminal() {
        let mut stdout = std::io::stdout().lock();
        write_osc52_to(&mut stdout, text, in_tmux).is_ok()
    } else {
        false
    };

    let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let candidates = native_clipboard_candidates(cfg!(target_os = "macos"), has_wayland);

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
