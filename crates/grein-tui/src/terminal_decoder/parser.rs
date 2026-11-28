use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};

const MAX_CSI_LEN: usize = 256;
const MAX_SS3_LEN: usize = 32;
const MAX_STRING_SEQ_LEN: usize = 65_536;
const MAX_PASTE_LEN: usize = 16 * 1024 * 1024;
const PASTE_END_MARKER: &[u8] = b"\x1b[201~";

const KITTY_FUNCTIONAL_KEYS: [KeyCode; 20] = [
    KeyCode::Esc, KeyCode::Enter, KeyCode::Tab, KeyCode::Backspace, KeyCode::Insert,
    KeyCode::Delete, KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down,
    KeyCode::PageUp, KeyCode::PageDown, KeyCode::Home, KeyCode::End, KeyCode::CapsLock,
    KeyCode::ScrollLock, KeyCode::NumLock, KeyCode::PrintScreen, KeyCode::Pause, KeyCode::Menu,
];
const KITTY_KEYPAD_KEYS: [KeyCode; 19] = [
    KeyCode::Char('.'), KeyCode::Char('/'), KeyCode::Char('*'), KeyCode::Char('-'),
    KeyCode::Char('+'), KeyCode::Enter, KeyCode::Char('='), KeyCode::Char(','),
    KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down, KeyCode::PageUp,
    KeyCode::PageDown, KeyCode::Home, KeyCode::End, KeyCode::Insert, KeyCode::Delete,
    KeyCode::KeypadBegin,
];
const KITTY_MEDIA_KEYS: [MediaKeyCode; 13] = [
    MediaKeyCode::Play, MediaKeyCode::Pause, MediaKeyCode::PlayPause, MediaKeyCode::Reverse,
    MediaKeyCode::Stop, MediaKeyCode::FastForward, MediaKeyCode::Rewind,
    MediaKeyCode::TrackNext, MediaKeyCode::TrackPrevious, MediaKeyCode::Record,
    MediaKeyCode::LowerVolume, MediaKeyCode::RaiseVolume, MediaKeyCode::MuteVolume,
];
const KITTY_MODIFIER_KEYS: [ModifierKeyCode; 12] = [
    ModifierKeyCode::LeftShift, ModifierKeyCode::LeftControl, ModifierKeyCode::LeftAlt,
    ModifierKeyCode::LeftSuper, ModifierKeyCode::LeftHyper, ModifierKeyCode::LeftMeta,
    ModifierKeyCode::RightShift, ModifierKeyCode::RightControl, ModifierKeyCode::RightAlt,
    ModifierKeyCode::RightSuper, ModifierKeyCode::RightHyper, ModifierKeyCode::RightMeta,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedInput {
    pub event: Event,
    pub raw_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyProbeReply {
    KittyFlags(u8),
    PrimaryDeviceAttributes,
}

#[derive(Debug, Default)]
pub struct KittyProbeScanner {
    buf: Vec<u8>,
    pub kitty_flags: Option<u8>,
    pub da1_seen: bool,
    pub buffered_bytes: Vec<u8>,
}

impl KittyProbeScanner {
    pub fn new() -> Self { Self::default() }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        self.drain_complete();
    }

    pub fn finish(&mut self) -> Vec<u8> {
        self.drain_complete();
        if !self.buf.is_empty() { self.buffered_bytes.append(&mut self.buf); }
        std::mem::take(&mut self.buffered_bytes)
    }

    fn drain_complete(&mut self) {
        let mut cursor = 0;
        while cursor < self.buf.len() {
            let Some(rel_esc) = self.buf[cursor..].iter().position(|&b| b == 0x1b) else {
                self.buffered_bytes.extend_from_slice(&self.buf[cursor..]);
                cursor = self.buf.len();
                break;
            };
            self.buffered_bytes.extend_from_slice(&self.buf[cursor..cursor + rel_esc]);
            cursor += rel_esc;
            let rem = &self.buf[cursor..];
            if rem.len() < 2 { break; }
            if rem[1] != b'[' { self.buffered_bytes.push(rem[0]); cursor += 1; continue; }
            if rem.len() < 3 { break; }
            if rem[2] != b'?' { self.buffered_bytes.extend_from_slice(&rem[..2]); cursor += 2; continue; }
            let Some(final_off) = rem[3..].iter().position(|&b| (0x40..=0x7e).contains(&b)) else {
                if rem.len() > MAX_CSI_LEN { self.buffered_bytes.extend_from_slice(rem); cursor = self.buf.len(); }
                break;
            };
            let seq_end = 3 + final_off + 1;
            match parse_probe_csi(&rem[3..3 + final_off], rem[3 + final_off]) {
                Some(KittyProbeReply::KittyFlags(flags)) => self.kitty_flags = Some(flags),
                Some(KittyProbeReply::PrimaryDeviceAttributes) => self.da1_seen = true,
                None => self.buffered_bytes.extend_from_slice(&rem[..seq_end]),
            }
            cursor += seq_end;
        }
        if cursor > 0 { self.buf.drain(..cursor); }
    }
}

pub fn extract_kitty_probe_replies(bytes: &[u8]) -> (Option<u8>, bool, Vec<u8>) {
    let mut scanner = KittyProbeScanner::new();
    scanner.feed(bytes);
    let leftover = scanner.finish();
    (scanner.kitty_flags, scanner.da1_seen, leftover)
}

fn parse_probe_csi(params: &[u8], final_byte: u8) -> Option<KittyProbeReply> {
    let text = std::str::from_utf8(params).ok()?;
    match final_byte {
        b'u' => Some(KittyProbeReply::KittyFlags(text.parse::<u16>().ok()?.min(u16::from(u8::MAX)) as u8)),
        b'c' if text.bytes().all(|b| b.is_ascii_digit() || b == b';') => Some(KittyProbeReply::PrimaryDeviceAttributes),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Ground,
    EscPending,
    Csi,
    Ss3,
    Delimited { allow_bel: bool, saw_esc: bool },
    X10Mouse,
    BracketedPaste,
    Utf8 { expected_len: usize, alt: bool },
}

#[derive(Debug)]
pub struct TerminalInputDecoder {
    state: State,
    raw: Vec<u8>,
    utf8_buf: Vec<u8>,
    paste_buf: Vec<u8>,
}

impl Default for TerminalInputDecoder {
    fn default() -> Self { Self::new() }
}

impl TerminalInputDecoder {
    pub fn new() -> Self {
        Self { state: State::Ground, raw: Vec::with_capacity(64), utf8_buf: Vec::with_capacity(4), paste_buf: Vec::new() }
    }

    pub fn has_pending_esc(&self) -> bool { matches!(self.state, State::EscPending) }

    pub fn flush_pending_esc(&mut self) -> Option<DecodedInput> {
        if !matches!(self.state, State::EscPending) { return None; }
        let raw_bytes = std::mem::take(&mut self.raw);
        self.state = State::Ground;
        Some(DecodedInput { event: Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), raw_bytes })
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<DecodedInput> {
        let mut out = Vec::new();
        for &b in bytes { self.step(b, &mut out); }
        out
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.raw.clear();
        self.utf8_buf.clear();
        self.paste_buf.clear();
    }

    fn reenter_esc(&mut self) {
        self.raw.clear();
        self.raw.push(0x1b);
        self.state = State::EscPending;
    }

    fn emit_event(&mut self, event: Event, out: &mut Vec<DecodedInput>) {
        out.push(DecodedInput { event, raw_bytes: std::mem::take(&mut self.raw) });
        self.state = State::Ground;
    }

    fn step(&mut self, b: u8, out: &mut Vec<DecodedInput>) {
        match self.state {
            State::Ground => self.step_ground(b, out),
            State::EscPending => self.step_esc_pending(b, out),
            State::Csi => self.step_csi(b, out),
            State::Ss3 => self.step_ss3(b, out),
            State::Delimited { allow_bel, saw_esc } => self.step_delimited(b, allow_bel, saw_esc),
            State::X10Mouse => self.step_x10_mouse(b, out),
            State::BracketedPaste => self.step_bracketed_paste(b, out),
            State::Utf8 { expected_len, alt } => self.step_utf8(b, expected_len, alt, out),
        }
    }

    fn step_ground(&mut self, b: u8, out: &mut Vec<DecodedInput>) {
        self.raw.clear();
        self.raw.push(b);
        if b == 0x1b {
            self.state = State::EscPending;
        } else if b < 0x80 {
            if let Some(key) = decode_single_ascii(b, KeyModifiers::NONE) { self.emit_event(Event::Key(key), out); }
            else { self.reset(); }
        } else {
            self.begin_utf8(b, false);
        }
    }

    fn step_esc_pending(&mut self, b: u8, out: &mut Vec<DecodedInput>) {
        if b == 0x1b {
            out.push(DecodedInput { event: Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), raw_bytes: std::mem::take(&mut self.raw) });
            self.reenter_esc();
            return;
        }
        self.raw.push(b);
        match b {
            b'[' => self.state = State::Csi,
            b'O' => self.state = State::Ss3,
            b']' => self.state = State::Delimited { allow_bel: true, saw_esc: false },
            b'P' | b'^' | b'_' | b'X' => self.state = State::Delimited { allow_bel: false, saw_esc: false },
            0x00..=0x7f => {
                if let Some(key) = decode_single_ascii(b, KeyModifiers::ALT) { self.emit_event(Event::Key(key), out); }
                else { self.reset(); }
            }
            _ => self.begin_utf8(b, true),
        }
    }

    fn begin_utf8(&mut self, lead: u8, alt: bool) {
        if let Some(expected_len) = utf8_expected_len(lead) {
            self.utf8_buf.clear();
            self.utf8_buf.push(lead);
            self.state = State::Utf8 { expected_len, alt };
        } else {
            self.reset();
        }
    }

    fn step_utf8(&mut self, b: u8, expected_len: usize, alt: bool, out: &mut Vec<DecodedInput>) {
        if (b & 0b1100_0000) != 0b1000_0000 { self.reset(); self.step_ground(b, out); return; }
        self.raw.push(b);
        self.utf8_buf.push(b);
        if self.utf8_buf.len() < expected_len { return; }
        let mods = if alt { KeyModifiers::ALT } else { KeyModifiers::NONE };
        if let Some(ch) = std::str::from_utf8(&self.utf8_buf).ok().and_then(|s| s.chars().next()) {
            self.utf8_buf.clear();
            self.emit_event(Event::Key(KeyEvent::new(KeyCode::Char(ch), mods)), out);
        } else {
            self.reset();
        }
    }

    fn step_csi(&mut self, b: u8, out: &mut Vec<DecodedInput>) {
        if b == 0x1b { self.reenter_esc(); return; }
        self.raw.push(b);
        // X10 mouse uses 3 raw bytes after \x1b[M that may fall outside 0x40..=0x7e.
        if self.raw.len() == 3 && b == b'M' { self.state = State::X10Mouse; return; }
        if (0x20..=0x3f).contains(&b) {
            if self.raw.len() >= MAX_CSI_LEN { self.reset(); }
        } else if (0x40..=0x7e).contains(&b) {
            let body = &self.raw[2..self.raw.len() - 1];
            if body == b"200" && b == b'~' {
                self.paste_buf.clear();
                self.state = State::BracketedPaste;
            } else if let Some(event) = decode_csi_sequence(body, b) {
                self.emit_event(event, out);
            } else {
                self.reset();
            }
        } else {
            self.reset();
        }
    }

    fn step_ss3(&mut self, b: u8, out: &mut Vec<DecodedInput>) {
        if b == 0x1b { self.reenter_esc(); return; }
        self.raw.push(b);
        if b.is_ascii_digit() || b == b';' || b == b':' {
            if self.raw.len() >= MAX_SS3_LEN { self.reset(); }
        } else if (0x40..=0x7e).contains(&b) {
            let params = &self.raw[2..self.raw.len() - 1];
            if let Some(key) = decode_ss3_sequence(params, b) { self.emit_event(Event::Key(key), out); }
            else { self.reset(); }
        } else {
            self.reset();
        }
    }

    fn step_delimited(&mut self, b: u8, allow_bel: bool, saw_esc: bool) {
        if (allow_bel && b == 0x07) || (saw_esc && b == b'\\') { self.reset(); return; }
        self.raw.push(b);
        if self.raw.len() >= MAX_STRING_SEQ_LEN { self.reset(); }
        else { self.state = State::Delimited { allow_bel, saw_esc: b == 0x1b }; }
    }

    fn step_x10_mouse(&mut self, b: u8, out: &mut Vec<DecodedInput>) {
        self.raw.push(b);
        if self.raw.len() < 6 { return; }
        let cb = self.raw[3].saturating_sub(32);
        let column = u16::from(self.raw[4].saturating_sub(33));
        let row = u16::from(self.raw[5].saturating_sub(33));
        self.emit_event(Event::Mouse(MouseEvent { kind: decode_mouse_kind(cb, false), column, row, modifiers: decode_mouse_modifiers(cb) }), out);
    }

    fn step_bracketed_paste(&mut self, b: u8, out: &mut Vec<DecodedInput>) {
        self.raw.push(b);
        self.paste_buf.push(b);
        if self.paste_buf.ends_with(PASTE_END_MARKER) {
            let text_len = self.paste_buf.len() - PASTE_END_MARKER.len();
            let text = String::from_utf8_lossy(&self.paste_buf[..text_len]).into_owned();
            self.paste_buf.clear();
            self.emit_event(Event::Paste(text), out);
        } else if self.paste_buf.len() >= MAX_PASTE_LEN {
            let text = String::from_utf8_lossy(&self.paste_buf).into_owned();
            self.paste_buf.clear();
            self.emit_event(Event::Paste(text), out);
        }
    }
}

fn utf8_expected_len(lead: u8) -> Option<usize> {
    match lead { 0xc2..=0xdf => Some(2), 0xe0..=0xef => Some(3), 0xf0..=0xf4 => Some(4), _ => None }
}

fn decode_single_ascii(b: u8, base_mods: KeyModifiers) -> Option<KeyEvent> {
    let (code, extra) = match b {
        0x00 => (KeyCode::Char(' '), KeyModifiers::CONTROL),
        0x09 => (KeyCode::Tab, KeyModifiers::NONE),
        0x0a | 0x0d => (KeyCode::Enter, KeyModifiers::NONE),
        0x7f => (KeyCode::Backspace, KeyModifiers::NONE),
        0x08 if base_mods.contains(KeyModifiers::ALT) => (KeyCode::Backspace, KeyModifiers::NONE),
        0x01..=0x1a => (KeyCode::Char((b - 1 + b'a') as char), KeyModifiers::CONTROL),
        0x1c => (KeyCode::Char('\\'), KeyModifiers::CONTROL),
        0x1d => (KeyCode::Char(']'), KeyModifiers::CONTROL),
        0x1e => (KeyCode::Char('^'), KeyModifiers::CONTROL),
        0x1f => (KeyCode::Char('_'), KeyModifiers::CONTROL),
        b'A'..=b'Z' => (KeyCode::Char(b as char), KeyModifiers::SHIFT),
        0x20..=0x7e => (KeyCode::Char(b as char), KeyModifiers::NONE),
        _ => return None,
    };
    Some(KeyEvent::new(code, base_mods | extra))
}

fn decode_xterm_modifier(raw: u32) -> (KeyModifiers, KeyEventState) {
    let mask = raw.saturating_sub(1);
    let mut mods = KeyModifiers::NONE;
    for (bit, flag) in [
        (1, KeyModifiers::SHIFT), (2, KeyModifiers::ALT), (4, KeyModifiers::CONTROL),
        (8, KeyModifiers::SUPER), (16, KeyModifiers::HYPER), (32, KeyModifiers::META),
    ] {
        if (mask & bit) != 0 { mods |= flag; }
    }
    let mut state = KeyEventState::NONE;
    if (mask & 64) != 0 { state |= KeyEventState::CAPS_LOCK; }
    if (mask & 128) != 0 { state |= KeyEventState::NUM_LOCK; }
    (mods, state)
}

fn decode_mod_and_kind(part: &str) -> Option<(KeyModifiers, KeyEventState, KeyEventKind)> {
    let mut sub = part.split(':');
    let mod_raw = sub.next().unwrap_or("");
    let mod_val = if mod_raw.is_empty() { 1 } else { mod_raw.parse::<u32>().ok()? };
    let kind = match sub.next() {
        None | Some("") | Some("1") => KeyEventKind::Press,
        Some("2") => KeyEventKind::Repeat,
        Some("3") => KeyEventKind::Release,
        Some(_) => return None,
    };
    if sub.next().is_some() { return None; }
    let (mods, state) = decode_xterm_modifier(mod_val);
    Some((mods, state, kind))
}

fn decode_csi_modifier_parts(params: &str) -> Option<(KeyModifiers, KeyEventState, KeyEventKind)> {
    if params.is_empty() { return Some((KeyModifiers::NONE, KeyEventState::NONE, KeyEventKind::Press)); }
    let parts: Vec<&str> = params.split(';').collect();
    match parts.as_slice() {
        [m] => decode_mod_and_kind(m),
        // Leading semicolon (`\x1b[;2A`) or explicit `1` (`\x1b[1;2A`).
        [first, m] if first.is_empty() || *first == "1" => decode_mod_and_kind(m),
        _ => None,
    }
}

fn decode_letter_keycode(final_byte: u8) -> Option<KeyCode> {
    Some(match final_byte {
        b'A' => KeyCode::Up, b'B' => KeyCode::Down, b'C' => KeyCode::Right, b'D' => KeyCode::Left,
        b'H' => KeyCode::Home, b'F' => KeyCode::End,
        b'P' => KeyCode::F(1), b'Q' => KeyCode::F(2), b'R' => KeyCode::F(3), b'S' => KeyCode::F(4),
        _ => return None,
    })
}

fn decode_ss3_sequence(params: &[u8], final_byte: u8) -> Option<KeyEvent> {
    let (mods, state, kind) = decode_csi_modifier_parts(std::str::from_utf8(params).ok()?)?;
    let (code, rxvt_mods) = match final_byte {
        b'a' => (KeyCode::Up, KeyModifiers::CONTROL),
        b'b' => (KeyCode::Down, KeyModifiers::CONTROL),
        b'c' => (KeyCode::Right, KeyModifiers::CONTROL),
        b'd' => (KeyCode::Left, KeyModifiers::CONTROL),
        _ => (decode_letter_keycode(final_byte)?, KeyModifiers::NONE),
    };
    Some(KeyEvent { code, modifiers: mods | rxvt_mods, kind, state })
}

fn decode_csi_sequence(body: &[u8], final_byte: u8) -> Option<Event> {
    let text = std::str::from_utf8(body).ok()?;
    if let Some(sgr_body) = text.strip_prefix('<') {
        return matches!(final_byte, b'M' | b'm')
            .then(|| decode_sgr_mouse(sgr_body, final_byte == b'm').map(Event::Mouse))
            .flatten();
    }
    if text.starts_with(['?', '>', '=']) { return None; }
    match final_byte {
        b'I' if text.is_empty() => Some(Event::FocusGained),
        b'O' if text.is_empty() => Some(Event::FocusLost),
        b'Z' => {
            let (mods, state, kind) = decode_csi_modifier_parts(text)?;
            Some(Event::Key(KeyEvent { code: KeyCode::BackTab, modifiers: mods | KeyModifiers::SHIFT, kind, state }))
        }
        b'a'..=b'd' if text.is_empty() => Some(Event::Key(KeyEvent::new(
            decode_letter_keycode(final_byte.to_ascii_uppercase())?,
            KeyModifiers::SHIFT,
        ))),
        b'A'..=b'D' | b'H' | b'F' | b'P'..=b'S' => {
            let (mods, state, kind) = decode_csi_modifier_parts(text)?;
            Some(Event::Key(KeyEvent { code: decode_letter_keycode(final_byte)?, modifiers: mods, kind, state }))
        }
        b'~' => decode_csi_tilde_key(text).map(Event::Key),
        b'u' => decode_kitty_u_key(text).map(Event::Key),
        _ => None,
    }
}

fn decode_csi_tilde_key(params: &str) -> Option<KeyEvent> {
    let parts: Vec<&str> = params.split(';').collect();
    if parts.len() == 3 && parts[0] == "27" {
        let (mods, state, kind) = decode_mod_and_kind(parts[1])?;
        let code = decode_unicode_or_control_codepoint(parts[2].parse::<u32>().ok()?, mods, None)?;
        return Some(KeyEvent { code, modifiers: mods, kind, state });
    }
    if parts.len() > 2 { return None; }
    let key_num = parts.first()?.parse::<u16>().ok()?;
    let (mods, state, kind) = match parts.get(1) {
        Some(m) => decode_mod_and_kind(m)?,
        None => (KeyModifiers::NONE, KeyEventState::NONE, KeyEventKind::Press),
    };
    let code = match key_num {
        1 | 7 => KeyCode::Home, 2 => KeyCode::Insert, 3 => KeyCode::Delete,
        4 | 8 => KeyCode::End, 5 => KeyCode::PageUp, 6 => KeyCode::PageDown,
        11..=15 => KeyCode::F((key_num - 10) as u8),
        17..=21 => KeyCode::F((key_num - 11) as u8),
        23..=24 => KeyCode::F((key_num - 12) as u8),
        25..=26 => KeyCode::F((key_num - 13) as u8),
        28..=29 => KeyCode::F((key_num - 14) as u8),
        31..=34 => KeyCode::F((key_num - 15) as u8),
        _ => return None,
    };
    Some(KeyEvent { code, modifiers: mods, kind, state })
}

fn decode_kitty_u_key(params: &str) -> Option<KeyEvent> {
    let parts: Vec<&str> = params.split(';').collect();
    if parts.is_empty() || parts.len() > 3 { return None; }
    let mut cp_parts = parts[0].split(':');
    let cp = cp_parts.next()?.parse::<u32>().ok()?;
    let shifted_cp = cp_parts.next().filter(|s| !s.is_empty()).and_then(|s| s.parse::<u32>().ok());
    let (mods, mut state, kind) = match parts.get(1) {
        Some(m) => decode_mod_and_kind(m)?,
        None => (KeyModifiers::NONE, KeyEventState::NONE, KeyEventKind::Press),
    };
    let (code, extra_state) = decode_kitty_codepoint(cp, mods, shifted_cp)?;
    state |= extra_state;
    Some(KeyEvent { code, modifiers: mods, kind, state })
}

fn decode_unicode_or_control_codepoint(cp: u32, mods: KeyModifiers, shifted_cp: Option<u32>) -> Option<KeyCode> {
    match cp {
        9 => Some(KeyCode::Tab), 13 => Some(KeyCode::Enter), 27 => Some(KeyCode::Esc), 8 | 127 => Some(KeyCode::Backspace),
        _ => char::from_u32(if mods == KeyModifiers::SHIFT { shifted_cp.unwrap_or(cp) } else { cp }).map(KeyCode::Char),
    }
}

fn decode_kitty_codepoint(cp: u32, mods: KeyModifiers, shifted_cp: Option<u32>) -> Option<(KeyCode, KeyEventState)> {
    let mut state = KeyEventState::NONE;
    let code = match cp {
        57344..=57363 => KITTY_FUNCTIONAL_KEYS[(cp - 57344) as usize],
        57364..=57398 => KeyCode::F((cp - 57363) as u8),
        57399..=57408 => { state |= KeyEventState::KEYPAD; KeyCode::Char(char::from_u32(b'0' as u32 + (cp - 57399))?) }
        57409..=57427 => { state |= KeyEventState::KEYPAD; KITTY_KEYPAD_KEYS[(cp - 57409) as usize] }
        57428..=57440 => KeyCode::Media(KITTY_MEDIA_KEYS[(cp - 57428) as usize]),
        57441..=57452 => KeyCode::Modifier(KITTY_MODIFIER_KEYS[(cp - 57441) as usize]),
        57453 => KeyCode::Modifier(ModifierKeyCode::IsoLevel3Shift),
        57454 => KeyCode::Modifier(ModifierKeyCode::IsoLevel5Shift),
        _ => decode_unicode_or_control_codepoint(cp, mods, shifted_cp)?,
    };
    Some((code, state))
}

fn decode_sgr_mouse(body: &str, is_release: bool) -> Option<MouseEvent> {
    let mut parts = body.split(';');
    let cb = parts.next()?.parse::<u16>().ok()? as u8;
    let column = parts.next()?.parse::<u16>().ok()?.saturating_sub(1);
    let row = parts.next()?.parse::<u16>().ok()?.saturating_sub(1);
    if parts.next().is_some() { return None; }
    Some(MouseEvent { kind: decode_mouse_kind(cb, is_release), column, row, modifiers: decode_mouse_modifiers(cb) })
}

fn decode_mouse_modifiers(cb: u8) -> KeyModifiers {
    let mut mods = KeyModifiers::NONE;
    if (cb & 4) != 0 { mods |= KeyModifiers::SHIFT; }
    if (cb & 8) != 0 { mods |= KeyModifiers::ALT; }
    if (cb & 16) != 0 { mods |= KeyModifiers::CONTROL; }
    mods
}

fn decode_mouse_kind(cb: u8, sgr_release: bool) -> MouseEventKind {
    let button = match cb & 3 { 0 => MouseButton::Left, 1 => MouseButton::Middle, _ => MouseButton::Right };
    if (cb & 64) != 0 {
        return match cb & 3 {
            0 => MouseEventKind::ScrollUp, 1 => MouseEventKind::ScrollDown,
            2 => MouseEventKind::ScrollLeft, _ => MouseEventKind::ScrollRight,
        };
    }
    if (cb & 32) != 0 {
        return match cb & 3 {
            0 => MouseEventKind::Drag(MouseButton::Left), 1 => MouseEventKind::Drag(MouseButton::Middle),
            2 => MouseEventKind::Drag(MouseButton::Right), _ => MouseEventKind::Moved,
        };
    }
    if sgr_release || (cb & 3) == 3 { MouseEventKind::Up(button) } else { MouseEventKind::Down(button) }
}
