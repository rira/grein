use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use crate::keymap::{KeyChord, Keymap};
use crate::terminal::providers::default_adapter;
use crate::terminal::{
    KeyboardProtocolPolicy, TerminalCapabilities, TerminalEnv, TerminalSessionHost,
    TerminalSessionOptions,
};
use crate::terminal_decoder::{DecodedInput, UnixRawInputReader};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyInspectorOptions {
    pub enhance_keyboard: bool,
    pub raw_bytes: bool,
}

struct RawInspectorGuard {
    host: TerminalSessionHost,
    enhancement_status: String,
}

fn format_enhancement_status(
    enhance_keyboard: bool,
    policy: KeyboardProtocolPolicy,
    keyboard_enhanced: bool,
) -> String {
    if !enhance_keyboard {
        return "disabled (--no-enhance), active=false".to_string();
    }
    match policy {
        KeyboardProtocolPolicy::None => "policy=None, active=false".to_string(),
        KeyboardProtocolPolicy::ModifyOtherKeys => {
            "policy=ModifyOtherKeys, active=true".to_string()
        }
        KeyboardProtocolPolicy::KittyDisambiguate => {
            "policy=KittyDisambiguate, active=true (DISAMBIGUATE_ESCAPE_CODES)".to_string()
        }
        KeyboardProtocolPolicy::BoundedProbeKitty { timeout_ms } => format!(
            "policy=BoundedProbeKitty({timeout_ms}ms), active={keyboard_enhanced}"
        ),
    }
}

