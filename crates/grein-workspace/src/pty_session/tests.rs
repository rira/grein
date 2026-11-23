use std::path::Path;

use super::reader::{process_pty_bytes, DA1_REPLY, DA2_REPLY, OSC10_FG_REPLY, OSC11_BG_REPLY};
use super::scanner::{parse_osc7_cwd, ScannerEvent, TerminalScanner, MAX_OSC_PAYLOAD_BYTES};
use super::state::{fit_row_to_cols, map_vt100_color, PtyState};
#[cfg(unix)]
use super::unix::{build_pty_command, resolve_shell, SCRUBBED_TERMINAL_ENV_VARS};
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
    let pinned_first = state.scrollback[state.scrollback.len() - 3][0]
        .contents
        .clone();
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
    process_pty_bytes(
        b"\x1b[?1049hALT\n\n\n\n\n",
        &mut scanner,
        &mut state,
        &mut responses,
    );
    assert_eq!(state.scrollback_offset, 0);
    assert_eq!(state.clamp_scrollback_offset(5), 0);
    assert_eq!(state.scrollback.len(), before_region);
    process_pty_bytes(b"\x1b[?1049l", &mut scanner, &mut state, &mut responses);
    assert_eq!(state.clamp_scrollback_offset(5), 5);

    // Line wrap + CSI S scroll into scrollback cleanly
    let before_wrap_and_su = state.scrollback.len();
    process_pty_bytes(
        b"\x1b[4;1H1234567890WRAP",
        &mut scanner,
        &mut state,
        &mut responses,
    );
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
