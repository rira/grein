use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::engine_port::{PtyMouseEncoding, PtyMouseMode};

fn xterm_modifier(modifiers: KeyModifiers) -> u8 {
    let mut mask = 0u8;
    if modifiers.contains(KeyModifiers::SHIFT) {
        mask |= 1;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        mask |= 2;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        mask |= 4;
    }
    if mask == 0 {
        1
    } else {
        1 + mask
    }
}

fn ctrl_byte(ch: char) -> Option<u8> {
    match ch {
        ' ' | '@' | '2' => Some(0x00),
        'a'..='z' => Some((ch as u8) - b'a' + 1),
        'A'..='Z' => Some((ch as u8) - b'A' + 1),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' | '~' => Some(0x1e),
        '_' | '/' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

fn encode_csi_or_ss3(letter: char, m: u8, application_cursor: bool) -> Vec<u8> {
    if m == 1 {
        if application_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    } else {
        format!("\x1b[1;{m}{letter}").into_bytes()
    }
}

fn encode_tilde(code: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        format!("\x1b[{code}~").into_bytes()
    } else {
        format!("\x1b[{code};{m}~").into_bytes()
    }
}

pub fn encode_key_event(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    if key.kind == KeyEventKind::Release {
        return None;
    }

    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let m = xterm_modifier(key.modifiers);

    match key.code {
        KeyCode::Char(c) => {
            if ctrl && alt && !c.is_control() && !c.is_ascii_alphabetic() {
                let mut buf = [0u8; 4];
                Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
            } else if ctrl {
                let b = ctrl_byte(c)?;
                if alt {
                    Some(vec![0x1b, b])
                } else {
                    Some(vec![b])
                }
            } else {
                let mut out = Vec::with_capacity(5);
                if alt {
                    out.push(0x1b);
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                Some(out)
            }
        }
        KeyCode::Enter => Some(if alt { vec![0x1b, b'\r'] } else { vec![b'\r'] }),
        KeyCode::Tab => {
            if shift {
                Some(b"\x1b[Z".to_vec())
            } else if alt {
                Some(b"\x1b\t".to_vec())
            } else {
                Some(b"\t".to_vec())
            }
        }
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Backspace => {
            let b = if ctrl { 0x08 } else { 0x7f };
            Some(if alt { vec![0x1b, b] } else { vec![b] })
        }
        KeyCode::Esc => Some(if alt {
            vec![0x1b, 0x1b]
        } else {
            vec![0x1b]
        }),
        KeyCode::Up => Some(encode_csi_or_ss3('A', m, application_cursor)),
        KeyCode::Down => Some(encode_csi_or_ss3('B', m, application_cursor)),
        KeyCode::Right => Some(encode_csi_or_ss3('C', m, application_cursor)),
        KeyCode::Left => Some(encode_csi_or_ss3('D', m, application_cursor)),
        KeyCode::Home => Some(encode_csi_or_ss3('H', m, application_cursor)),
        KeyCode::End => Some(encode_csi_or_ss3('F', m, application_cursor)),
        KeyCode::Insert => Some(encode_tilde(2, m)),
        KeyCode::Delete => Some(encode_tilde(3, m)),
        KeyCode::PageUp => Some(encode_tilde(5, m)),
        KeyCode::PageDown => Some(encode_tilde(6, m)),
        KeyCode::F(n @ 1..=4) => {
            let letter = match n {
                1 => 'P',
                2 => 'Q',
                3 => 'R',
                4 => 'S',
                _ => unreachable!(),
            };
            Some(encode_csi_or_ss3(letter, m, true))
        }
        KeyCode::F(n @ 5..=12) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => unreachable!(),
            };
            Some(encode_tilde(code, m))
        }
        _ => None,
    }
}