impl RawInspectorGuard {
    fn enter(options: KeyInspectorOptions) -> io::Result<Self> {
        let env = TerminalEnv::from_env();
        let adapter = default_adapter(&env);
        let session_opts = TerminalSessionOptions {
            alternate_screen: false,
            mouse_capture: false,
            bracketed_paste: false,
            focus_change: false,
            enhance_keyboard: options.enhance_keyboard,
            raw_bytes: options.raw_bytes,
        };
        let host = TerminalSessionHost::new(adapter, env, session_opts)?;
        let enhancement_status = format_enhancement_status(
            options.enhance_keyboard,
            host.capabilities().keyboard_protocol,
            host.active_modes().keyboard_enhanced,
        );
        Ok(Self {
            host,
            enhancement_status,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PendingExitKey {
    #[default]
    None,
    CtrlC,
    PlainQ,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitStep {
    Continue,
    ArmCtrlC,
    ArmPlainQ,
    Exit,
}

impl PendingExitKey {
    fn step(&mut self, key: KeyEvent) -> ExitStep {
        let is_ctrl_c = key.modifiers == KeyModifiers::CONTROL
            && matches!(key.code, KeyCode::Char('c' | 'C'));
        let is_plain_q = key.modifiers.is_empty() && matches!(key.code, KeyCode::Char('q'));

        match (*self, is_ctrl_c, is_plain_q) {
            (Self::CtrlC, true, _) | (Self::PlainQ, _, true) => {
                *self = Self::None;
                ExitStep::Exit
            }
            (_, true, _) => {
                *self = Self::CtrlC;
                ExitStep::ArmCtrlC
            }
            (_, _, true) => {
                *self = Self::PlainQ;
                ExitStep::ArmPlainQ
            }
            _ => {
                *self = Self::None;
                ExitStep::Continue
            }
        }
    }
}

pub fn format_inspector_header(
    term: Option<&str>,
    term_program: Option<&str>,
    colorterm: Option<&str>,
    enhancement_status: &str,
    adapter_name: &str,
    capabilities: &TerminalCapabilities,
) -> String {
    [
        "Grein Key Inspector".to_string(),
        format!(
            "TERM={}  TERM_PROGRAM={}  COLORTERM={}",
            term.unwrap_or("<unset>"),
            term_program.unwrap_or("<unset>"),
            colorterm.unwrap_or("<unset>"),
        ),
        format!(
            "adapter={}  color={:?}  keyboard_protocol={:?}  osc52={}  focus={}  paste={}",
            adapter_name,
            capabilities.color,
            capabilities.keyboard_protocol,
            capabilities.osc52_clipboard,
            capabilities.focus_events,
            capabilities.bracketed_paste,
        ),
        format!("keyboard_enhancement={enhancement_status}"),
        "Press any key to inspect raw terminal events (press Ctrl+C twice or q twice to exit)"
            .to_string(),
    ]
    .join("\n")
}

fn format_modifiers(modifiers: KeyModifiers) -> String {
    let flags = [
        (KeyModifiers::CONTROL, "CONTROL"),
        (KeyModifiers::ALT, "ALT"),
        (KeyModifiers::SHIFT, "SHIFT"),
        (KeyModifiers::SUPER, "SUPER"),
        (KeyModifiers::HYPER, "HYPER"),
        (KeyModifiers::META, "META"),
    ];
    let parts: Vec<&str> = flags
        .into_iter()
        .filter_map(|(flag, name)| modifiers.contains(flag).then_some(name))
        .collect();
    if parts.is_empty() {
        "NONE".to_string()
    } else {
        parts.join("|")
    }
}

fn format_key_state(state: KeyEventState) -> String {
    let flags = [
        (KeyEventState::KEYPAD, "KEYPAD"),
        (KeyEventState::CAPS_LOCK, "CAPS_LOCK"),
        (KeyEventState::NUM_LOCK, "NUM_LOCK"),
    ];
    let parts: Vec<&str> = flags
        .into_iter()
        .filter_map(|(flag, name)| state.contains(flag).then_some(name))
        .collect();
    if parts.is_empty() {
        "NONE".to_string()
    } else {
        parts.join("|")
    }
}

fn format_unsupported_modifiers(modifiers: KeyModifiers) -> String {
    let flags = [
        (KeyModifiers::SUPER, "SUPER"),
        (KeyModifiers::HYPER, "HYPER"),
        (KeyModifiers::META, "META"),
    ];
    let parts: Vec<&str> = flags
        .into_iter()
        .filter_map(|(flag, name)| modifiers.contains(flag).then_some(name))
        .collect();
    if parts.is_empty() {
        "SUPER/HYPER/META".to_string()
    } else {
        parts.join("/")
    }
}

pub fn format_raw_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "hex=<none> escaped=\"\"".to_string();
    }
    let hex = bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut escaped = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        match b {
            b'\x1b' => escaped.push_str("\\x1b"),
            b'\n' => escaped.push_str("\\n"),
            b'\r' => escaped.push_str("\\r"),
            b'\t' => escaped.push_str("\\t"),
            b'\\' => escaped.push_str("\\\\"),
            b'"' => escaped.push_str("\\\""),
            0x20..=0x7e => escaped.push(b as char),
            _ => escaped.push_str(&format!("\\x{b:02x}")),
        }
    }
    format!("hex={hex} escaped=\"{escaped}\"")
}

pub fn format_inspected_key_event(key: KeyEvent, keymap: &Keymap) -> String {
    let chord_str = match KeyChord::from_event(key) {
        Some(chord) => format!("{} ({})", chord.display_label(), chord.canonical_string()),
        None => format!(
            "ignored (unsupported modifier: {})",
            format_unsupported_modifiers(key.modifiers)
        ),
    };
    let action_str = match keymap.match_write_key(key) {
        Some(action) => format!("{} ({})", action.as_str(), action.description()),
        None => "none".to_string(),
    };

    let mut hints = Vec::new();
    if key.modifiers.is_empty()
        && matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
        )
    {
        hints.push(
            "Shift+Arrow sent without Shift? Try Ctrl+G manage mode or Terminal.app keyboard profile.",
        );
    }
    if !key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Char(c) if !c.is_ascii())
    {
        hints.push(
            "Received composed character (macOS Option composition active — Option+ keys produce symbols while Ctrl+G works everywhere).",
        );
    }

    let mut out = format!(
        "code={:?} modifiers={} kind={:?} state={} chord={} action={}",
        key.code,
        format_modifiers(key.modifiers),
        key.kind,
        format_key_state(key.state),
        chord_str,
        action_str,
    );
    for hint in hints {
        out.push_str("\n  hint: ");
        out.push_str(hint);
    }
    out
}

