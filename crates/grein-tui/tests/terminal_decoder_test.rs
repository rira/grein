use std::io::Write;
use std::os::unix::io::FromRawFd;
use std::time::Duration;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use grein_tui::terminal_decoder::{
    extract_kitty_probe_replies, DecodedInput, TerminalInputDecoder, UnixRawInputReader,
};

fn decode_all(bytes: &[u8]) -> Vec<DecodedInput> {
    let mut decoder = TerminalInputDecoder::new();
    let mut events = decoder.feed(bytes);
    if let Some(ev) = decoder.flush_pending_esc() {
        events.push(ev);
    }
    events
}

fn decode_one_key(bytes: &[u8]) -> KeyEvent {
    let events = decode_all(bytes);
    assert_eq!(events.len(), 1, "expected 1 event for {bytes:?}, got {events:?}");
    assert_eq!(events[0].raw_bytes, bytes);
    match events[0].event {
        Event::Key(key) => key,
        ref other => panic!("expected Key event, got {other:?}"),
    }
}

#[test]
fn decodes_modify_other_keys_format_2() {
    let shift_enter = decode_one_key(b"\x1b[27;2;13~");
    assert_eq!(
        shift_enter,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
    );

    let ctrl_enter = decode_one_key(b"\x1b[27;5;13~");
    assert_eq!(
        ctrl_enter,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)
    );

    let ctrl_shift_a = decode_one_key(b"\x1b[27;6;97~");
    assert_eq!(
        ctrl_shift_a,
        KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )
    );

    let alt_tab = decode_one_key(b"\x1b[27;3;9~");
    assert_eq!(alt_tab, KeyEvent::new(KeyCode::Tab, KeyModifiers::ALT));
}

#[test]
fn decodes_rxvt_shift_and_ctrl_arrows() {
    let cases = [
        (b"\x1b[a".as_slice(), KeyCode::Up, KeyModifiers::SHIFT),
        (b"\x1b[b".as_slice(), KeyCode::Down, KeyModifiers::SHIFT),
        (b"\x1b[c".as_slice(), KeyCode::Right, KeyModifiers::SHIFT),
        (b"\x1b[d".as_slice(), KeyCode::Left, KeyModifiers::SHIFT),
        (b"\x1bOa".as_slice(), KeyCode::Up, KeyModifiers::CONTROL),
        (b"\x1bOb".as_slice(), KeyCode::Down, KeyModifiers::CONTROL),
        (b"\x1bOc".as_slice(), KeyCode::Right, KeyModifiers::CONTROL),
        (b"\x1bOd".as_slice(), KeyCode::Left, KeyModifiers::CONTROL),
    ];
    for (bytes, code, mods) in cases {
        assert_eq!(decode_one_key(bytes), KeyEvent::new(code, mods));
    }
}

#[test]
fn decodes_standard_and_modified_ss3_sequences() {
    let cases = [
        (b"\x1bOA".as_slice(), KeyCode::Up, KeyModifiers::NONE),
        (b"\x1bOB".as_slice(), KeyCode::Down, KeyModifiers::NONE),
        (b"\x1bOC".as_slice(), KeyCode::Right, KeyModifiers::NONE),
        (b"\x1bOD".as_slice(), KeyCode::Left, KeyModifiers::NONE),
        (b"\x1bOH".as_slice(), KeyCode::Home, KeyModifiers::NONE),
        (b"\x1bOF".as_slice(), KeyCode::End, KeyModifiers::NONE),
        (b"\x1bOP".as_slice(), KeyCode::F(1), KeyModifiers::NONE),
        (b"\x1bOQ".as_slice(), KeyCode::F(2), KeyModifiers::NONE),
        (b"\x1bOR".as_slice(), KeyCode::F(3), KeyModifiers::NONE),
        (b"\x1bOS".as_slice(), KeyCode::F(4), KeyModifiers::NONE),
        (b"\x1bO2A".as_slice(), KeyCode::Up, KeyModifiers::SHIFT),
        (b"\x1bO2B".as_slice(), KeyCode::Down, KeyModifiers::SHIFT),
        (b"\x1bO1;2C".as_slice(), KeyCode::Right, KeyModifiers::SHIFT),
        (b"\x1bO1;5D".as_slice(), KeyCode::Left, KeyModifiers::CONTROL),
    ];
    for (bytes, code, mods) in cases {
        assert_eq!(decode_one_key(bytes), KeyEvent::new(code, mods));
    }
}

