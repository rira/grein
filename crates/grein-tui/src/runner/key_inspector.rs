use std::io::{self, IsTerminal, Write};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement,
};

use crate::keymap::{KeyChord, Keymap};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyInspectorOptions {
    pub enhance_keyboard: bool,
}

struct RawInspectorGuard {
    raw_mode: bool,
    keyboard_enhanced: bool,
    enhancement_status: String,
}

impl RawInspectorGuard {
    fn enter(enhance_keyboard: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut guard = Self {
            raw_mode: true,
            keyboard_enhanced: false,
            enhancement_status: String::new(),
        };

        if enhance_keyboard {
            let supported = supports_keyboard_enhancement().unwrap_or(false);
            let pushed = if supported {
                execute!(
                    io::stdout(),
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    )
                )
                .is_ok()
            } else {
                false
            };
            guard.keyboard_enhanced = pushed;
            guard.enhancement_status = if pushed {
                "supported=true, active=true (DISAMBIGUATE_ESCAPE_CODES)".to_string()
            } else {
                format!("supported={supported}, active=false")
            };
        } else {
            guard.enhancement_status = "disabled (--no-enhance), active=false".to_string();
        }

        Ok(guard)
    }
}

impl Drop for RawInspectorGuard {
    fn drop(&mut self) {
        if self.keyboard_enhanced {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        if self.raw_mode {
            let _ = disable_raw_mode();
        }
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

fn read_non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn format_inspector_header(
    term: Option<&str>,
    term_program: Option<&str>,
    colorterm: Option<&str>,
    enhancement_status: &str,
) -> String {
    [
        "Grein Key Inspector".to_string(),
        format!(
            "TERM={}  TERM_PROGRAM={}  COLORTERM={}",
            term.unwrap_or("<unset>"),
            term_program.unwrap_or("<unset>"),
            colorterm.unwrap_or("<unset>"),
        ),
        format!("keyboard_enhancement={enhancement_status}"),
        "Press any key to inspect raw crossterm events (press Ctrl+C twice or q twice to exit)"
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

    let guard = RawInspectorGuard::enter(options.enhance_keyboard)?;
    let term = read_non_empty_env("TERM");
    let term_program = read_non_empty_env("TERM_PROGRAM");
    let colorterm = read_non_empty_env("COLORTERM");
    let header = format_inspector_header(
        term.as_deref(),
        term_program.as_deref(),
        colorterm.as_deref(),
        &guard.enhancement_status,
    );

    let mut stdout = io::stdout();
    write_raw_lines(&mut stdout, &header)?;

    let keymap = Keymap::default();
    let mut exit_tracker = PendingExitKey::default();

    loop {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Release => continue,
            Event::Key(key) => {
                let line = format_inspected_key_event(key, &keymap);
                write_raw_lines(&mut stdout, &line)?;
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
            _ => continue,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!shift_up.contains("Shift+Arrow sent without Shift?"), "{shift_up}");

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
    fn format_inspector_header_lists_env_and_instructions() {
        let header = format_inspector_header(
            Some("xterm-256color"),
            Some("Apple_Terminal"),
            None,
            "supported=false, active=false",
        );
        assert!(header.contains("TERM=xterm-256color"), "{header}");
        assert!(header.contains("TERM_PROGRAM=Apple_Terminal"), "{header}");
        assert!(header.contains("COLORTERM=<unset>"), "{header}");
        assert!(
            header.contains("keyboard_enhancement=supported=false, active=false"),
            "{header}"
        );
        assert!(
            header.contains(
                "Press any key to inspect raw crossterm events (press Ctrl+C twice or q twice to exit)"
            ),
            "{header}"
        );
    }
}
