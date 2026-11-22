use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

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
            if ctrl {
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
        assert_eq!(
            encode_paste("echo hi\nls", true),
            b"\x1b[200~echo hi\nls\x1b[201~"
        );
    }
}