#[test]
fn decodes_leading_semicolon_and_standard_csi_keys() {
    let cases = [
        (b"\x1b[;2A".as_slice(), KeyCode::Up, KeyModifiers::SHIFT),
        (b"\x1b[;2B".as_slice(), KeyCode::Down, KeyModifiers::SHIFT),
        (b"\x1b[;2C".as_slice(), KeyCode::Right, KeyModifiers::SHIFT),
        (b"\x1b[;2D".as_slice(), KeyCode::Left, KeyModifiers::SHIFT),
        (b"\x1b[1;2A".as_slice(), KeyCode::Up, KeyModifiers::SHIFT),
        (b"\x1b[2A".as_slice(), KeyCode::Up, KeyModifiers::SHIFT),
        (b"\x1b[1;5D".as_slice(), KeyCode::Left, KeyModifiers::CONTROL),
        (b"\x1b[H".as_slice(), KeyCode::Home, KeyModifiers::NONE),
        (b"\x1b[F".as_slice(), KeyCode::End, KeyModifiers::NONE),
        (b"\x1b[5~".as_slice(), KeyCode::PageUp, KeyModifiers::NONE),
        (b"\x1b[6~".as_slice(), KeyCode::PageDown, KeyModifiers::NONE),
        (b"\x1b[3;2~".as_slice(), KeyCode::Delete, KeyModifiers::SHIFT),
        (b"\x1b[2~".as_slice(), KeyCode::Insert, KeyModifiers::NONE),
        (b"\x1b[15~".as_slice(), KeyCode::F(5), KeyModifiers::NONE),
        (b"\x1b[24;5~".as_slice(), KeyCode::F(12), KeyModifiers::CONTROL),
        (b"\x1b[Z".as_slice(), KeyCode::BackTab, KeyModifiers::SHIFT),
    ];
    for (bytes, code, mods) in cases {
        assert_eq!(decode_one_key(bytes), KeyEvent::new(code, mods));
    }

    let repeat_up = decode_one_key(b"\x1b[1;2:2A");
    assert_eq!(repeat_up.code, KeyCode::Up);
    assert_eq!(repeat_up.modifiers, KeyModifiers::SHIFT);
    assert_eq!(repeat_up.kind, KeyEventKind::Repeat);
}

#[test]
fn decodes_split_packet_esc_streaming_byte_by_byte() {
    let mut decoder = TerminalInputDecoder::new();

    // Feed \x1b[1;2A one byte at a time; must never emit premature Esc
    let seq = b"\x1b[1;2A";
    for &b in &seq[..seq.len() - 1] {
        let out = decoder.feed(&[b]);
        assert!(out.is_empty(), "premature event on byte {b:#x}: {out:?}");
    }
    let out = decoder.feed(&[seq[seq.len() - 1]]);
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0].event,
        Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT))
    );
    assert_eq!(out[0].raw_bytes, seq);
    assert!(!decoder.has_pending_esc());

    // Lone ESC stays pending until flush_pending_esc() is called on idle timeout
    assert!(decoder.feed(b"\x1b").is_empty());
    assert!(decoder.has_pending_esc());
    let flushed = decoder.flush_pending_esc().expect("pending Esc must flush");
    assert_eq!(
        flushed.event,
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    );
    assert_eq!(flushed.raw_bytes, b"\x1b");
    assert!(!decoder.has_pending_esc());
    assert!(decoder.flush_pending_esc().is_none());
}

#[test]
fn decodes_kitty_keyboard_protocol_with_press_repeat_release() {
    let press = decode_one_key(b"\x1b[13;2u");
    assert_eq!(press.code, KeyCode::Enter);
    assert_eq!(press.modifiers, KeyModifiers::SHIFT);
    assert_eq!(press.kind, KeyEventKind::Press);

    let repeat = decode_one_key(b"\x1b[97;5:2u");
    assert_eq!(repeat.code, KeyCode::Char('a'));
    assert_eq!(repeat.modifiers, KeyModifiers::CONTROL);
    assert_eq!(repeat.kind, KeyEventKind::Repeat);

    let release = decode_one_key(b"\x1b[97;5:3u");
    assert_eq!(release.code, KeyCode::Char('a'));
    assert_eq!(release.modifiers, KeyModifiers::CONTROL);
    assert_eq!(release.kind, KeyEventKind::Release);

    let kitty_f5 = decode_one_key(b"\x1b[57368;2u");
    assert_eq!(kitty_f5.code, KeyCode::F(5));
    assert_eq!(kitty_f5.modifiers, KeyModifiers::SHIFT);

    let kitty_media = decode_one_key(b"\x1b[57430u");
    assert_eq!(kitty_media.code, KeyCode::Media(MediaKeyCode::PlayPause));

    let kitty_mod = decode_one_key(b"\x1b[57441;2:3u");
    assert_eq!(
        kitty_mod.code,
        KeyCode::Modifier(ModifierKeyCode::LeftShift)
    );
    assert_eq!(kitty_mod.kind, KeyEventKind::Release);

    let shifted_cp = decode_one_key(b"\x1b[97:65;2u");
    assert_eq!(shifted_cp.code, KeyCode::Char('A'));
    assert_eq!(shifted_cp.modifiers, KeyModifiers::SHIFT);
    assert_eq!(shifted_cp.state, KeyEventState::NONE);
}

