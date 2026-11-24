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

#[test]
fn test_scanner_intermediate_ignore_dcs_overflow_and_utf8_recovery() {
    let mut scanner = TerminalScanner::default();

    assert!(scanner.executes_c0());
    assert!(scanner.will_print_byte(b'A'));
    assert!(!scanner.will_print_byte(0x0a));

    // EscIntermediate: ESC ( B then abort via CAN/SUB and ESC restart
    let esc_inter = b"\x1b(B\x1b(\x00 \x18\x1b(\x1a\x1b(\x1b[5n";
    let mut events = Vec::new();
    for &b in esc_inter {
        if let Some(ev) = scanner.feed_byte(b) {
            events.push(ev);
        }
    }
    assert_eq!(events, vec![ScannerEvent::DeviceStatus]);

    // CsiIntermediate -> ground / CsiIgnore -> ground, plus CAN/SUB and ':' subparam
    let csi_cases = b"\x1b[1 p\x1b[ 1?m\x1b[ \x18\x1b[ \x1b[5n\x1b[4:3m\x1b[5:1n\x1b[1?5n\x1b[1?\x80\x1b[1?\x18\x1b[1?\x1a\x1b[1?\x1b[6n";
    events.clear();
    for &b in csi_cases {
        if let Some(ev) = scanner.feed_byte(b) {
            events.push(ev);
        }
    }
    assert_eq!(
        events,
        vec![ScannerEvent::DeviceStatus, ScannerEvent::CursorPosition]
    );

    // >16 CSI param overflow sets csi_ignore and drops result
    let overflow_csi = b"\x1b[1;2;3;4;5;6;7;8;9;10;11;12;13;14;15;16;17n\x1b[5n";
    events.clear();
    for &b in overflow_csi {
        if let Some(ev) = scanner.feed_byte(b) {
            events.push(ev);
        }
    }
    assert_eq!(events, vec![ScannerEvent::DeviceStatus]);

    // DcsString + DcsEsc + C1 ST + CAN/SUB + OscEsc non-ST
    let dcs_osc = b"\x1bPqdata\x1b\\\x1bXpm\x9c\x1b^abc\x18\x1b_xyz\x1a\x1bPq\x1b[5n\x1b]7;file:///tmp\x18\x1b]7;file:///tmp\x1b[6n";
    events.clear();
    for &b in dcs_osc {
        if let Some(ev) = scanner.feed_byte(b) {
            events.push(ev);
        }
    }
    assert_eq!(
        events,
        vec![ScannerEvent::DeviceStatus, ScannerEvent::CursorPosition]
    );

    // Multi-byte UTF-8 and invalid continuation recovery
    assert_eq!(scanner.feed_byte(0xc3), None);
    assert!(scanner.will_print_byte(0xa9));
    assert_eq!(scanner.feed_byte(0xa9), None);

    assert_eq!(scanner.feed_byte(0xe2), None);
    assert!(!scanner.will_print_byte(0x82));
    assert_eq!(scanner.feed_byte(0x82), None);
    assert!(scanner.will_print_byte(0xac));
    assert_eq!(scanner.feed_byte(0xac), None);

    // Interrupted 4-byte sequence followed by ASCII and ESC
    assert_eq!(scanner.feed_byte(0xf0), None);
    assert!(scanner.will_print_byte(b'X'));
    assert_eq!(scanner.feed_byte(b'X'), None);
    assert_eq!(scanner.feed_byte(0x1b), None);
    assert_eq!(scanner.feed_byte(b'c'), Some(ScannerEvent::Reset));

    // Invalid hex nibble in OSC 7 percent-encoding stays literal
    assert_eq!(
        parse_osc7_cwd(b"file:///tmp/100%ZZok%"),
        Some("/tmp/100%ZZok%".to_string())
    );
}

#[test]
fn test_pty_state_resize_clamping_eviction_and_cols_one_wrap() {
    let mut state = PtyState::new(10, 20);
    state.set_scroll_region(3, 8);
    assert!(state.scroll_region_active());

    // Invalid region (top >= bottom) resets scroll region
    state.set_scroll_region(5, 3);
    assert!(!state.scroll_region_active());

    // Bottom=0 means max_row; resizing shrinks scroll_bottom and resets scroll_top when inverted
    state.set_scroll_region(4, 0);
    assert!(state.scroll_region_active());
    state.resize(6, 20);
    assert!(state.scroll_region_active());
    state.resize(2, 20);
    assert!(!state.scroll_region_active());

    // Eviction past 1000 scrollback rows
    for i in 0..1050usize {
        state.push_scrollback_row(vec![PtyCell {
            contents: i.to_string(),
            ..PtyCell::default()
        }]);
    }
    assert_eq!(state.scrollback.len(), 1000);
    assert_eq!(state.scrollback.front().expect("front")[0].contents, "50");
    assert_eq!(state.scrollback.back().expect("back")[0].contents, "1049");

    // cols == 1 wrap scroll and cols == 4 / cols == 3 wide-char wrap
    let mut scanner = TerminalScanner::default();
    let mut narrow = PtyState::new(3, 1);
    let mut responses = Vec::new();
    process_pty_bytes(b"\x1b[3;1Habcd", &mut scanner, &mut narrow, &mut responses);
    assert_eq!(narrow.scrollback.len(), 3);

    let mut four_col = PtyState::new(3, 4);
    process_pty_bytes(b"\x1b[3;1H123\xf0\x9f\x93\xa6", &mut scanner, &mut four_col, &mut responses);
    assert_eq!(four_col.scrollback.len(), 1);

    let mut three_col = PtyState::new(3, 3);
    process_pty_bytes(b"\x1b[3;1H12\xf0\x9f\x93\xa6", &mut scanner, &mut three_col, &mut responses);
    assert!(three_col.scrollback.is_empty());
    process_pty_bytes(b"XY", &mut scanner, &mut three_col, &mut responses);
    assert_eq!(three_col.scrollback.len(), 1);
}
