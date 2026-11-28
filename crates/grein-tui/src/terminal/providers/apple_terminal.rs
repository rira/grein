use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalSessionOptions,
};
use crate::terminal::providers::{matches_term_program, standard_osc52};

#[derive(Debug, Clone, Copy, Default)]
pub struct AppleTerminalProvider;

impl AppleTerminalProvider {
    fn has_explicit_truecolor_colorterm(env: &TerminalEnv) -> bool {
        env.colorterm.as_deref().is_some_and(|v| {
            let lower = v.trim().to_ascii_lowercase();
            lower == "truecolor" || lower == "24bit"
        })
    }
}

impl TerminalAdapter for AppleTerminalProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::AppleTerminal
    }

    fn display_name(&self) -> &'static str {
        TerminalKind::AppleTerminal.display_name()
    }

    fn matches(&self, env: &TerminalEnv) -> Option<u8> {
        matches_term_program(env, "Apple_Terminal").then_some(70)
    }

    fn capabilities(&self, env: &TerminalEnv) -> TerminalCapabilities {
        let color = if Self::has_explicit_truecolor_colorterm(env) {
            ColorCapability::TrueColor
        } else {
            ColorCapability::Xterm256
        };
        TerminalCapabilities {
            color,
            keyboard_protocol: KeyboardProtocolPolicy::None,
            osc52_clipboard: false,
            focus_events: false,
            bracketed_paste: true,
            notes: vec![
                "Apple_Terminal adapter (legacy xterm input, no Kitty keyboard probing)"
                    .to_string(),
            ],
        }
    }

    fn protocol_plan(
        &self,
        _env: &TerminalEnv,
        _opts: &TerminalSessionOptions,
    ) -> TerminalProtocolPlan {
        TerminalProtocolPlan::default()
    }

    fn encode_osc52(&self, payload_b64: &str) -> String {
        standard_osc52(payload_b64)
    }

    fn diagnostic_hints(&self, env: &TerminalEnv) -> Vec<TerminalHint> {
        let mut hints = vec![TerminalHint {
            level: HintLevel::Warn,
            message: "Apple_Terminal detected: run `grein terminal setup apple-terminal` to configure Shift+Arrows, use `Ctrl+G` manage mode as a universal fallback, and verify bindings with `grein keys`.".to_string(),
        }];
        if !Self::has_explicit_truecolor_colorterm(env) {
            hints.push(TerminalHint {
                level: HintLevel::Info,
                message: "Apple_Terminal without COLORTERM=truecolor uses 256-color fallback (truecolor requires macOS 26 Tahoe+ or COLORTERM=truecolor).".to_string(),
            });
        }
        hints
    }
}