#[test]
fn decodes_sgr_and_x10_mouse_paste_focus_utf8_and_ctrl_alt() {
    // SGR mouse press & release (1-based -> 0-based)
    let events = decode_all(b"\x1b[<0;12;5M\x1b[<0;12;5m\x1b[<64;3;4M");
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].event,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 11,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
    );
    assert_eq!(events[0].raw_bytes, b"\x1b[<0;12;5M");
    assert_eq!(
        events[1].event,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 11,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
    );
    assert_eq!(
        events[2].event,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        })
    );

    // X10 mouse with raw bytes
    let x10 = decode_all(&[0x1b, b'[', b'M', 32 + 4, 33 + 10, 33 + 7]);
    assert_eq!(x10.len(), 1);
    assert_eq!(
        x10[0].event,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 7,
            modifiers: KeyModifiers::SHIFT,
        })
    );

    // Bracketed paste split across chunks
    let mut decoder = TerminalInputDecoder::new();
    assert!(decoder.feed(b"\x1b[200~hello ").is_empty());
    assert!(decoder.feed(b"world\x1b[20").is_empty());
    let pasted = decoder.feed(b"1~");
    assert_eq!(pasted.len(), 1);
    assert_eq!(pasted[0].event, Event::Paste("hello world".to_string()));
    assert_eq!(pasted[0].raw_bytes, b"\x1b[200~hello world\x1b[201~");

    // Focus In / Out
    let focus = decode_all(b"\x1b[I\x1b[O");
    assert_eq!(focus[0].event, Event::FocusGained);
    assert_eq!(focus[1].event, Event::FocusLost);

    // UTF-8 multi-byte split + Alt+UTF-8 + Ctrl+char + Alt+char
    let mut utf8_dec = TerminalInputDecoder::new();
    assert!(utf8_dec.feed(&[0xf0, 0x9f]).is_empty());
    let crab = utf8_dec.feed(&[0xa6, 0x80]);
    assert_eq!(
        crab[0].event,
        Event::Key(KeyEvent::new(KeyCode::Char('🦀'), KeyModifiers::NONE))
    );
    assert_eq!(
        decode_one_key("\x1bå".as_bytes()),
        KeyEvent::new(KeyCode::Char('å'), KeyModifiers::ALT)
    );
    assert_eq!(
        decode_one_key(b"\x03"),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    );
    assert_eq!(
        decode_one_key(b"\x1bx"),
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)
    );
}

#[test]
fn consumes_unknown_csi_da1_osc_dcs_without_wedging() {
    let stream = b"\x1b[?1u\x1b[?62;22c\x1b[>0;276;0c\x1b]11;rgb:0000/0000/0000\x07\x1bP>|tmux 3.5\x1b\\\x1b[1;2A";
    let events = decode_all(stream);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event,
        Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT))
    );
    assert_eq!(events[0].raw_bytes, b"\x1b[1;2A");
}

#[test]
fn extracts_kitty_probe_replies_and_preserves_early_keystrokes() {
    let (flags, da1, buffered) =
        extract_kitty_probe_replies(b"ab\x1b[?1u\x1b[A\x1b[?62;22cxy");
    assert_eq!(flags, Some(1));
    assert!(da1);
    assert_eq!(buffered, b"ab\x1b[Axy");

    let (no_flags, da1_only, buffered2) = extract_kitty_probe_replies(b"\x1b[?1;2c\x1b[1;2B");
    assert_eq!(no_flags, None);
    assert!(da1_only);
    assert_eq!(buffered2, b"\x1b[1;2B");
}

#[cfg(unix)]
#[test]
fn unix_reader_reads_pipe_flushes_lone_esc_and_probes_kitty_without_losing_keys() {
    let mut fds: [libc::c_int; 2] = [0; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (read_fd, write_fd) = (fds[0], fds[1]);
    let mut writer = unsafe { std::fs::File::from_raw_fd(write_fd) };

    let mut probe_out = Vec::new();
    writer
        .write_all(b"k\x1b[?3u\x1b[?62;22c\x1b[1;2A")
        .unwrap();
    writer.flush().unwrap();

    let probe = UnixRawInputReader::probe_kitty_keyboard_io(read_fd, &mut probe_out, 100).unwrap();
    assert_eq!(probe_out, b"\x1b[?u\x1b[c");
    assert!(probe.supported);
    assert_eq!(probe.flags, Some(3));
    assert_eq!(probe.buffered_bytes, b"k\x1b[1;2A");

    let mut reader = UnixRawInputReader::with_fd(read_fd);
    reader.push_initial_bytes(&probe.buffered_bytes);

    let first = reader.read_next(Duration::from_millis(50)).unwrap().unwrap();
    assert_eq!(
        first.event,
        Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
    );
    let second = reader.read_next(Duration::from_millis(50)).unwrap().unwrap();
    assert_eq!(
        second.event,
        Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT))
    );

    // Lone ESC written to pipe flushes after ~25ms idle timeout
    writer.write_all(b"\x1b").unwrap();
    writer.flush().unwrap();
    let esc_ev = reader.read_next(Duration::from_millis(100)).unwrap().unwrap();
    assert_eq!(
        esc_ev.event,
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    );
    assert_eq!(esc_ev.raw_bytes, b"\x1b");

    unsafe {
        libc::close(read_fd);
    }
}
