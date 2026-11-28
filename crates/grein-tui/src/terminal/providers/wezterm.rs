use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalSessionOptions,
};
use crate::terminal::providers::{
    bounded_probe_kitty_plan, has_non_empty, matches_term_program, standard_osc52,
    WEZTERM_KITTY_PROBE_TIMEOUT_MS,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct WezTermProvider;

impl TerminalAdapter for WezTermProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::WezTerm
    }

    fn display_name(&self) -> &'static str {
        TerminalKind::WezTerm.display_name()
    }

    fn matches(&self, env: &TerminalEnv) -> Option<u8> {
        (matches_term_program(env, "WezTerm") || has_non_empty(&env.wezterm_executable))
            .then_some(65)
    }

    fn capabilities(&self, _env: &TerminalEnv) -> TerminalCapabilities {
        TerminalCapabilities {
            color: ColorCapability::TrueColor,
            keyboard_protocol: KeyboardProtocolPolicy::BoundedProbeKitty {
                timeout_ms: WEZTERM_KITTY_PROBE_TIMEOUT_MS,
            },
            osc52_clipboard: true,
            focus_events: true,
            bracketed_paste: true,
            notes: vec![
                "WezTerm adapter (TrueColor, bounded Kitty keyboard probe, OSC 52)".to_string(),
            ],
        }
    }

    fn protocol_plan(
        &self,
        _env: &TerminalEnv,
        opts: &TerminalSessionOptions,
    ) -> TerminalProtocolPlan {
        bounded_probe_kitty_plan(opts, WEZTERM_KITTY_PROBE_TIMEOUT_MS)
    }

    fn encode_osc52(&self, payload_b64: &str) -> String {
        standard_osc52(payload_b64)
    }

    fn diagnostic_hints(&self, _env: &TerminalEnv) -> Vec<TerminalHint> {
        vec![TerminalHint {
            level: HintLevel::Info,
            message: "WezTerm detected: run `grein terminal setup wezterm` to enable `enable_kitty_keyboard = true`.".to_string(),
        }]
    }
}
