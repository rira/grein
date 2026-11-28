use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalSessionOptions,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct GenericXtermProvider;

impl TerminalAdapter for GenericXtermProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::GenericXterm
    }

    fn display_name(&self) -> &'static str {
        TerminalKind::GenericXterm.display_name()
    }

    fn matches(&self, _env: &TerminalEnv) -> Option<u8> {
        Some(10)
    }

    fn capabilities(&self, env: &TerminalEnv) -> TerminalCapabilities {
        let color = if env.supports_truecolor_hint() {
            ColorCapability::TrueColor
        } else {
            ColorCapability::Xterm256
        };

        TerminalCapabilities {
            color,
            keyboard_protocol: KeyboardProtocolPolicy::BoundedProbeKitty { timeout_ms: 120 },
            osc52_clipboard: true,
            focus_events: true,
            bracketed_paste: true,
            notes: vec!["Baseline generic xterm adapter".to_string()],
        }
    }

    fn protocol_plan(
        &self,
        _env: &TerminalEnv,
        opts: &TerminalSessionOptions,
    ) -> TerminalProtocolPlan {
        if opts.enhance_keyboard {
            TerminalProtocolPlan {
                enter_bytes: b"\x1b[>1u".to_vec(),
                probe_kitty_timeout_ms: Some(120),
                expects_kitty_pop: true,
                leave_bytes: b"\x1b[<u".to_vec(),
            }
        } else {
            TerminalProtocolPlan::default()
        }
    }

    fn encode_osc52(&self, payload_b64: &str) -> String {
        format!("\x1b]52;c;{payload_b64}\x07")
    }

    fn diagnostic_hints(&self, env: &TerminalEnv) -> Vec<TerminalHint> {
        let mut hints = Vec::new();
        if env.term.is_none() {
            hints.push(TerminalHint {
                level: HintLevel::Warn,
                message: "TERM is not set; using generic xterm fallback".to_string(),
            });
        }
        if !env.supports_truecolor_hint() {
            hints.push(TerminalHint {
                level: HintLevel::Info,
                message: "COLORTERM=truecolor not detected; using 256-color palette".to_string(),
            });
        }
        hints
    }
}
