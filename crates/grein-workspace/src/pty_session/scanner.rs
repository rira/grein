pub(super) const MAX_OSC_PAYLOAD_BYTES: usize = 8192;
const MAX_CSI_PARAMS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ScanState {
    #[default]
    Ground,
    Utf8 {
        remaining: u8,
    },
    Esc,
    EscIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    OscEsc,
    DcsString,
    DcsEsc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScannerEvent {
    DeviceStatus,
    CursorPosition,
    DecCursorPosition,
    PrimaryDeviceAttributes,
    SecondaryDeviceAttributes,
    OscForegroundColorQuery,
    OscBackgroundColorQuery,
    OscCwd(String),
    ScrollRegion { top: u16, bottom: u16 },
    ScrollUp(u16),
    Reset,
}

#[derive(Default)]
pub(super) struct TerminalScanner {
    state: ScanState,
    csi_prefix: Option<u8>,
    csi_params: [u16; MAX_CSI_PARAMS],
    csi_param_len: usize,
    csi_current_param: Option<u16>,
    csi_has_intermediate: bool,
    csi_ignore: bool,
    pub(super) osc_buf: Vec<u8>,
    pub(super) osc_overflow: bool,
}

impl TerminalScanner {
    pub(super) fn executes_c0(&self) -> bool {
        matches!(
            self.state,
            ScanState::Ground
                | ScanState::Esc
                | ScanState::EscIntermediate
                | ScanState::CsiEntry
                | ScanState::CsiParam
                | ScanState::CsiIntermediate
                | ScanState::CsiIgnore
                | ScanState::OscEsc
                | ScanState::DcsEsc
        )
    }

    pub(super) fn will_print_byte(&self, byte: u8) -> bool {
        match self.state {
            ScanState::Ground => (0x20..=0x7e).contains(&byte),
            ScanState::Utf8 { remaining } => remaining == 1 || !(0x80..=0xbf).contains(&byte),
            _ => false,
        }
    }

    fn reset_csi(&mut self) {
        self.csi_prefix = None;
        self.csi_param_len = 0;
        self.csi_current_param = None;
        self.csi_has_intermediate = false;
        self.csi_ignore = false;
    }

    fn reset_osc(&mut self) {
        self.osc_buf.clear();
        self.osc_overflow = false;
    }

    fn push_csi_param(&mut self, value: u16) {
        if self.csi_param_len < MAX_CSI_PARAMS {
            self.csi_params[self.csi_param_len] = value;
            self.csi_param_len += 1;
        } else {
            self.csi_ignore = true;
        }
    }

    fn finish_csi(&mut self, final_byte: u8) -> Option<ScannerEvent> {
        if self.csi_current_param.is_some() || self.csi_param_len > 0 {
            let val = self.csi_current_param.take().unwrap_or(0);
            self.push_csi_param(val);
        }
        self.state = ScanState::Ground;
        if self.csi_ignore || self.csi_has_intermediate {
            return None;
        }
        let params = &self.csi_params[..self.csi_param_len];
        match (self.csi_prefix, final_byte, params) {
            (None, b'n', &[5]) => Some(ScannerEvent::DeviceStatus),
            (None, b'n', &[6]) => Some(ScannerEvent::CursorPosition),
            (Some(b'?'), b'n', &[6]) => Some(ScannerEvent::DecCursorPosition),
            (None, b'c', &[] | &[0]) => Some(ScannerEvent::PrimaryDeviceAttributes),
            (Some(b'>'), b'c', &[] | &[0]) => Some(ScannerEvent::SecondaryDeviceAttributes),
            (None, b'r', _) => Some(ScannerEvent::ScrollRegion {
                top: params.first().copied().unwrap_or(0),
                bottom: params.get(1).copied().unwrap_or(0),
            }),
            (None, b'S', _) => Some(ScannerEvent::ScrollUp(
                params.first().copied().unwrap_or(0).max(1),
            )),
            _ => None,
        }
    }

    fn finish_osc(&mut self) -> Option<ScannerEvent> {
        let event = if self.osc_overflow {
            None
        } else {
            parse_osc_payload(&self.osc_buf)
        };
        self.reset_osc();
        self.state = ScanState::Ground;
        event
    }

    fn handle_esc_byte(&mut self, byte: u8) -> Option<ScannerEvent> {
        match byte {
            0x1b => {
                self.state = ScanState::Esc;
                None
            }
            0x18 | 0x1a => {
                self.state = ScanState::Ground;
                None
            }
            0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => None,
            b'[' => {
                self.reset_csi();
                self.state = ScanState::CsiEntry;
                None
            }
            b']' => {
                self.reset_osc();
                self.state = ScanState::OscString;
                None
            }
            b'P' | b'X' | b'^' | b'_' => {
                self.state = ScanState::DcsString;
                None
            }
            0x20..=0x2f => {
                self.state = ScanState::EscIntermediate;
                None
            }
            b'c' => {
                self.state = ScanState::Ground;
                Some(ScannerEvent::Reset)
            }
            _ => {
                self.state = ScanState::Ground;
                None
            }
        }
    }

    pub(super) fn feed_byte(&mut self, byte: u8) -> Option<ScannerEvent> {
        if let ScanState::Utf8 { remaining } = self.state {
            if (0x80..=0xbf).contains(&byte) && remaining > 1 {
                self.state = ScanState::Utf8 {
                    remaining: remaining - 1,
                };
            } else {
                self.state = ScanState::Ground;
            }
            return None;
        }

        match self.state {
            ScanState::Ground => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0xc2..=0xdf => {
                    self.state = ScanState::Utf8 { remaining: 1 };
                    None
                }
                0xe0..=0xef => {
                    self.state = ScanState::Utf8 { remaining: 2 };
                    None
                }
                0xf0..=0xf4 => {
                    self.state = ScanState::Utf8 { remaining: 3 };
                    None
                }
                _ => None,
            },
            ScanState::Utf8 { .. } => unreachable!(),
            ScanState::Esc => self.handle_esc_byte(byte),
            ScanState::EscIntermediate => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x20..=0x2f | 0x7f => None,
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::CsiEntry | ScanState::CsiParam => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => None,
                b'0'..=b'9' => {
                    let digit = u16::from(byte - b'0');
                    let next = self
                        .csi_current_param
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit);
                    self.csi_current_param = Some(next);
                    self.state = ScanState::CsiParam;
                    None
                }
                b';' => {
                    let val = self.csi_current_param.take().unwrap_or(0);
                    self.push_csi_param(val);
                    self.state = ScanState::CsiParam;
                    None
                }
                b':' => {
                    self.csi_ignore = true;
                    self.state = ScanState::CsiParam;
                    None
                }
                0x3c..=0x3f => {
                    if self.state == ScanState::CsiEntry {
                        self.csi_prefix = Some(byte);
                        self.state = ScanState::CsiParam;
                    } else {
                        self.csi_ignore = true;
                        self.state = ScanState::CsiIgnore;
                    }
                    None
                }
                0x20..=0x2f => {
                    self.csi_has_intermediate = true;
                    self.state = ScanState::CsiIntermediate;
                    None
                }
                0x40..=0x7e => self.finish_csi(byte),
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::CsiIntermediate => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x20..=0x2f | 0x7f => None,
                0x30..=0x3f => {
                    self.state = ScanState::CsiIgnore;
                    None
                }
                0x40..=0x7e => {
                    self.state = ScanState::Ground;
                    None
                }
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::CsiIgnore => match byte {
                0x1b => {
                    self.state = ScanState::Esc;
                    None
                }
                0x18 | 0x1a | 0x40..=0x7e => {
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x3f | 0x7f => None,
                _ => {
                    self.state = ScanState::Ground;
                    None
                }
            },
            ScanState::OscString => match byte {
                0x07 => self.finish_osc(),
                0x1b => {
                    self.state = ScanState::OscEsc;
                    None
                }
                0x18 | 0x1a => {
                    self.reset_osc();
                    self.state = ScanState::Ground;
                    None
                }
                0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f => None,
                _ => {
                    if !self.osc_overflow {
                        if self.osc_buf.len() < MAX_OSC_PAYLOAD_BYTES {
                            self.osc_buf.push(byte);
                        } else {
                            self.osc_overflow = true;
                        }
                    }
                    None
                }
            },
            ScanState::OscEsc => {
                if byte == b'\\' {
                    self.finish_osc()
                } else {
                    self.reset_osc();
                    self.handle_esc_byte(byte)
                }
            }
            ScanState::DcsString => match byte {
                0x1b => {
                    self.state = ScanState::DcsEsc;
                    None
                }
                0x18 | 0x1a | 0x9c => {
                    self.state = ScanState::Ground;
                    None
                }
                _ => None,
            },
            ScanState::DcsEsc => {
                if byte == b'\\' {
                    self.state = ScanState::Ground;
                    None
                } else {
                    self.handle_esc_byte(byte)
                }
            }
        }
    }
}

fn parse_osc_payload(payload: &[u8]) -> Option<ScannerEvent> {
    let sep = payload.iter().position(|&b| b == b';')?;
    let code = &payload[..sep];
    let value = &payload[sep + 1..];
    match code {
        b"10" if value == b"?" => Some(ScannerEvent::OscForegroundColorQuery),
        b"11" if value == b"?" => Some(ScannerEvent::OscBackgroundColorQuery),
        b"7" => parse_osc7_cwd(value).map(ScannerEvent::OscCwd),
        _ => None,
    }
}

pub(super) fn parse_osc7_cwd(value: &[u8]) -> Option<String> {
    const FILE_SCHEME: &[u8] = b"file://";
    if value.len() < FILE_SCHEME.len()
        || !value[..FILE_SCHEME.len()].eq_ignore_ascii_case(FILE_SCHEME)
    {
        return None;
    }
    let after_scheme = &value[FILE_SCHEME.len()..];
    let slash_pos = after_scheme.iter().position(|&b| b == b'/')?;
    percent_decode_utf8(&after_scheme[slash_pos..])
}

fn percent_decode_utf8(bytes: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    if out.contains(&0) {
        return None;
    }
    String::from_utf8(out).ok()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