pub fn encode_paste(text: &str, bracketed_paste: bool) -> Vec<u8> {
    if bracketed_paste {
        let sanitized = text.replace("\x1b[201~", "");
        let mut out = Vec::with_capacity(sanitized.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(sanitized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

fn mouse_button_code(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn mouse_modifier_bits(modifiers: KeyModifiers) -> u8 {
    let mut bits = 0u8;
    if modifiers.contains(KeyModifiers::SHIFT) {
        bits |= 4;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        bits |= 8;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        bits |= 16;
    }
    bits
}

pub fn encode_mouse_event(
    event: MouseEvent,
    local_col: u16,
    local_row: u16,
    mode: PtyMouseMode,
    encoding: PtyMouseEncoding,
) -> Option<Vec<u8>> {
    if mode == PtyMouseMode::None {
        return None;
    }

    let (base_code, is_release) = match (mode, event.kind) {
        (_, MouseEventKind::Down(button)) => (mouse_button_code(button), false),
        (_, MouseEventKind::ScrollUp) => (64, false),
        (_, MouseEventKind::ScrollDown) => (65, false),
        (
            PtyMouseMode::PressRelease | PtyMouseMode::ButtonMotion | PtyMouseMode::AnyMotion,
            MouseEventKind::Up(button),
        ) => {
            let code = if encoding == PtyMouseEncoding::Sgr {
                mouse_button_code(button)
            } else {
                3
            };
            (code, true)
        }
        (PtyMouseMode::ButtonMotion | PtyMouseMode::AnyMotion, MouseEventKind::Drag(button)) => {
            (mouse_button_code(button) + 32, false)
        }
        (PtyMouseMode::AnyMotion, MouseEventKind::Moved) => (35, false),
        _ => return None,
    };

    let cb = base_code | mouse_modifier_bits(event.modifiers);
    match encoding {
        PtyMouseEncoding::Sgr => {
            let suffix = if is_release { 'm' } else { 'M' };
            let col = u32::from(local_col) + 1;
            let row = u32::from(local_row) + 1;
            Some(format!("\x1b[<{cb};{col};{row}{suffix}").into_bytes())
        }
        PtyMouseEncoding::Default => {
            let cx = u8::try_from(local_col.min(222)).unwrap_or(222) + 33;
            let cy = u8::try_from(local_row.min(222)).unwrap_or(222) + 33;
            Some(vec![0x1b, b'[', b'M', cb.saturating_add(32), cx, cy])
        }
        PtyMouseEncoding::Utf8 => {
            let mut out = Vec::with_capacity(9);
            out.extend_from_slice(b"\x1b[M");
            let mut buf = [0u8; 4];
            for val in [
                u32::from(cb) + 32,
                u32::from(local_col) + 33,
                u32::from(local_row) + 33,
            ] {
                let ch = char::from_u32(val)?;
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn encodes_printable_ctrl_alt_and_basic_keys() {
        assert_eq!(
            encode_key_event(key(KeyCode::Char('a'), KeyModifiers::NONE), false),
            Some(b"a".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('Å'), KeyModifiers::SHIFT), false),
            Some("Å".as_bytes().to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            Some(vec![0x03])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('C'), KeyModifiers::CONTROL), false),
            Some(vec![0x03])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Enter, KeyModifiers::NONE), false),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Tab, KeyModifiers::NONE), false),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Tab, KeyModifiers::SHIFT), false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Backspace, KeyModifiers::NONE), false),
            Some(vec![0x7f])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Backspace, KeyModifiers::ALT), false),
            Some(b"\x1b\x7f".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Esc, KeyModifiers::NONE), false),
            Some(b"\x1b".to_vec())
        );
    }

    #[test]
    fn encodes_arrows_cursor_modes_and_xterm_modifiers() {
        assert_eq!(
            encode_key_event(key(KeyCode::Up, KeyModifiers::NONE), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Down, KeyModifiers::NONE), false),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Right, KeyModifiers::NONE), false),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Left, KeyModifiers::NONE), false),
            Some(b"\x1b[D".to_vec())
        );

        assert_eq!(
            encode_key_event(key(KeyCode::Up, KeyModifiers::NONE), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Down, KeyModifiers::NONE), true),
            Some(b"\x1bOB".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Right, KeyModifiers::NONE), true),
            Some(b"\x1bOC".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Left, KeyModifiers::NONE), true),
            Some(b"\x1bOD".to_vec())
        );

        assert_eq!(
            encode_key_event(key(KeyCode::Up, KeyModifiers::CONTROL), false),
            Some(b"\x1b[1;5A".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Up, KeyModifiers::CONTROL), true),
            Some(b"\x1b[1;5A".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Down, KeyModifiers::SHIFT), false),
            Some(b"\x1b[1;2B".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Left, KeyModifiers::ALT), false),
            Some(b"\x1b[1;3D".to_vec())
        );
        assert_eq!(
            encode_key_event(
                key(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                false
            ),
            Some(b"\x1b[1;6C".to_vec())
        );
    }

    #[test]
    fn encodes_navigation_function_keys_and_ignores_release() {
        assert_eq!(
            encode_key_event(key(KeyCode::Home, KeyModifiers::NONE), false),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Home, KeyModifiers::NONE), true),
            Some(b"\x1bOH".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::End, KeyModifiers::NONE), false),
            Some(b"\x1b[F".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::End, KeyModifiers::NONE), true),
            Some(b"\x1bOF".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Insert, KeyModifiers::NONE), false),
            Some(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Delete, KeyModifiers::NONE), false),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::PageUp, KeyModifiers::NONE), false),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::PageDown, KeyModifiers::NONE), false),
            Some(b"\x1b[6~".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Delete, KeyModifiers::CONTROL), false),
            Some(b"\x1b[3;5~".to_vec())
        );

        let f_expected = [
            (1, b"\x1bOP".as_slice()),
            (2, b"\x1bOQ".as_slice()),
            (3, b"\x1bOR".as_slice()),
            (4, b"\x1bOS".as_slice()),
            (5, b"\x1b[15~".as_slice()),
            (6, b"\x1b[17~".as_slice()),
            (7, b"\x1b[18~".as_slice()),
            (8, b"\x1b[19~".as_slice()),
            (9, b"\x1b[20~".as_slice()),
            (10, b"\x1b[21~".as_slice()),
            (11, b"\x1b[23~".as_slice()),
            (12, b"\x1b[24~".as_slice()),
        ];
        for (n, expected) in f_expected {
            assert_eq!(
                encode_key_event(key(KeyCode::F(n), KeyModifiers::NONE), false),
                Some(expected.to_vec()),
                "F{n}"
            );
        }
        assert_eq!(
            encode_key_event(key(KeyCode::F(1), KeyModifiers::CONTROL), false),
            Some(b"\x1b[1;5P".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::F(12), KeyModifiers::SHIFT), false),
            Some(b"\x1b[24;2~".to_vec())
        );

        let release = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert_eq!(encode_key_event(release, false), None);
    }

    #[test]
    fn encodes_bracketed_and_raw_paste() {
        assert_eq!(encode_paste("echo hi", false), b"echo hi");
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(encode_paste("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(
            encode_paste("echo hi\nls", true),
            b"\x1b[200~echo hi\nls\x1b[201~"
        );
        assert_eq!(
            encode_paste("safe\x1b[201~injected\x1b[201~end", true),
            b"\x1b[200~safeinjectedend\x1b[201~"
        );
    }

    #[test]
    fn encodes_swedish_altgr_characters_as_utf8() {
        let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
        let altgr_shift = altgr | KeyModifiers::SHIFT;
        for ch in ['@', '£', '$', '{', '}', '[', ']', '\\', '|', '~', '€', 'µ'] {
            let expected = Some(ch.to_string().into_bytes());
            assert_eq!(
                encode_key_event(key(KeyCode::Char(ch), altgr), false),
                expected,
                "AltGr char {ch:?}"
            );
            assert_eq!(
                encode_key_event(key(KeyCode::Char(ch), altgr_shift), false),
                expected,
                "AltGr+Shift char {ch:?}"
            );
        }
    }

    #[test]
    fn encodes_ctrl_punctuation_ctrl_alt_alpha_alt_specials_and_edge_cases() {
        let ctrl_punct = [
            (' ', 0x00),
            ('@', 0x00),
            ('2', 0x00),
            ('[', 0x1b),
            ('3', 0x1b),
            ('\\', 0x1c),
            ('4', 0x1c),
            (']', 0x1d),
            ('5', 0x1d),
            ('^', 0x1e),
            ('6', 0x1e),
            ('~', 0x1e),
            ('_', 0x1f),
            ('/', 0x1f),
            ('7', 0x1f),
            ('?', 0x7f),
            ('8', 0x7f),
        ];
        for (ch, byte) in ctrl_punct {
            assert_eq!(
                encode_key_event(key(KeyCode::Char(ch), KeyModifiers::CONTROL), false),
                Some(vec![byte]),
                "Ctrl+{ch:?}"
            );
        }
        assert_eq!(
            encode_key_event(key(KeyCode::Char('{'), KeyModifiers::CONTROL), false),
            None
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('€'), KeyModifiers::CONTROL), false),
            None
        );

        let ctrl_alt = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(
            encode_key_event(key(KeyCode::Char('a'), ctrl_alt), false),
            Some(vec![0x1b, 0x01])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('Z'), ctrl_alt), false),
            Some(vec![0x1b, 0x1a])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('\x03'), ctrl_alt), false),
            None
        );

        assert_eq!(
            encode_key_event(key(KeyCode::Enter, KeyModifiers::ALT), false),
            Some(vec![0x1b, b'\r'])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Tab, KeyModifiers::ALT), false),
            Some(b"\x1b\t".to_vec())
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Esc, KeyModifiers::ALT), false),
            Some(vec![0x1b, 0x1b])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Backspace, KeyModifiers::CONTROL), false),
            Some(vec![0x08])
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Backspace, ctrl_alt), false),
            Some(vec![0x1b, 0x08])
        );

        assert_eq!(
            encode_key_event(key(KeyCode::F(13), KeyModifiers::NONE), false),
            None
        );
        assert_eq!(
            encode_key_event(key(KeyCode::F(24), KeyModifiers::NONE), false),
            None
        );
        assert_eq!(
            encode_key_event(key(KeyCode::CapsLock, KeyModifiers::NONE), false),
            None
        );

        let repeat = KeyEvent {
            code: KeyCode::Up,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        };
        assert_eq!(encode_key_event(repeat, false), Some(b"\x1b[A".to_vec()));
        let repeat_altgr = KeyEvent {
            code: KeyCode::Char('{'),
            modifiers: ctrl_alt,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        };
        assert_eq!(encode_key_event(repeat_altgr, false), Some(b"{".to_vec()));
    }

    fn mouse(kind: MouseEventKind, modifiers: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column: 99,
            row: 88,
            modifiers,
        }
    }

    #[test]
    fn encodes_sgr_mouse_press_release_drag_wheel_moved_and_modifiers() {
        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE),
                0,
                0,
                PtyMouseMode::Press,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
        assert_eq!(
            encode_mouse_event(
                mouse(
                    MouseEventKind::Down(MouseButton::Middle),
                    KeyModifiers::NONE
                ),
                9,
                4,
                PtyMouseMode::PressRelease,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<1;10;5M".to_vec())
        );
        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::Down(MouseButton::Right), KeyModifiers::NONE),
                299,
                249,
                PtyMouseMode::PressRelease,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<2;300;250M".to_vec())
        );
        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::Up(MouseButton::Right), KeyModifiers::SHIFT),
                5,
                6,
                PtyMouseMode::PressRelease,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<6;6;7m".to_vec())
        );
        assert_eq!(
            encode_mouse_event(
                mouse(
                    MouseEventKind::Drag(MouseButton::Left),
                    KeyModifiers::SHIFT | KeyModifiers::ALT
                ),
                10,
                20,
                PtyMouseMode::ButtonMotion,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<44;11;21M".to_vec())
        );
        assert_eq!(
            encode_mouse_event(
                mouse(
                    MouseEventKind::Moved,
                    KeyModifiers::ALT | KeyModifiers::CONTROL
                ),
                2,
                3,
                PtyMouseMode::AnyMotion,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<59;3;4M".to_vec())
        );
        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::ScrollUp, KeyModifiers::CONTROL),
                1,
                2,
                PtyMouseMode::Press,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<80;2;3M".to_vec())
        );
        assert_eq!(
            encode_mouse_event(
                mouse(
                    MouseEventKind::ScrollDown,
                    KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL
                ),
                1,
                2,
                PtyMouseMode::PressRelease,
                PtyMouseEncoding::Sgr,
            ),
            Some(b"\x1b[<93;2;3M".to_vec())
        );
    }

    #[test]
    fn encodes_x10_default_and_utf8_mouse_encodings() {
        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE),
                0,
                0,
                PtyMouseMode::Press,
                PtyMouseEncoding::Default,
            ),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::Up(MouseButton::Right), KeyModifiers::NONE),
                5,
                6,
                PtyMouseMode::PressRelease,
                PtyMouseEncoding::Default,
            ),
            Some(vec![0x1b, b'[', b'M', 35, 38, 39])
        );
        assert_eq!(
            encode_mouse_event(
                mouse(
                    MouseEventKind::Down(MouseButton::Middle),
                    KeyModifiers::NONE
                ),
                222,
                300,
                PtyMouseMode::Press,
                PtyMouseEncoding::Default,
            ),
            Some(vec![0x1b, b'[', b'M', 33, 255, 255])
        );

        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::Up(MouseButton::Left), KeyModifiers::NONE),
                0,
                0,
                PtyMouseMode::PressRelease,
                PtyMouseEncoding::Utf8,
            ),
            Some(vec![0x1b, b'[', b'M', 35, 33, 33])
        );
        let mut expected_utf8 = b"\x1b[M ".to_vec();
        expected_utf8.extend_from_slice('\u{0085}'.encode_utf8(&mut [0; 4]).as_bytes());
        expected_utf8.extend_from_slice('\u{00e9}'.encode_utf8(&mut [0; 4]).as_bytes());
        assert_eq!(
            encode_mouse_event(
                mouse(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE),
                100,
                200,
                PtyMouseMode::Press,
                PtyMouseEncoding::Utf8,
            ),
            Some(expected_utf8)
        );
    }

    #[test]
    fn filters_mouse_events_by_protocol_mode() {
        let down = mouse(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE);
        let up = mouse(MouseEventKind::Up(MouseButton::Left), KeyModifiers::NONE);
        let drag = mouse(MouseEventKind::Drag(MouseButton::Left), KeyModifiers::NONE);
        let moved = mouse(MouseEventKind::Moved, KeyModifiers::NONE);
        let wheel_up = mouse(MouseEventKind::ScrollUp, KeyModifiers::NONE);
        let wheel_down = mouse(MouseEventKind::ScrollDown, KeyModifiers::NONE);
        let wheel_left = mouse(MouseEventKind::ScrollLeft, KeyModifiers::NONE);

        for ev in [down, up, drag, moved, wheel_up, wheel_down, wheel_left] {
            assert_eq!(
                encode_mouse_event(ev, 0, 0, PtyMouseMode::None, PtyMouseEncoding::Sgr),
                None
            );
        }

        assert!(encode_mouse_event(down, 0, 0, PtyMouseMode::Press, PtyMouseEncoding::Sgr).is_some());
        assert!(
            encode_mouse_event(wheel_up, 0, 0, PtyMouseMode::Press, PtyMouseEncoding::Sgr).is_some()
        );
        assert!(
            encode_mouse_event(wheel_down, 0, 0, PtyMouseMode::Press, PtyMouseEncoding::Sgr)
                .is_some()
        );
        assert_eq!(
            encode_mouse_event(up, 0, 0, PtyMouseMode::Press, PtyMouseEncoding::Sgr),
            None
        );
        assert_eq!(
            encode_mouse_event(drag, 0, 0, PtyMouseMode::Press, PtyMouseEncoding::Sgr),
            None
        );
        assert_eq!(
            encode_mouse_event(moved, 0, 0, PtyMouseMode::Press, PtyMouseEncoding::Sgr),
            None
        );

        assert!(
            encode_mouse_event(up, 0, 0, PtyMouseMode::PressRelease, PtyMouseEncoding::Sgr)
                .is_some()
        );
        assert_eq!(
            encode_mouse_event(drag, 0, 0, PtyMouseMode::PressRelease, PtyMouseEncoding::Sgr),
            None
        );
        assert_eq!(
            encode_mouse_event(
                moved,
                0,
                0,
                PtyMouseMode::PressRelease,
                PtyMouseEncoding::Sgr
            ),
            None
        );

        assert!(
            encode_mouse_event(drag, 0, 0, PtyMouseMode::ButtonMotion, PtyMouseEncoding::Sgr)
                .is_some()
        );
        assert_eq!(
            encode_mouse_event(
                moved,
                0,
                0,
                PtyMouseMode::ButtonMotion,
                PtyMouseEncoding::Sgr
            ),
            None
        );

        assert!(
            encode_mouse_event(moved, 0, 0, PtyMouseMode::AnyMotion, PtyMouseEncoding::Sgr)
                .is_some()
        );
        assert_eq!(
            encode_mouse_event(
                wheel_left,
                0,
                0,
                PtyMouseMode::AnyMotion,
                PtyMouseEncoding::Sgr
            ),
            None
        );
    }
}