pub fn format_decoded_input(
    decoded: &DecodedInput,
    keymap: &Keymap,
    include_raw_bytes: bool,
) -> String {
    let base = match decoded.event {
        Event::Key(key) => format_inspected_key_event(key, keymap),
        ref other => format!("event={other:?}"),
    };
    if !include_raw_bytes {
        return base;
    }
    let raw = format_raw_bytes(&decoded.raw_bytes);
    match base.split_once('\n') {
        Some((first, rest)) => format!("{first} raw=[{raw}]\n{rest}"),
        None => format!("{base} raw=[{raw}]"),
    }
}

fn write_raw_lines(out: &mut impl Write, text: &str) -> io::Result<()> {
    let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
    write!(out, "{normalized}\r\n")?;
    out.flush()
}

pub fn run_key_inspector(options: KeyInspectorOptions) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "grein keys requires an interactive terminal (TTY)",
        ));
    }

    let mut guard = RawInspectorGuard::enter(options)?;
    let initial_bytes = guard.host.take_pending_input_bytes();
    let mut reader = UnixRawInputReader::new()?;
    reader.push_initial_bytes(&initial_bytes);

    let env = guard.host.env();
    let header = format_inspector_header(
        env.term.as_deref(),
        env.term_program.as_deref(),
        env.colorterm.as_deref(),
        &guard.enhancement_status,
        guard.host.adapter().display_name(),
        guard.host.capabilities(),
    );

    let mut stdout = io::stdout();
    write_raw_lines(&mut stdout, &header)?;

    let keymap = Keymap::default();
    let mut exit_tracker = PendingExitKey::default();

    loop {
        let Some(decoded) = reader.read_next(Duration::from_millis(250))? else {
            continue;
        };
        match decoded.event {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release && !options.raw_bytes {
                    continue;
                }
                let line = format_decoded_input(&decoded, &keymap, options.raw_bytes);
                write_raw_lines(&mut stdout, &line)?;
                if key.kind != KeyEventKind::Release {
                    match exit_tracker.step(key) {
                        ExitStep::Continue => {}
                        ExitStep::ArmCtrlC => {
                            write_raw_lines(&mut stdout, "  hint: Press Ctrl+C again to exit.")?;
                        }
                        ExitStep::ArmPlainQ => {
                            write_raw_lines(&mut stdout, "  hint: Press q again to exit.")?;
                        }
                        ExitStep::Exit => {
                            write_raw_lines(&mut stdout, "Exiting key inspector.")?;
                            break;
                        }
                    }
                }
            }
            _ if options.raw_bytes => {
                let line = format_decoded_input(&decoded, &keymap, true);
                write_raw_lines(&mut stdout, &line)?;
            }
            _ => continue,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::ColorCapability;

    #[test]
    fn format_inspected_key_event_reports_shift_up_ctrl_g_and_alt_z() {
        let keymap = Keymap::default();

        let shift_up = format_inspected_key_event(
            KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT),
            &keymap,
        );
        assert!(shift_up.contains("code=Up"), "{shift_up}");
        assert!(shift_up.contains("modifiers=SHIFT"), "{shift_up}");
        assert!(shift_up.contains("kind=Press"), "{shift_up}");
        assert!(shift_up.contains("state=NONE"), "{shift_up}");
        assert!(shift_up.contains("Shift+↑ (shift+up)"), "{shift_up}");
        assert!(shift_up.contains("tree_up"), "{shift_up}");
        assert!(
            !shift_up.contains("Shift+Arrow sent without Shift?"),
            "{shift_up}"
        );

        let alt_z = format_inspected_key_event(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT),
            &keymap,
        );
        assert!(alt_z.contains("modifiers=ALT"), "{alt_z}");
        assert!(alt_z.contains("Alt+Z (alt+z)"), "{alt_z}");
        assert!(alt_z.contains("toggle_zoom"), "{alt_z}");

        let ctrl_g = format_inspected_key_event(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            &keymap,
        );
        assert!(ctrl_g.contains("modifiers=CONTROL"), "{ctrl_g}");
        assert!(ctrl_g.contains("Ctrl+G (ctrl+g)"), "{ctrl_g}");
        assert!(ctrl_g.contains("enter_manage_mode"), "{ctrl_g}");
    }

    #[test]
    fn format_inspected_key_event_reports_plain_arrows_super_and_option_chars() {
        let keymap = Keymap::default();

        for arrow in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right] {
            let plain_arrow =
                format_inspected_key_event(KeyEvent::new(arrow, KeyModifiers::NONE), &keymap);
            assert!(
                plain_arrow.contains(
                    "Shift+Arrow sent without Shift? Try Ctrl+G manage mode or Terminal.app keyboard profile."
                ),
                "{plain_arrow}"
            );
        }

        let super_up = format_inspected_key_event(
            KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER),
            &keymap,
        );
        assert!(
            super_up.contains("ignored (unsupported modifier: SUPER)"),
            "{super_up}"
        );
        assert!(super_up.contains("action=none"), "{super_up}");

        let omega = format_inspected_key_event(
            KeyEvent::new(KeyCode::Char('Ω'), KeyModifiers::NONE),
            &keymap,
        );
        assert!(
            omega.contains(
                "Received composed character (macOS Option composition active — Option+ keys produce symbols while Ctrl+G works everywhere)."
            ),
            "{omega}"
        );

        let bracket = format_inspected_key_event(
            KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE),
            &keymap,
        );
        assert!(bracket.contains("code=Char('[')"), "{bracket}");
        assert!(bracket.contains("action=none"), "{bracket}");
        assert!(
            !bracket.contains("Received composed character"),
            "plain ASCII '[' must not trigger non-ASCII composed character hint: {bracket}"
        );
    }

    #[test]
    fn pending_exit_key_requires_two_consecutive_ctrl_c_or_plain_q() {
        let mut tracker = PendingExitKey::default();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let plain_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let alt_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT);
        let other = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);

        assert_eq!(tracker.step(ctrl_c), ExitStep::ArmCtrlC);
        assert_eq!(tracker.step(other), ExitStep::Continue);
        assert_eq!(tracker.step(ctrl_c), ExitStep::ArmCtrlC);
        assert_eq!(tracker.step(ctrl_c), ExitStep::Exit);

        assert_eq!(tracker.step(plain_q), ExitStep::ArmPlainQ);
        assert_eq!(tracker.step(alt_q), ExitStep::Continue);
        assert_eq!(tracker.step(plain_q), ExitStep::ArmPlainQ);
        assert_eq!(tracker.step(ctrl_c), ExitStep::ArmCtrlC);
        assert_eq!(tracker.step(plain_q), ExitStep::ArmPlainQ);
        assert_eq!(tracker.step(plain_q), ExitStep::Exit);
    }

    #[test]
    fn format_inspector_header_lists_env_adapter_capabilities_and_instructions() {
        let caps = TerminalCapabilities {
            color: ColorCapability::Xterm256,
            keyboard_protocol: KeyboardProtocolPolicy::None,
            osc52_clipboard: false,
            focus_events: false,
            bracketed_paste: true,
            notes: vec![],
        };
        let header = format_inspector_header(
            Some("xterm-256color"),
            Some("Apple_Terminal"),
            None,
            "policy=None, active=false",
            "Apple Terminal",
            &caps,
        );
        assert!(header.contains("TERM=xterm-256color"), "{header}");
        assert!(header.contains("TERM_PROGRAM=Apple_Terminal"), "{header}");
        assert!(header.contains("COLORTERM=<unset>"), "{header}");
        assert!(header.contains("adapter=Apple Terminal"), "{header}");
        assert!(header.contains("color=Xterm256"), "{header}");
        assert!(header.contains("keyboard_protocol=None"), "{header}");
        assert!(
            header.contains("keyboard_enhancement=policy=None, active=false"),
            "{header}"
        );
        assert!(
            header.contains(
                "Press any key to inspect raw terminal events (press Ctrl+C twice or q twice to exit)"
            ),
            "{header}"
        );
    }
}
